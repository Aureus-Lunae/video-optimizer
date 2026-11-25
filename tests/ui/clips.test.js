// ui/clips.js: how a <video> is given its clip. The scheme address first;
// when the webview reports that it cannot play that scheme (WebKitGTK on
// Linux), the same address is fetched and played from a blob: address.
// Driven with a fake <video>, fetch and object URLs (fake-window.js).

import test from 'node:test';
import assert from 'node:assert/strict';

import { createClipSources, MEDIA_ERR_SRC_NOT_SUPPORTED } from '../../ui/clips.js';
import { fakeDocument } from './fake-document.js';
import { fakeFetch, fakeObjectUrls, settle } from './fake-window.js';

const A = 'media://localhost/3-before';
const B = 'media://localhost/3-sample-1';
const C = 'media://localhost/3-sample-2';

/** Clip sources over fakes, with `count` attached <video> elements. */
function start(count = 1) {
  const doc = fakeDocument();
  const fetcher = fakeFetch();
  const urls = fakeObjectUrls();
  const sources = createClipSources({ fetch: fetcher.fetch, urls });
  const videos = Array.from({ length: count }, () => doc.createElement('video'));
  const clips = videos.map((video) => sources.attach(video));
  return { sources, fetcher, urls, videos, clips, video: videos[0], clip: clips[0] };
}

/** The element reports an error with `code`, as a webview's does. */
function fail(video, code) {
  video.error = { code };
  video.fire('error');
}

/** The scheme address could not be played: fetched, answered, and the blob address is set. */
async function switchToBlobs(w, address = A) {
  w.clip.show(address);
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  w.fetcher.answer(address);
  await settle();
}

test('a clip is first given as its scheme address, with nothing fetched', () => {
  const w = start();
  w.clip.show(A);
  assert.equal(w.video.getAttribute('src'), A);
  assert.deepEqual(w.fetcher.calls, []);
  assert.deepEqual(w.urls.created, []);
});

test('an element that cannot play the scheme (error code 4) gets a blob address made from the fetched bytes', async () => {
  const w = start();
  w.clip.show(A);
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  assert.deepEqual(w.fetcher.calls, [A], 'the same address is fetched');
  assert.equal(w.video.getAttribute('src'), A, 'the old address stays until the bytes are there');
  w.fetcher.answer(A);
  await settle();
  assert.equal(w.urls.created.length, 1);
  assert.deepEqual(w.urls.created[0].blob, { clip: A, call: 1 }, 'made from the fetched bytes');
  assert.equal(w.video.getAttribute('src'), w.urls.created[0].url);
  assert.match(w.video.getAttribute('src'), /^blob:/);
});

test('any other error does not switch to a blob address', async () => {
  for (const code of [1, 2, 3, undefined]) {
    const w = start();
    w.clip.show(A);
    fail(w.video, code);
    await settle();
    assert.deepEqual(w.fetcher.calls, [], `code ${code}`);
    assert.deepEqual(w.urls.created, [], `code ${code}`);
    assert.equal(w.video.getAttribute('src'), A, `code ${code}`);
  }
});

test('an error event with no error on the element does not switch', async () => {
  const w = start();
  w.clip.show(A);
  assert.equal(w.video.getAttribute('src'), A, 'the clip is shown');
  w.video.fire('error');
  await settle();
  assert.deepEqual(w.fetcher.calls, []);
  assert.equal(w.video.getAttribute('src'), A);
});

test('an error on a blob address does not fetch again', async () => {
  const w = start();
  await switchToBlobs(w);
  assert.equal(w.fetcher.calls.length, 1);
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  await settle();
  assert.equal(w.fetcher.calls.length, 1, 'no loop');
  assert.equal(w.urls.created.length, 1);
});

test('an error on a cleared element does not fetch', async () => {
  const w = start();
  w.clip.show(A);
  assert.equal(w.video.getAttribute('src'), A, 'the clip is shown');
  w.clip.clear();
  assert.equal(w.video.getAttribute('src'), null, 'and then cleared');
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  await settle();
  assert.deepEqual(w.fetcher.calls, []);
});

test('after one switch a new clip goes straight to the blob path, with no direct attempt', async () => {
  const w = start();
  await switchToBlobs(w);
  w.video.srcWrites.length = 0;
  w.clip.show(B);
  assert.deepEqual(w.fetcher.calls, [A, B], 'fetched at once');
  assert.deepEqual(w.video.srcWrites, [], 'the scheme address is never written');
  w.fetcher.answer(B);
  await settle();
  assert.deepEqual(w.video.srcWrites, [w.urls.created[1].url]);
});

test('the switch is remembered for the other element too', async () => {
  const w = start(2);
  await switchToBlobs(w);
  w.clips[1].show(B);
  assert.deepEqual(w.videos[1].srcWrites, [], 'the second element never gets the scheme address');
  assert.deepEqual(w.fetcher.calls, [A, B]);
});

test('without a switch a new clip uses the scheme address', async () => {
  const w = start();
  w.clip.show(A);
  fail(w.video, 3);
  w.clip.show(B);
  assert.equal(w.video.getAttribute('src'), B);
  assert.deepEqual(w.fetcher.calls, []);
});

test('onSet runs when the source is set: at once for a scheme address, after the bytes for a blob', async () => {
  const w = start();
  const events = [];
  w.clip.show(A, { onSet: () => events.push(['set', w.video.getAttribute('src')]) });
  assert.deepEqual(events, [['set', A]]);
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  assert.equal(events.length, 1, 'not yet: the bytes are still coming');
  w.fetcher.answer(A);
  await settle();
  assert.deepEqual(events[1], ['set', w.urls.created[0].url]);
  assert.equal(events.length, 2);
});

// ---- revoking ----

test('the previous object URL is revoked when a new one replaces it, not before', async () => {
  const w = start();
  await switchToBlobs(w);
  const first = w.urls.created[0].url;
  w.clip.show(B);
  assert.deepEqual(w.urls.revoked, [], 'the element still plays the first one while the next is fetched');
  w.fetcher.answer(B);
  await settle();
  assert.deepEqual(w.urls.revoked, [first]);
  assert.deepEqual(w.urls.live(), [w.urls.created[1].url]);
});

test('the object URL is revoked when the element is cleared, and the source is removed', async () => {
  const w = start();
  await switchToBlobs(w);
  w.clip.clear();
  assert.deepEqual(w.urls.revoked, [w.urls.created[0].url]);
  assert.equal(w.video.getAttribute('src'), null);
  assert.equal(w.video.loads, 1);
  assert.ok(w.video.pauses >= 1);
});

test('an object URL is revoked when the page goes away', async () => {
  const w = start(2);
  await switchToBlobs(w);
  w.clips[1].show(B);
  w.fetcher.answer(B);
  await settle();
  assert.equal(w.urls.live().length, 2);
  w.sources.dispose();
  assert.deepEqual(w.urls.live(), []);
});

test('a scheme address is never passed to revokeObjectURL', () => {
  const w = start();
  w.clip.show(A);
  w.clip.show(B);
  assert.equal(w.video.getAttribute('src'), B, 'the second address replaced the first');
  w.clip.clear();
  w.clip.show(C);
  assert.equal(w.video.getAttribute('src'), C);
  w.sources.dispose();
  assert.deepEqual(w.urls.revoked, []);
});

test('an element never holds more than one object URL', async () => {
  const w = start();
  await switchToBlobs(w);
  for (const address of [B, C, A]) {
    w.clip.show(address);
    w.fetcher.answer(address);
    await settle();
    assert.equal(w.urls.live().length, 1, address);
  }
  assert.equal(w.urls.created.length, 4);
});

test('clearing an element twice revokes its object URL once', async () => {
  const w = start();
  await switchToBlobs(w);
  w.clip.clear();
  assert.deepEqual(w.urls.live(), []);
  w.clip.clear();
  assert.equal(w.urls.revoked.length, 1, 'clearing twice revokes once');
});

// ---- stale answers ----

test('a fetch that finishes after a newer clip was set makes no object URL', async () => {
  const w = start();
  await switchToBlobs(w);
  w.clip.show(B);
  w.clip.show(C);
  w.fetcher.answer(B);
  await settle();
  assert.equal(w.urls.created.length, 1, 'only the first clip so far: B was stale');
  w.fetcher.answer(C);
  await settle();
  assert.equal(w.urls.created.length, 2);
  assert.deepEqual(w.urls.created[1].blob, { clip: C, call: 3 });
  assert.equal(w.video.getAttribute('src'), w.urls.created[1].url);
});

test('a fetch that finishes after the element was cleared makes no object URL', async () => {
  const w = start();
  w.clip.show(A);
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  w.clip.clear();
  w.fetcher.answer(A);
  await settle();
  assert.deepEqual(w.urls.created, []);
  assert.equal(w.video.getAttribute('src'), null);
});

test('a fetch whose bytes are read after the element moved on makes no object URL', async () => {
  const w = start();
  w.clip.show(A);
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  w.fetcher.answer(A);
  w.clip.show(B);
  await settle();
  assert.deepEqual(w.urls.created, []);
  w.fetcher.answer(B);
  await settle();
  assert.equal(w.urls.created.length, 1);
  assert.equal(w.urls.created[0].blob.clip, B);
});

test('a stale failure is not reported', async () => {
  const w = start();
  await switchToBlobs(w);
  const failures = [];
  w.clip.show(B, { onFailed: (reason) => failures.push(reason) });
  w.clip.show(C);
  w.fetcher.fail(B, new Error('late'));
  await settle();
  assert.deepEqual(failures, []);
});

test('a fetch that finishes for the other element does not touch this one', async () => {
  const w = start(2);
  await switchToBlobs(w);
  w.clips[1].show(B);
  w.clips[0].show(C);
  w.fetcher.answer(B);
  await settle();
  assert.equal(w.videos[1].getAttribute('src'), w.urls.created[1].url, 'B is current for the second element');
  assert.equal(w.urls.created[1].blob.clip, B);
});

// ---- failed fetches ----

test('a fetch that is refused sets no source and reports the failure', async () => {
  const w = start();
  const failures = [];
  const sets = [];
  w.clip.show(A, { onSet: () => sets.push(1), onFailed: (reason) => failures.push(reason) });
  sets.length = 0;
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  w.fetcher.fail(A, new TypeError('Failed to fetch'));
  await settle();
  assert.equal(failures.length, 1);
  assert.match(failures[0], /Failed to fetch/);
  assert.deepEqual(w.urls.created, []);
  assert.equal(w.video.getAttribute('src'), A, 'no blob source was set');
  assert.deepEqual(sets, []);
});

test('an answer that is not 2xx sets no source and reports its status', async () => {
  for (const status of [404, 500]) {
    const w = start();
    const failures = [];
    w.clip.show(A, { onFailed: (reason) => failures.push(reason) });
    fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
    w.fetcher.answer(A, { status });
    await settle();
    assert.equal(failures.length, 1, String(status));
    assert.match(failures[0], new RegExp(String(status)));
    assert.deepEqual(w.urls.created, [], String(status));
    assert.equal(w.video.getAttribute('src'), A, String(status));
  }
});

test('bytes that cannot be read set no source and report the failure', async () => {
  const w = start();
  const failures = [];
  w.clip.show(A, { onFailed: (reason) => failures.push(reason) });
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  w.fetcher.answer(A, { blobError: new Error('stream broke') });
  await settle();
  assert.equal(failures.length, 1);
  assert.match(failures[0], /stream broke/);
  assert.deepEqual(w.urls.created, []);
});

test('a failed fetch on the blob path leaves the previous object URL playing and unrevoked', async () => {
  const w = start();
  await switchToBlobs(w);
  const first = w.urls.created[0].url;
  w.clip.show(B, { onFailed: () => {} });
  w.fetcher.answer(B, { status: 500 });
  await settle();
  assert.deepEqual(w.urls.revoked, []);
  assert.equal(w.video.getAttribute('src'), first);
});

test('a successful fetch reports no failure', async () => {
  const w = start();
  const failures = [];
  w.clip.show(A, { onFailed: (reason) => failures.push(reason) });
  fail(w.video, MEDIA_ERR_SRC_NOT_SUPPORTED);
  w.fetcher.answer(A);
  await settle();
  assert.deepEqual(failures, []);
  assert.equal(w.urls.created.length, 1);
});
