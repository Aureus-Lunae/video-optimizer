import test from 'node:test';
import assert from 'node:assert/strict';

import { canSave, controlsDisabled, initialState, reduce, sampleRequest } from '../../ui/state.js';

// ---- helpers ----

/** Freezes `value` all the way down, so a reducer that changes its input throws. */
function deepFreeze(value) {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const inner of Object.values(value)) {
      deepFreeze(inner);
    }
  }
  return value;
}

/** Applies the actions in order to a frozen state, and freezes each result. */
function run(state, ...actions) {
  return actions.reduce((current, action) => deepFreeze(reduce(deepFreeze(current), action)), state);
}

/** The backend's `video` payload for Showreel V1.mp4, as an action. */
function videoAction(overrides = {}) {
  return {
    type: 'video',
    video: 3,
    name: 'Showreel V1.mp4',
    width: 3840,
    height: 2160,
    fps: 24,
    duration: 36.04,
    bytes: 96_900_000,
    audio: { codec: 'aac', bitrate: 317_000 },
    displayCopy: false,
    ...overrides,
  };
}

/** The backend's `sample` payload, as an action. */
function sampleAction(overrides = {}) {
  return {
    type: 'sample',
    video: 3,
    request: 1,
    crf: 28,
    originalBytes: 26_916_667,
    sampleBytes: 11_300_000,
    smallerBy: 58,
    wholeBytes: 40_680_000,
    psnr: 42.41,
    identical: false,
    ...overrides,
  };
}

/** A video that is loaded but whose before clip is not ready. */
function loaded(overrides) {
  return run(initialState(), videoAction(overrides));
}

/** A video with its before clip ready: a sample is being made. */
function ready() {
  return run(loaded(), { type: 'before-ready', video: 3 });
}

/** A video whose sample has arrived. */
function sampled() {
  const state = ready();
  return run(state, sampleAction({ request: state.request }));
}

/** A running save, started from `state`. */
function saving(state = sampled()) {
  return run(state, { type: 'save-started' });
}

// ---- the initial state ----

test('the window opens with CRF 28, Audio Keep, next to the original, column open and Before / after', () => {
  assert.deepEqual(initialState(), {
    video: null,
    crf: 28,
    audio: 'keep',
    folder: null,
    view: 'before-after',
    columnOpen: true,
    fill: false,
    zoom: 'fit',
    request: 0,
    sample: null,
    sampling: false,
    saving: null,
    status: '',
    keepStatus: false,
  });
});

test('each call gives a state of its own', () => {
  const first = initialState();
  const second = initialState();
  assert.notEqual(first, second);
  assert.deepEqual(first, second);
});

test('an action that is not known, or not an action, changes nothing', () => {
  const state = sampled();
  for (const action of [{ type: 'nothing' }, { type: 'reset-everything', crf: 20 }, {}, null, undefined, 'crf', 7, []]) {
    assert.equal(reduce(state, action), state, JSON.stringify(action) ?? String(action));
  }
  // Control: a known action does change it.
  assert.notEqual(reduce(state, { type: 'crf', value: 30 }), state);
});

// ---- video ----

test('a video event stores the video, and asks nothing before the before clip is ready', () => {
  const state = loaded();
  assert.equal(state.video.video, 3);
  assert.equal(state.video.name, 'Showreel V1.mp4');
  assert.equal(state.video.bytes, 96_900_000);
  assert.deepEqual(state.video.audio, { codec: 'aac', bitrate: 317_000 });
  assert.equal(state.video.beforeReady, false);
  assert.equal(state.video.type, undefined);
  assert.equal(state.sampling, false);
  assert.equal(sampleRequest(state), null);
});

test('a video event says Making the sample… and, for a display copy, Preparing the original…', () => {
  assert.equal(loaded().status, 'Making the sample…');
  assert.equal(loaded({ displayCopy: true }).status, 'Preparing the original…');
  // Only the flag decides: a missing one is not a display copy.
  assert.equal(loaded({ displayCopy: undefined }).status, 'Making the sample…');
});

test('a video event resets the sample and keeps crf, audio, folder, view and the column', () => {
  let state = sampled();
  state = run(
    state,
    { type: 'crf', value: 31 },
    { type: 'audio', value: 'remove' },
    { type: 'folder', folder: 'Exports' },
    { type: 'view', view: 'side-by-side' },
    { type: 'column', open: false },
    { type: 'zoom', zoom: 'actual' },
  );
  const before = state.request;
  const next = run(state, videoAction({ video: 4, name: 'Other.mov' }));
  assert.equal(next.video.video, 4);
  assert.equal(next.video.name, 'Other.mov');
  assert.equal(next.sample, null);
  assert.equal(next.sampling, false);
  assert.equal(next.crf, 31);
  assert.equal(next.audio, 'remove');
  assert.equal(next.folder, 'Exports');
  assert.equal(next.view, 'side-by-side');
  assert.equal(next.columnOpen, false);
  // The new picture starts at Fit.
  assert.equal(next.zoom, 'fit');
  // A sample of the new video is a new request, above everything asked before.
  assert.ok(next.request > before, `${next.request} > ${before}`);
});

test('a video event without a usable id is ignored', () => {
  const state = sampled();
  for (const id of ['3', -1, 1.5, NaN, null, undefined, [3]]) {
    assert.equal(reduce(state, videoAction({ video: id })), state, String(JSON.stringify(id)));
  }
  // Control: id 0 is a real id.
  assert.equal(run(initialState(), videoAction({ video: 0 })).video.video, 0);
});

// ---- before-ready and sampleRequest ----

test('before-ready of the current video starts the sample', () => {
  const state = ready();
  assert.equal(state.video.beforeReady, true);
  assert.equal(state.sampling, true);
  assert.equal(state.status, 'Making the sample…');
});

test('before-ready of another video, or with no video, is ignored', () => {
  const state = loaded();
  assert.equal(reduce(state, { type: 'before-ready', video: 2 }), state);
  assert.equal(reduce(state, { type: 'before-ready', video: '3' }), state);
  assert.equal(reduce(state, { type: 'before-ready' }), state);
  const empty = initialState();
  assert.equal(reduce(empty, { type: 'before-ready', video: 3 }), empty);
});

test('a before-ready that comes twice asks nothing the second time', () => {
  const state = sampled();
  assert.equal(reduce(state, { type: 'before-ready', video: 3 }), state);
  // It would otherwise await a sample for a request that was answered.
  assert.equal(state.sampling, false);
});

test('before-ready during a save marks the clip ready and starts no sample', () => {
  const state = run(loaded(), { type: 'save-started' }, { type: 'before-ready', video: 3 });
  assert.equal(state.video.beforeReady, true);
  assert.equal(state.sampling, false);
  assert.equal(state.status, 'Saving…');
  assert.equal(sampleRequest(state), null);
});

test('sampleRequest gives video, request, crf and audio once the before clip is ready', () => {
  const state = ready();
  assert.deepEqual(sampleRequest(state), { video: 3, request: state.request, crf: 28, audio: 'keep' });
  const changed = run(state, { type: 'crf', value: 31 }, { type: 'audio', value: 'remove' });
  assert.deepEqual(sampleRequest(changed), { video: 3, request: changed.request, crf: 31, audio: 'remove' });
});

test('sampleRequest is null without a video, before before-ready and while saving', () => {
  assert.equal(sampleRequest(initialState()), null);
  assert.equal(sampleRequest(loaded()), null);
  assert.equal(sampleRequest(saving(ready())), null);
  assert.equal(sampleRequest(saving()), null);
  // Control: the same video is asked once it is ready and the save is over.
  assert.notEqual(sampleRequest(ready()), null);
});

// ---- crf and audio ----

test('a crf action stores the value and asks a new sample', () => {
  const state = ready();
  const next = run(state, { type: 'crf', value: 31 });
  assert.equal(next.crf, 31);
  assert.equal(next.request, state.request + 1);
  assert.equal(next.sampling, true);
  assert.equal(next.status, 'Making the sample…');
  assert.equal(sampleRequest(next).crf, 31);
});

test('a crf action accepts the ends of the range, 20 and 36', () => {
  assert.equal(run(ready(), { type: 'crf', value: 20 }).crf, 20);
  assert.equal(run(ready(), { type: 'crf', value: 36 }).crf, 36);
});

test('a crf action with a value that is not a whole number from 20 to 36 changes nothing', () => {
  const state = ready();
  for (const value of [37, 19, 28.5, 0, 51, -1, '28', NaN, [28], null, undefined, Infinity, {}]) {
    assert.equal(reduce(state, { type: 'crf', value }), state, String(JSON.stringify(value) ?? value));
  }
});

test('a crf action with the value already set asks no new sample', () => {
  const state = ready();
  assert.equal(reduce(state, { type: 'crf', value: 28 }), state);
});

test('a crf action while saving changes nothing', () => {
  const state = saving();
  assert.equal(reduce(state, { type: 'crf', value: 31 }), state);
});

test('a crf action before the before clip is ready stores the value and starts no sample', () => {
  const state = loaded();
  const next = run(state, { type: 'crf', value: 31 });
  assert.equal(next.crf, 31);
  assert.equal(next.request, state.request + 1);
  assert.equal(next.sampling, false);
  assert.equal(next.status, state.status);
  assert.equal(sampleRequest(next), null);
  // And without a video at all.
  const empty = run(initialState(), { type: 'crf', value: 31 });
  assert.equal(empty.crf, 31);
  assert.equal(empty.sampling, false);
  assert.equal(empty.status, '');
});

test('each crf change gives a request number above the one before', () => {
  let state = ready();
  let last = state.request;
  for (const value of [29, 30, 20, 36, 28]) {
    state = run(state, { type: 'crf', value });
    assert.ok(state.request > last, `${state.request} > ${last}`);
    last = state.request;
  }
});

test('an audio action stores the choice and asks a new sample', () => {
  const state = ready();
  const next = run(state, { type: 'audio', value: 'remove' });
  assert.equal(next.audio, 'remove');
  assert.equal(next.request, state.request + 1);
  assert.equal(next.sampling, true);
  assert.equal(run(next, { type: 'audio', value: 'keep' }).audio, 'keep');
});

test('an audio action with another word, the same choice, or while saving changes nothing', () => {
  const state = ready();
  for (const value of ['mute', 'Keep', 'REMOVE', '', null, undefined, true, ['keep']]) {
    assert.equal(reduce(state, { type: 'audio', value }), state, String(JSON.stringify(value)));
  }
  assert.equal(reduce(state, { type: 'audio', value: 'keep' }), state);
  const busy = saving();
  assert.equal(reduce(busy, { type: 'audio', value: 'remove' }), busy);
});

// ---- sample ----

test('the current sample is stored and the status says Sample ready…', () => {
  const state = ready();
  const next = run(state, sampleAction({ request: state.request }));
  assert.equal(next.sample.request, state.request);
  assert.equal(next.sample.video, 3);
  assert.equal(next.sample.crf, 28);
  assert.equal(next.sample.sampleBytes, 11_300_000);
  assert.equal(next.sample.originalBytes, 26_916_667);
  assert.equal(next.sample.smallerBy, 58);
  assert.equal(next.sample.wholeBytes, 40_680_000);
  assert.equal(next.sample.psnr, 42.41);
  assert.equal(next.sample.identical, false);
  assert.equal(next.sample.type, undefined);
  assert.equal(next.sampling, false);
  assert.equal(next.status, 'Sample ready. Save encodes the whole video.');
});

test('a stale sample event is ignored', () => {
  const state = run(ready(), { type: 'crf', value: 29 }, { type: 'crf', value: 30 });
  const current = state.request;
  assert.ok(current >= 2);
  // The earlier request, which a newer one replaced.
  assert.equal(reduce(state, sampleAction({ request: current - 1 })), state);
  assert.equal(reduce(state, sampleAction({ request: 0 })), state);
  // A request that was never asked.
  assert.equal(reduce(state, sampleAction({ request: current + 1 })), state);
  // Control: the current one is taken.
  assert.notEqual(reduce(state, sampleAction({ request: current })), state);
});

test('a sample event of another video is ignored', () => {
  const state = ready();
  assert.equal(reduce(state, sampleAction({ video: 2, request: state.request })), state);
  assert.equal(reduce(state, sampleAction({ video: '3', request: state.request })), state);
  const empty = initialState();
  assert.equal(reduce(empty, sampleAction({ video: 3, request: 0 })), empty);
});

test('a sample that arrives after a new video was loaded is ignored', () => {
  const state = ready();
  const staleRequest = state.request;
  const next = run(state, videoAction({ video: 4 }), { type: 'before-ready', video: 4 });
  assert.equal(reduce(next, sampleAction({ video: 3, request: staleRequest })), next);
});

test('a sample that arrives after a save started is ignored', () => {
  const asked = ready();
  const busy = run(asked, { type: 'save-started' });
  assert.equal(reduce(busy, sampleAction({ request: asked.request })), busy);
});

test('a sample event that comes twice, or when none was awaited, is ignored the second time', () => {
  const state = sampled();
  assert.equal(state.sampling, false);
  assert.equal(reduce(state, sampleAction({ request: state.request, sampleBytes: 1 })), state);
  assert.equal(reduce(state, { type: 'sample-failed', video: 3, request: state.request, reason: 'x' }), state);
  // Nothing was asked of a video whose before clip is not ready.
  const waiting = loaded();
  assert.equal(reduce(waiting, sampleAction({ request: waiting.request })), waiting);
});

test('a new sample replaces the one before it', () => {
  const first = sampled();
  const asked = run(first, { type: 'crf', value: 31 });
  // The old sample stays until the new one is there.
  assert.equal(asked.sample.crf, 28);
  const next = run(asked, sampleAction({ request: asked.request, crf: 31, sampleBytes: 9_000_000 }));
  assert.equal(next.sample.crf, 31);
  assert.equal(next.sample.sampleBytes, 9_000_000);
});

test('a failed sample says why, keeps the sample before it and stops the wait', () => {
  const first = sampled();
  const asked = run(first, { type: 'crf', value: 31 });
  const next = run(asked, { type: 'sample-failed', video: 3, request: asked.request, reason: 'No space left on device' });
  assert.equal(next.status, 'The sample failed: No space left on device');
  assert.equal(next.sampling, false);
  assert.deepEqual(next.sample, first.sample);
});

test('a failed sample of an older request or another video is ignored', () => {
  const state = run(ready(), { type: 'crf', value: 31 });
  assert.equal(reduce(state, { type: 'sample-failed', video: 3, request: state.request - 1, reason: 'x' }), state);
  assert.equal(reduce(state, { type: 'sample-failed', video: 2, request: state.request, reason: 'x' }), state);
  assert.equal(reduce(state, { type: 'sample-failed', video: 3, request: state.request + 1, reason: 'x' }), state);
});

// ---- notices ----

test('several-files gives "One video at a time"', () => {
  const next = run(sampled(), { type: 'notice', kind: 'several-files' });
  assert.equal(next.status, 'One video at a time');
});

test('a single video event does not show "One video at a time"', () => {
  const state = run(sampled(), { type: 'notice', kind: 'several-files' });
  const next = run(state, videoAction({ video: 4 }));
  assert.notEqual(next.status, 'One video at a time');
  assert.equal(next.status, 'Making the sample…');
});

test('a notice saving says to wait for the save, or cancel it', () => {
  const next = run(saving(), { type: 'notice', kind: 'saving' });
  assert.equal(next.status, 'Wait for the save to finish, or cancel it');
});

test('a notice refused names the file and the reason, and keeps the current video and its sample', () => {
  const state = sampled();
  const next = run(state, { type: 'notice', kind: 'refused', name: 'notes.mp4', reason: 'Invalid data found when processing input' });
  assert.equal(next.status, 'notes.mp4 cannot be used: Invalid data found when processing input');
  assert.deepEqual(next.video, state.video);
  assert.deepEqual(next.sample, state.sample);
  assert.equal(next.request, state.request);
  assert.equal(next.crf, state.crf);
});

test('a notice too-large names the file and the limit, and keeps the current video', () => {
  const state = sampled();
  const next = run(state, { type: 'notice', kind: 'too-large', name: 'huge.mov' });
  assert.equal(next.status, 'huge.mov is larger than 10 GiB');
  assert.deepEqual(next.video, state.video);
  assert.deepEqual(next.sample, state.sample);
});

test('a notice failed while the before clip is not ready says so and ends the wait for a sample', () => {
  const state = run(loaded(), { type: 'crf', value: 30 });
  assert.equal(state.video.beforeReady, false);
  const next = run(state, { type: 'notice', kind: 'failed', reason: 'ffmpeg failed: x' });
  assert.equal(next.status, 'The original could not be prepared: ffmpeg failed: x');
  assert.equal(next.sampling, false);
  assert.equal(sampleRequest(next), null);
  assert.equal(next.video.name, 'Showreel V1.mp4');
});

test('a notice failed while the before clip is ready only changes the status: the sample in flight still counts', () => {
  // The backend also sends `failed` when it could make no work folder for a
  // new video; the current video, its before clip and its sample are fine.
  const state = ready();
  assert.equal(state.video.beforeReady, true);
  assert.equal(state.sampling, true);
  const next = run(state, { type: 'notice', kind: 'failed', reason: 'no work folder' });
  assert.equal(next.status, 'The original could not be prepared: no work folder');
  assert.equal(next.sampling, true);
  assert.equal(next.request, state.request);
  assert.deepEqual(next.video, state.video);
  const done = run(next, sampleAction({ request: next.request }));
  assert.equal(done.sample.request, state.request);
  assert.equal(done.status, 'Sample ready. Save encodes the whole video.');
});

test('a notice of another kind, or without text, does not break the status', () => {
  const state = sampled();
  assert.equal(reduce(state, { type: 'notice', kind: 'surprise' }), state);
  assert.equal(reduce(state, { type: 'notice' }), state);
  // A name or reason that is not text is left out, never printed as "undefined".
  const refused = run(state, { type: 'notice', kind: 'refused', name: undefined, reason: null });
  assert.doesNotMatch(refused.status, /undefined|null/);
});

// ---- saving ----

test('a save started stops the sample, disables the controls and bumps the request', () => {
  const state = ready();
  const next = run(state, { type: 'save-started' });
  assert.notEqual(next.saving, null);
  assert.equal(next.sampling, false);
  assert.equal(next.status, 'Saving…');
  assert.ok(next.request > state.request, 'a late event of the cancelled sample is stale');
  assert.equal(controlsDisabled(next), true);
  assert.equal(canSave(next), false);
});

test('a save cannot start without a video, or while another runs', () => {
  const empty = initialState();
  assert.equal(reduce(empty, { type: 'save-started' }), empty);
  const busy = saving();
  assert.equal(reduce(busy, { type: 'save-started' }), busy);
});

test('save-progress 42 gives "Saving… 42%"', () => {
  const next = run(saving(), { type: 'save-progress', percent: 42 });
  assert.equal(next.status, 'Saving… 42%');
  assert.equal(run(saving(), { type: 'save-progress', percent: 0 }).status, 'Saving… 0%');
  assert.equal(run(saving(), { type: 'save-progress', percent: 100 }).status, 'Saving… 100%');
});

test('save-progress null gives "Saving…", never NaN', () => {
  const state = run(saving(), { type: 'save-progress', percent: 42 });
  const next = run(state, { type: 'save-progress', percent: null });
  assert.equal(next.status, 'Saving…');
  assert.doesNotMatch(next.status, /NaN/);
});

test('save-progress with a percent above 100, below 0, a fraction or not a number is shown as "Saving…"', () => {
  for (const percent of [101, -1, 42.5, 'x', '42', NaN, Infinity, undefined, [42], {}]) {
    const state = run(saving(), { type: 'save-progress', percent: 42 });
    const next = run(state, { type: 'save-progress', percent });
    assert.equal(next.status, 'Saving…', String(JSON.stringify(percent) ?? percent));
    assert.equal(next.saving.percent, null);
  }
});

test('save-progress when no save runs is ignored', () => {
  for (const state of [sampled(), run(saving(), { type: 'save-cancelled' })]) {
    assert.equal(reduce(state, { type: 'save-progress', percent: 42 }), state);
  }
});

test('save-done gives the name, the size and how much smaller', () => {
  const next = run(saving(), {
    type: 'save-done',
    name: 'optimized-Showreel V1.mp4',
    bytes: 40_800_000,
    inputBytes: 96_900_000,
    keptOriginal: false,
  });
  assert.equal(next.status, 'Saved optimized-Showreel V1.mp4 (40.8 MB, 58% smaller)');
  assert.equal(next.saving, null);
});

test('save-done with the original kept says the result was not smaller', () => {
  const next = run(saving(), {
    type: 'save-done',
    name: 'optimized-Showreel V1.mov',
    bytes: 96_900_000,
    inputBytes: 96_900_000,
    keptOriginal: true,
  });
  assert.equal(next.status, 'Saved optimized-Showreel V1.mov: original kept: result was not smaller');
  assert.doesNotMatch(next.status, /smaller\)/);
});

test('save-done without a usable original size gives the size and no percentage', () => {
  for (const inputBytes of [0, null, undefined, NaN, '96900000']) {
    const next = run(saving(), { type: 'save-done', name: 'o.mp4', bytes: 40_800_000, inputBytes, keptOriginal: false });
    assert.equal(next.status, 'Saved o.mp4 (40.8 MB)', String(inputBytes));
  }
  // A result that is not smaller does not claim to be.
  const bigger = run(saving(), { type: 'save-done', name: 'o.mp4', bytes: 50, inputBytes: 40, keptOriginal: false });
  assert.equal(bigger.status, 'Saved o.mp4 (0.0 MB)');
});

test('save-failed gives the reason as it comes, with no path', () => {
  const next = run(saving(), { type: 'save-failed', reason: 'No space left on device' });
  assert.equal(next.status, 'Save failed: No space left on device');
  assert.equal(next.saving, null);
});

test('save-cancelled gives "Save cancelled"', () => {
  const next = run(saving(), { type: 'save-cancelled' });
  assert.equal(next.status, 'Save cancelled');
  assert.equal(next.saving, null);
});

test('controlsDisabled while saving, enabled after save-done, save-failed and save-cancelled', () => {
  assert.equal(controlsDisabled(saving()), true);
  assert.equal(controlsDisabled(initialState()), false);
  assert.equal(controlsDisabled(sampled()), false);
  const ends = [
    { type: 'save-done', name: 'o.mp4', bytes: 1, inputBytes: 2, keptOriginal: false },
    { type: 'save-failed', reason: 'x' },
    { type: 'save-cancelled' },
  ];
  for (const end of ends) {
    const next = run(saving(), end);
    assert.equal(controlsDisabled(next), false, end.type);
    assert.equal(canSave(next), true, end.type);
  }
});

test('canSave needs a video and no save running', () => {
  assert.equal(canSave(initialState()), false);
  assert.equal(canSave(loaded()), true);
  assert.equal(canSave(sampled()), true);
  assert.equal(canSave(saving()), false);
});

test('the sample made before a save is kept after it', () => {
  const state = sampled();
  const next = run(state, { type: 'save-started' }, { type: 'save-failed', reason: 'x' });
  assert.deepEqual(next.sample, state.sample);
  assert.equal(next.sampling, false);
});

test('a sample cut short by a save is asked again when the save ends', () => {
  const asked = ready();
  const busy = run(asked, { type: 'save-started' });
  assert.equal(busy.sampling, false);
  for (const end of [{ type: 'save-failed', reason: 'x' }, { type: 'save-cancelled' }]) {
    const next = run(busy, end);
    assert.equal(next.sampling, true, end.type);
    assert.ok(next.request > busy.request, end.type);
    assert.deepEqual(sampleRequest(next), { video: 3, request: next.request, crf: 28, audio: 'keep' });
  }
  // The save's own message is not replaced by the sample's.
  assert.equal(run(busy, { type: 'save-cancelled' }).status, 'Save cancelled');
});

/** The three ends of a save, each with the status it gives. */
const SAVE_ENDS = [
  [{ type: 'save-done', name: 'optimized-a.mp4', bytes: 5_800_000, inputBytes: 68_300_000, keptOriginal: false }, 'Saved optimized-a.mp4 (5.8 MB, 92% smaller)'],
  [{ type: 'save-failed', reason: 'disk full' }, 'Save failed: disk full'],
  [{ type: 'save-cancelled' }, 'Save cancelled'],
];

test('a sample cut short by a save keeps the save message on the status line when it arrives', () => {
  const busy = run(ready(), { type: 'save-started' });
  for (const [end, message] of SAVE_ENDS) {
    const ended = run(busy, end);
    assert.equal(ended.status, message, end.type);
    assert.equal(ended.sampling, true, end.type);
    const arrived = run(ended, sampleAction({ request: ended.request, smallerBy: 61 }));
    // The sample counts: it is stored and the wait is over.
    assert.equal(arrived.sampling, false, end.type);
    assert.ok(arrived.sample !== null && arrived.sample.smallerBy === 61, end.type);
    assert.equal(arrived.status, message, end.type);
    // A sample that fails says so: that is news, not a repeat.
    const failed = run(ended, { type: 'sample-failed', video: 3, request: ended.request, reason: 'x' });
    assert.equal(failed.status, 'The sample failed: x', end.type);
  }
});

test('a before clip that became ready after a save keeps the save message through its sample', () => {
  const busy = run(loaded(), { type: 'save-started' });
  for (const [end, message] of SAVE_ENDS) {
    const asked = run(busy, end, { type: 'before-ready', video: 3 });
    assert.equal(asked.sampling, true, end.type);
    assert.equal(asked.status, message, end.type);
    const arrived = run(asked, sampleAction({ request: asked.request }));
    assert.equal(arrived.sampling, false, end.type);
    assert.equal(arrived.status, message, end.type);
  }
});

test('a sample asked by a quality or audio change after a save says Sample ready…', () => {
  const busy = run(ready(), { type: 'save-started' });
  for (const [end, message] of SAVE_ENDS) {
    for (const change of [{ type: 'crf', value: 31 }, { type: 'audio', value: 'remove' }]) {
      const asked = run(busy, end, change);
      assert.equal(asked.status, 'Making the sample…', `${end.type} ${change.type}`);
      const arrived = run(asked, sampleAction({ request: asked.request }));
      assert.notEqual(arrived.status, message, `${end.type} ${change.type}`);
      assert.equal(arrived.status, 'Sample ready. Save encodes the whole video.', `${end.type} ${change.type}`);
    }
  }
  // A new video after a save starts afresh too.
  const after = run(busy, SAVE_ENDS[0][0], videoAction({ video: 4 }), { type: 'before-ready', video: 4 });
  assert.equal(after.status, 'Making the sample…');
  const arrived = run(after, sampleAction({ video: 4, request: after.request }));
  assert.equal(arrived.status, 'Sample ready. Save encodes the whole video.');
});

test('a before clip that became ready during a save gets its sample when the save ends', () => {
  const busy = run(loaded(), { type: 'save-started' }, { type: 'before-ready', video: 3 });
  const next = run(busy, { type: 'save-failed', reason: 'x' });
  assert.equal(next.sampling, true);
  assert.deepEqual(sampleRequest(next), { video: 3, request: next.request, crf: 28, audio: 'keep' });
});

test('no sample is asked again when none was cut short, or the before clip is not ready', () => {
  const idle = saving(sampled());
  const next = run(idle, { type: 'save-cancelled' });
  assert.equal(next.sampling, false);
  assert.equal(next.request, idle.request);
  const waiting = run(loaded(), { type: 'save-started' }, { type: 'save-cancelled' });
  assert.equal(waiting.sampling, false);
  assert.equal(sampleRequest(waiting), null);
});

test('a crf change made while the before clip was not ready is asked once it is', () => {
  const state = run(loaded(), { type: 'crf', value: 31 }, { type: 'before-ready', video: 3 });
  assert.deepEqual(sampleRequest(state), { video: 3, request: state.request, crf: 31, audio: 'keep' });
});

// ---- the folder ----

test('a folder action stores the name the backend answers', () => {
  const next = run(initialState(), { type: 'folder', folder: 'Exports' });
  assert.equal(next.folder, 'Exports');
  assert.equal(run(next, { type: 'folder', folder: null }).folder, null);
});

test('a cancelled folder dialog keeps the folder shown, as the backend answers with the one still in effect', () => {
  const state = run(initialState(), { type: 'folder', folder: 'Exports' });
  assert.equal(run(state, { type: 'folder', folder: 'Exports' }).folder, 'Exports');
});

test('a folder that is not a name or null is ignored', () => {
  const state = run(initialState(), { type: 'folder', folder: 'Exports' });
  for (const folder of [undefined, 7, ['Exports'], {}, true]) {
    assert.equal(reduce(state, { type: 'folder', folder }), state, String(JSON.stringify(folder)));
  }
});

test('a folder action asks no sample', () => {
  const state = ready();
  const next = run(state, { type: 'folder', folder: 'Exports' });
  assert.equal(next.request, state.request);
  assert.equal(next.status, state.status);
});

// ---- view, column, fill and zoom ----

test('a view action stores either view and ignores any other', () => {
  const state = initialState();
  assert.equal(run(state, { type: 'view', view: 'side-by-side' }).view, 'side-by-side');
  assert.equal(run(state, { type: 'view', view: 'side-by-side' }, { type: 'view', view: 'before-after' }).view, 'before-after');
  for (const view of ['grid', '', null, undefined, 'Before-After', ['side-by-side']]) {
    assert.equal(reduce(state, { type: 'view', view }), state, String(JSON.stringify(view)));
  }
});

test('column false hides the column and keeps crf, audio and folder', () => {
  const state = run(
    sampled(),
    { type: 'crf', value: 31 },
    { type: 'audio', value: 'remove' },
    { type: 'folder', folder: 'Exports' },
  );
  const hidden = run(state, { type: 'column', open: false });
  assert.equal(hidden.columnOpen, false);
  assert.equal(hidden.crf, 31);
  assert.equal(hidden.audio, 'remove');
  assert.equal(hidden.folder, 'Exports');
  assert.deepEqual(hidden.video, state.video);
  assert.equal(hidden.request, state.request);
});

test('column true restores it, with crf, audio and folder as they were', () => {
  const state = run(sampled(), { type: 'crf', value: 31 }, { type: 'folder', folder: 'Exports' });
  const hidden = run(state, { type: 'column', open: false });
  const shown = run(hidden, { type: 'column', open: true });
  assert.deepEqual(shown, state);
});

test('a column action that is not true or false is ignored', () => {
  const state = initialState();
  for (const open of ['false', 0, 1, null, undefined, []]) {
    assert.equal(reduce(state, { type: 'column', open }), state, String(JSON.stringify(open)));
  }
});

test('a fill action stores true and false and ignores anything else', () => {
  const state = initialState();
  assert.equal(run(state, { type: 'fill', on: true }).fill, true);
  assert.equal(run(state, { type: 'fill', on: true }, { type: 'fill', on: false }).fill, false);
  for (const on of ['true', 1, null, undefined]) {
    assert.equal(reduce(state, { type: 'fill', on }), state, String(JSON.stringify(on)));
  }
});

test('a zoom action stores fit or actual and ignores any other', () => {
  const state = initialState();
  assert.equal(run(state, { type: 'zoom', zoom: 'actual' }).zoom, 'actual');
  assert.equal(run(state, { type: 'zoom', zoom: 'actual' }, { type: 'zoom', zoom: 'fit' }).zoom, 'fit');
  for (const zoom of ['200%', '', null, undefined, 1]) {
    assert.equal(reduce(state, { type: 'zoom', zoom }), state, String(JSON.stringify(zoom)));
  }
});

// ---- the reducer does not change what it is given ----

test('reduce never changes the state it is given', () => {
  // Every state and action here is frozen by `run`, so a write would throw.
  const actions = [
    videoAction(),
    { type: 'before-ready', video: 3 },
    { type: 'crf', value: 30 },
    { type: 'audio', value: 'remove' },
    { type: 'folder', folder: 'Exports' },
    { type: 'view', view: 'side-by-side' },
    { type: 'column', open: false },
    { type: 'fill', on: true },
    { type: 'zoom', zoom: 'actual' },
    { type: 'save-started' },
    { type: 'save-progress', percent: 10 },
    { type: 'save-done', name: 'o.mp4', bytes: 1, inputBytes: 2, keptOriginal: false },
  ];
  const state = run(initialState(), ...actions);
  assert.equal(state.crf, 30);
  assert.equal(state.saving, null);
});
