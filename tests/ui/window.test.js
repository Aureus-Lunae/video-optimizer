// The window itself: ui/window.js's start function, driven through the page
// built from ui/index.html, a fake Tauri, strict timers and a ResizeObserver
// (fake-window.js). Each test fires the events a person or the backend would
// and asserts what the window sends and what the page shows.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { startWindow } from '../../ui/window.js';
import { crfValueText, crfWord } from '../../ui/quality.js';
import { toActual, toFit, zoomLimits } from '../../ui/zoom.js';
import { fakePage, fakeResizeObservers, fakeTauri, settle, strictTimers } from './fake-window.js';

// ---- helpers ----

/** The size of the area each layer has (both, in Before / after). */
const AREA = { width: 800, height: 450 };
/** Where the picture sits in the window. */
const RECT = { left: 100, top: 50, width: 800, height: 450 };

/**
 * Starts the window on a fresh page built from index.html, with focus as
 * strict as a browser's; every fake is returned for the test.
 * `resizeObserver: false` starts it without the ResizeObserver class.
 */
function start({ refuse = {}, refuseListen = [], hold = [], answers = {}, resizeObserver = true } = {}) {
  const doc = fakePage();
  doc.strictFocus = true;
  for (const id of ['layerBefore', 'layerAfter']) {
    doc.byId(id).clientWidth = AREA.width;
    doc.byId(id).clientHeight = AREA.height;
  }
  doc.byId('picture').rect = { ...RECT };
  const tauri = fakeTauri({ refuse, refuseListen, hold, answers });
  const timers = strictTimers();
  const observers = fakeResizeObservers();
  const logged = [];
  const { listening } = startWindow({
    doc,
    tauri,
    timers,
    ResizeObserver: resizeObserver ? observers.ResizeObserver : undefined,
    log: (...values) => logged.push(values),
  });
  return { doc, el: doc.byId, tauri, timers, observers, logged, listening };
}

/** The backend's `video` payload for Showreel V1.mp4. */
const videoOf = (fields = {}) => ({
  video: 3,
  name: 'Showreel V1.mp4',
  width: 3840,
  height: 2160,
  fps: 24,
  duration: 36.04,
  bytes: 96_900_000,
  audio: { codec: 'aac', bitrate: 317_000 },
  displayCopy: false,
  ...fields,
});

/** The backend's `sample` payload for request `request` of video 3. */
const sampleOf = (request, fields = {}) => ({
  video: 3,
  request,
  crf: 28,
  originalBytes: 26_916_667,
  sampleBytes: 11_300_000,
  smallerBy: 58,
  wholeBytes: 40_680_000,
  psnr: 42.41,
  identical: false,
  ...fields,
});

/** Video 3 loaded; its before clip not ready yet. */
async function load(w, fields = {}) {
  await w.listening;
  w.tauri.emit('video', videoOf(fields));
  await settle();
}

/** Video 3 loaded and its before clip ready: sample request 1 is asked. */
async function ready(w, fields = {}) {
  await load(w, fields);
  w.tauri.emit('before-ready', { video: 3 });
  await settle();
}

/** `ready`, and the sample of request 1 arrived. */
async function sampled(w) {
  await ready(w);
  w.tauri.emit('sample', sampleOf(1));
  await settle();
}

/** The slider moved to `value`, as the browser reports a drag or a key: `input`, then `change`. */
function moveSlider(w, value) {
  w.el('crf').value = value;
  w.el('crf').fire('input');
  w.el('crf').fire('change');
}

/** Audio set to `value` ('keep' or 'remove'). */
function pickAudio(w, value) {
  w.el('audio').value = value;
  w.el('audio').fire('change');
}

const sent = (w, command) => w.tauri.calls.filter(([name]) => name === command);
const samplesAsked = (w) => sent(w, 'make_sample').map(([, args]) => args);

/** The transform both videos show; it throws unless both show the same one. */
function shownTransform(w) {
  const before = w.el('videoBefore').style.transform;
  const after = w.el('videoAfter').style.transform;
  assert.notEqual(before, '', 'a transform is written');
  assert.equal(before, after, 'both videos show the same view');
  return before;
}

const transformOf = (view) => `translate(${view.x}px, ${view.y}px) scale(${view.scale})`;

/** The controls a save turns off. */
const SAVE_LOCKED = ['crf', 'audio', 'chooseOther', 'chooseFolder'];

// ---- the page as index.html builds it ----

test('both videos load ahead, play inline and have no autoplay or controls', () => {
  const w = start();
  for (const id of ['videoBefore', 'videoAfter']) {
    const video = w.el(id);
    assert.equal(video.tagName, 'video', id);
    assert.equal(video.getAttribute('preload'), 'auto', id);
    assert.equal(video.getAttribute('playsinline'), '', id);
    assert.equal(video.getAttribute('autoplay'), null, id);
    assert.equal(video.getAttribute('controls'), null, id);
  }
});

test('the empty window shows the drop prompt, No video yet and a Save that sends nothing', async () => {
  const w = start();
  await w.listening;
  assert.equal(w.el('empty').hidden, false);
  assert.equal(w.el('emptyTitle').textContent, 'Drop a video here, or choose one');
  assert.equal(w.el('chooseVideo').textContent, 'Choose video');
  assert.equal(w.el('compare').hidden, true);
  for (const id of ['fileEmpty', 'resultEmpty']) {
    assert.equal(w.el(id).hidden, false, id);
    assert.equal(w.el(id).textContent, 'No video yet', id);
  }
  assert.equal(w.el('fileFigures').hidden, true);
  assert.equal(w.el('resultFigures').hidden, true);
  assert.equal(w.el('save').getAttribute('aria-disabled'), 'true');
  w.el('save').click();
  await settle();
  assert.deepEqual(sent(w, 'save'), []);
});

/** The name each input and select of `doc` is read out with: its aria-label, else its `<label for>`'s text ('' for none). */
function controlNames(doc) {
  const labels = doc.created.filter((element) => element.tagName === 'label');
  return doc.created
    .filter((element) => ['input', 'select'].includes(element.tagName))
    .map((control) => {
      const label = labels.find((element) => element.getAttribute('for') === control.id);
      return [control.id, control.getAttribute('aria-label') ?? (label === undefined ? '' : label.text)];
    });
}

test('every input and select of the page has its name', () => {
  const names = controlNames(start().doc);
  assert.deepEqual(names, [
    ['divider', 'Before and after divider'],
    ['position', 'Position in the 10 second sample'],
    ['crf', 'Quality (CRF)'],
    ['audio', 'Audio'],
  ]);
});

test('an input without a label is seen as nameless', () => {
  const names = controlNames(fakePage('<input id="x" type="range"><label for="y">Y</label><input id="y">'));
  assert.deepEqual(names, [
    ['x', ''],
    ['y', 'Y'],
  ]);
});

test('the page reader refuses markup it cannot read, so a broken page fails the tests', () => {
  assert.equal(fakePage('<div id="a"><span id="b">x</span></div>').byId('b').parent.id, 'a');
  assert.throws(() => fakePage('<div><span></div>'), /closes <span>/);
  assert.throws(() => fakePage('<div>'), /never closed/);
  assert.throws(() => fakePage('<p id="a"></p><p id="a"></p>'), /used twice/);
});

// ---- the quality slider ----

test('moving the slider updates the number, word and valuetext at once', async () => {
  const w = start();
  await ready(w);
  moveSlider(w, '32');
  assert.equal(w.el('crfValue').textContent, 'CRF 32');
  assert.equal(w.el('crfWord').textContent, crfWord(32));
  assert.notEqual(crfWord(32), crfWord(28));
  assert.equal(w.el('crf').getAttribute('aria-valuetext'), crfValueText(32));
  assert.equal(w.el('tagAfter').textContent, 'Sample, CRF 32');
});

test('the slider starts at CRF 28 with its word and valuetext', () => {
  const w = start();
  assert.equal(w.el('crf').value, '28');
  assert.equal(w.el('crfValue').textContent, 'CRF 28');
  assert.equal(w.el('crfWord').textContent, 'Recommended for the web');
  assert.equal(w.el('crf').getAttribute('aria-valuetext'), 'CRF 28, Recommended for the web');
  assert.equal(w.el('tagAfter').textContent, 'Sample, CRF 28');
});

test('it asks no sample until 400 ms after the last move', async () => {
  const w = start();
  await ready(w);
  assert.equal(samplesAsked(w).length, 1, 'before-ready asked request 1');
  moveSlider(w, '30');
  w.timers.advance(399);
  assert.equal(samplesAsked(w).length, 1, 'nothing before 400 ms');
  moveSlider(w, '31');
  w.timers.advance(399);
  assert.equal(samplesAsked(w).length, 1, 'the second move started the wait again');
  w.timers.advance(1);
  await settle();
  const asked = samplesAsked(w);
  assert.equal(asked.length, 2, 'two moves, one request');
  assert.deepEqual(asked[1], { video: 3, request: 3, crf: 31, audio: 'keep' });
  w.timers.advance(5000);
  assert.equal(samplesAsked(w).length, 2, 'nothing more after the wait');
});

test('the slider stops at 20 and 36', async () => {
  const w = start();
  await ready(w);
  const slider = w.el('crf');
  assert.equal(slider.getAttribute('min'), '20');
  assert.equal(slider.getAttribute('max'), '36');
  assert.equal(slider.getAttribute('step'), '1');
  const timersBefore = w.timers.set.length;
  moveSlider(w, '37');
  assert.equal(w.el('crfValue').textContent, 'CRF 28', '37 is read as no change');
  assert.equal(slider.value, '28', 'the slider is put back');
  assert.equal(w.timers.set.length, timersBefore, 'no wait started');
  w.timers.advance(400);
  assert.equal(samplesAsked(w).length, 1);
});

test('a value of 28 sends crf 28', async () => {
  const w = start();
  await ready(w);
  moveSlider(w, '30');
  moveSlider(w, '28');
  w.timers.advance(400);
  const last = samplesAsked(w).at(-1);
  assert.equal(last.crf, 28, 'a number, not the text "28"');
  assert.equal(last.request, 3);
});

test('Audio Remove asks a sample with audio remove after 400 ms', async () => {
  const w = start();
  await ready(w);
  pickAudio(w, 'remove');
  w.timers.advance(399);
  assert.equal(samplesAsked(w).length, 1);
  w.timers.advance(1);
  assert.deepEqual(samplesAsked(w).at(-1), { video: 3, request: 2, crf: 28, audio: 'remove' });
});

test('an Audio value that is not a choice asks nothing', async () => {
  const w = start();
  await ready(w);
  pickAudio(w, 'loud');
  w.timers.advance(400);
  assert.equal(samplesAsked(w).length, 1);
  assert.equal(w.el('audio').value, 'keep', 'the select is put back');
});

test('a refused make_sample shows that the sample failed', async () => {
  const w = start({ refuse: { make_sample: 'quality must be a whole number from 20 to 36' } });
  await ready(w);
  assert.equal(samplesAsked(w).length, 1);
  assert.equal(w.el('statusText').textContent, 'The sample failed: quality must be a whole number from 20 to 36');
});

test('an accepted make_sample leaves the status at Making the sample', async () => {
  const w = start();
  await ready(w);
  assert.equal(w.el('statusText').textContent, 'Making the sample…');
});

// ---- a video loads ----

test('a video event shows the File group and asks a sample after before-ready', async () => {
  const w = start();
  await load(w);
  assert.equal(w.el('fileEmpty').hidden, true);
  assert.equal(w.el('fileFigures').hidden, false);
  assert.equal(w.el('fileName').textContent, 'Showreel V1.mp4');
  assert.equal(w.el('fileSize').textContent, '3840 x 2160');
  assert.equal(w.el('fileFps').textContent, '24 fps');
  assert.equal(w.el('fileLength').textContent, '0:36');
  assert.equal(w.el('fileAudio').textContent, 'AAC, 317 kbps');
  assert.equal(w.el('empty').hidden, true);
  assert.equal(w.el('compare').hidden, false);
  assert.equal(w.el('save').getAttribute('aria-disabled'), 'false');
  w.tauri.emit('before-ready', { video: 3 });
  await settle();
  assert.deepEqual(samplesAsked(w), [{ video: 3, request: 1, crf: 28, audio: 'keep' }]);
  assert.equal(w.el('videoBefore').getAttribute('src'), 'media://localhost/3-before');
});

test('no sample is asked before before-ready', async () => {
  const w = start();
  await load(w);
  w.timers.advance(1000);
  w.tauri.emit('before-ready', { video: 4 });
  await settle();
  assert.deepEqual(samplesAsked(w), []);
  assert.equal(w.el('videoBefore').getAttribute('src'), null, 'no clip for another video');
});

test('a display copy is tagged Original (converted for display)', async () => {
  const w = start();
  await load(w, { displayCopy: true });
  assert.equal(w.el('tagBefore').textContent, 'Original (converted for display)');
  assert.equal(w.el('statusText').textContent, 'Preparing the original…');
});

test('an original the window can play is tagged Original', async () => {
  const w = start();
  await load(w);
  assert.equal(w.el('tagBefore').textContent, 'Original');
});

test('dropping two files shows "One video at a time"', async () => {
  const w = start();
  await w.listening;
  w.tauri.emit('notice', { kind: 'several-files' });
  assert.equal(w.el('statusText').textContent, 'One video at a time');
});

test('a single file dropped does not say "One video at a time"', async () => {
  const w = start();
  await load(w);
  assert.notEqual(w.el('statusText').textContent, 'One video at a time');
  assert.equal(w.el('statusText').textContent, 'Making the sample…');
});

test('a new video stops the old one and clears both clips', async () => {
  const w = start();
  await sampled(w);
  w.el('play').click();
  await settle();
  w.tauri.emit('video', videoOf({ video: 4, name: 'b.mp4' }));
  await settle();
  for (const id of ['videoBefore', 'videoAfter']) {
    const video = w.el(id);
    assert.equal(video.paused, true, id);
    assert.equal(video.getAttribute('src'), null, id);
    assert.ok(video.loads >= 1, id);
  }
  assert.equal(w.el('play').textContent, 'Play');
  assert.equal(w.el('fileName').textContent, 'b.mp4');
  assert.equal(w.el('resultEmpty').textContent, 'No sample yet');
});

test('Choose video and Choose other send choose_video', async () => {
  const w = start();
  await w.listening;
  w.el('chooseVideo').click();
  await load(w);
  w.el('chooseOther').click();
  await settle();
  assert.deepEqual(w.tauri.commands().filter((name) => name === 'choose_video'), ['choose_video', 'choose_video']);
});

test('Choose other sends nothing while a save runs', async () => {
  const w = start({ hold: ['save'] });
  await ready(w);
  w.el('save').click();
  w.el('chooseOther').click();
  w.el('chooseFolder').click();
  await settle();
  assert.deepEqual(sent(w, 'choose_video'), []);
  assert.deepEqual(sent(w, 'choose_folder'), []);
});

test('a refused choose_video is logged and changes nothing', async () => {
  const w = start({ refuse: { choose_video: 'not allowed' } });
  await w.listening;
  w.el('chooseVideo').click();
  await settle();
  assert.deepEqual(w.logged, [['choose_video', 'not allowed']]);
  assert.equal(w.el('empty').hidden, false);
});

test('a load with focus on Choose video moves focus to Choose other', async () => {
  const w = start();
  w.el('chooseVideo').focus();
  await load(w);
  assert.equal(w.doc.activeElement, w.el('chooseOther'));
});

test('a load with the column folded moves focus from Choose video to Play', async () => {
  const w = start();
  w.el('hideSettings').click();
  w.el('chooseVideo').focus();
  await load(w);
  assert.equal(w.doc.activeElement, w.el('play'));
});

test('a load with focus elsewhere leaves focus alone', async () => {
  const w = start();
  w.el('crf').focus();
  await load(w);
  assert.equal(w.doc.activeElement, w.el('crf'));
});

// ---- the sample ----

test('a new sample event shows the sample at the original time, playing when the original plays', async () => {
  const w = start();
  await ready(w);
  w.el('play').click();
  await settle();
  w.el('videoBefore').time = 2.5;
  w.tauri.emit('sample', sampleOf(1));
  await settle();
  const sample = w.el('videoAfter');
  assert.equal(sample.getAttribute('src'), 'media://localhost/3-sample-1');
  assert.equal(sample.currentTime, 2.5);
  assert.equal(sample.paused, false);
  assert.equal(sample.hidden, false);
});

test('a new sample while paused stays paused at the original time', async () => {
  const w = start();
  await ready(w);
  w.el('videoBefore').time = 4;
  w.tauri.emit('sample', sampleOf(1));
  await settle();
  assert.equal(w.el('videoAfter').currentTime, 4);
  assert.equal(w.el('videoAfter').plays, 0);
  assert.equal(w.el('videoAfter').paused, true);
});

test('a stale sample event changes nothing', async () => {
  const w = start();
  await ready(w);
  moveSlider(w, '30');
  w.tauri.emit('sample', sampleOf(1));
  await settle();
  assert.equal(w.el('videoAfter').getAttribute('src'), null);
  assert.equal(w.el('videoAfter').hidden, true, 'no sample is shown before one arrives');
  assert.equal(w.el('resultEmpty').hidden, false);
});

test('a sample shows its figures in the Result group', async () => {
  const w = start();
  await sampled(w);
  assert.equal(w.el('resultEmpty').hidden, true);
  assert.equal(w.el('resultFigures').hidden, false);
  assert.equal(w.el('resultOriginal').textContent, '26.9 MB');
  assert.equal(w.el('resultOptimized').textContent, '11.3 MB');
  assert.equal(w.el('resultSmaller').textContent, '58%');
  assert.equal(w.el('resultWhole').textContent, '40.7 MB');
  assert.equal(w.el('resultPsnr').textContent, '42.4 dB');
  assert.equal(w.el('statusText').textContent, 'Sample ready. Save encodes the whole video.');
});

test('a sample with unknown figures shows unknown, never NaN', async () => {
  const w = start();
  await ready(w);
  w.tauri.emit('sample', sampleOf(1, { originalBytes: null, smallerBy: null, wholeBytes: null, psnr: null }));
  await settle();
  for (const id of ['resultOriginal', 'resultSmaller', 'resultWhole', 'resultPsnr']) {
    assert.equal(w.el(id).textContent, 'unknown', id);
  }
});

// ---- the column ----

test('Hide settings folds the column, shows Show settings and floats the status', async () => {
  const w = start();
  await ready(w);
  w.el('hideSettings').focus();
  w.el('hideSettings').click();
  assert.equal(w.el('settings').classList.contains('-hidden'), true);
  assert.equal(w.el('settings').inert, true);
  assert.equal(w.el('hideSettings').getAttribute('aria-expanded'), 'false');
  assert.equal(w.el('showSettings').hidden, false);
  assert.equal(w.doc.activeElement, w.el('showSettings'));
  assert.equal(w.el('status').classList.contains('-floating'), true);
  assert.equal(w.el('status').parent, w.el('statusFloat'));
});

test('Show settings restores the column with the same crf, audio and folder', async () => {
  const w = start({ answers: { choose_folder: { folder: 'Exports' } } });
  await ready(w);
  moveSlider(w, '31');
  pickAudio(w, 'remove');
  w.el('chooseFolder').click();
  await settle();
  w.el('hideSettings').click();
  w.el('showSettings').click();
  assert.equal(w.el('settings').classList.contains('-hidden'), false);
  assert.equal(w.el('settings').inert, false);
  assert.equal(w.el('hideSettings').getAttribute('aria-expanded'), 'true');
  assert.equal(w.el('showSettings').hidden, true);
  assert.equal(w.doc.activeElement, w.el('hideSettings'));
  assert.equal(w.el('status').classList.contains('-floating'), false);
  assert.equal(w.el('status').parent, w.el('settingsBottom'));
  assert.equal(w.el('settingsBottom').children[0], w.el('status'), 'the status sits above Save again');
  assert.equal(w.el('crf').value, '31');
  assert.equal(w.el('crfValue').textContent, 'CRF 31');
  assert.equal(w.el('audio').value, 'remove');
  assert.equal(w.el('saveTo').textContent, 'Exports');
});

// ---- Fill window ----

test('Fill window makes the rest inert and Escape restores it, focus back on Fill window', async () => {
  const w = start();
  await ready(w);
  w.el('fill').focus();
  w.el('fill').click();
  assert.equal(w.el('stage').classList.contains('-full'), true);
  assert.equal(w.el('fill').getAttribute('aria-pressed'), 'true');
  assert.equal(w.el('header').inert, true);
  assert.equal(w.el('settings').inert, true);
  assert.equal(w.el('stage').inert, false);
  w.el('play').focus();
  const escape = w.doc.fire('keydown', { key: 'Escape' });
  assert.equal(escape.defaultPrevented, true);
  assert.equal(w.el('stage').classList.contains('-full'), false);
  assert.equal(w.el('fill').getAttribute('aria-pressed'), 'false');
  assert.equal(w.el('header').inert, false);
  assert.equal(w.el('settings').inert, false);
  assert.equal(w.doc.activeElement, w.el('fill'));
});

test('Escape without fill does nothing', async () => {
  const w = start();
  await ready(w);
  w.el('play').focus();
  const escape = w.doc.fire('keydown', { key: 'Escape' });
  assert.equal(escape.defaultPrevented, false);
  assert.equal(w.doc.activeElement, w.el('play'));
  assert.equal(w.el('header').inert, false);
  assert.equal(w.el('settings').inert, false);
});

test('another key during fill keeps the fill', async () => {
  const w = start();
  await ready(w);
  w.el('fill').click();
  w.doc.fire('keydown', { key: 'Enter' });
  assert.equal(w.el('stage').classList.contains('-full'), true);
});

test('Fill window with the column folded keeps the column inert after Escape', async () => {
  const w = start();
  await ready(w);
  w.el('hideSettings').click();
  w.el('fill').click();
  assert.equal(w.el('showSettings').hidden, true, 'Show settings goes while the comparison fills the window');
  w.doc.fire('keydown', { key: 'Escape' });
  assert.equal(w.el('settings').inert, true);
  assert.equal(w.el('showSettings').hidden, false);
});

test('Fill window and Zoom 100% do nothing without a video', async () => {
  const w = start();
  await w.listening;
  assert.equal(w.el('fill').getAttribute('aria-disabled'), 'true');
  assert.equal(w.el('zoom').getAttribute('aria-disabled'), 'true');
  w.el('fill').click();
  w.el('zoom').click();
  assert.equal(w.el('stage').classList.contains('-full'), false);
  assert.equal(w.el('stage').classList.contains('-zoomed'), false);
  assert.equal(w.el('header').inert, false);
});

// ---- the view switch and the divider ----

test('Side by side shows the two next to each other without the clip', async () => {
  const w = start();
  await ready(w);
  w.el('viewSideBySide').click();
  assert.equal(w.el('stage').classList.contains('-sideBySide'), true);
  assert.equal(w.el('viewSideBySide').getAttribute('aria-pressed'), 'true');
  assert.equal(w.el('viewBeforeAfter').getAttribute('aria-pressed'), 'false');
  assert.equal(w.el('layerAfter').style.clipPath, 'none');
  assert.equal(w.el('divider').hidden, true);
});

test('Before / after brings back the clip where the divider was', async () => {
  const w = start();
  await ready(w);
  w.el('divider').value = '30';
  w.el('divider').fire('input');
  w.el('viewSideBySide').click();
  w.el('viewBeforeAfter').click();
  assert.equal(w.el('stage').classList.contains('-sideBySide'), false);
  assert.equal(w.el('viewBeforeAfter').getAttribute('aria-pressed'), 'true');
  assert.equal(w.el('layerAfter').style.clipPath, 'inset(0 0 0 30%)');
  assert.equal(w.el('divider').hidden, false);
});

test('the divider starts at 50', () => {
  const w = start();
  assert.equal(w.el('divider').getAttribute('min'), '0');
  assert.equal(w.el('divider').getAttribute('max'), '100');
  assert.equal(w.el('divider').value, '50');
  assert.equal(w.el('layerAfter').style.clipPath, 'inset(0 0 0 50%)');
  assert.equal(w.el('handle').style.left, '50%');
});

test('the divider input moves the clip', async () => {
  const w = start();
  await ready(w);
  w.el('divider').value = '20';
  w.el('divider').fire('input');
  assert.equal(w.el('layerAfter').style.clipPath, 'inset(0 0 0 20%)');
  assert.equal(w.el('handle').style.left, '20%');
  w.el('divider').value = '50';
  w.el('divider').fire('input');
  assert.equal(w.el('layerAfter').style.clipPath, 'inset(0 0 0 50%)');
  assert.equal(w.el('handle').style.left, '50%');
});

test('"abc" leaves the last clip', async () => {
  const w = start();
  await ready(w);
  w.el('divider').value = '20';
  w.el('divider').fire('input');
  w.el('divider').value = 'abc';
  w.el('divider').fire('input');
  assert.equal(w.el('layerAfter').style.clipPath, 'inset(0 0 0 20%)');
  assert.equal(w.el('handle').style.left, '20%');
});

// ---- playback ----

test('play starts both videos, pause stops both, seeking the bar seeks both', async () => {
  const w = start();
  await sampled(w);
  const [before, after] = [w.el('videoBefore'), w.el('videoAfter')];
  w.el('play').click();
  await settle();
  assert.equal(before.paused, false);
  assert.equal(after.paused, false);
  assert.equal(w.el('play').textContent, 'Pause');
  w.el('play').click();
  await settle();
  assert.equal(before.paused, true);
  assert.equal(after.paused, true);
  assert.equal(w.el('play').textContent, 'Play');
  w.el('position').value = '4.5';
  w.el('position').fire('input');
  assert.equal(before.seeks.at(-1), 4.5);
  assert.equal(after.seeks.at(-1), 4.5);
  assert.equal(w.el('time').textContent, '0:04 / 0:10');
});

test('a position that is not a number seeks nothing', async () => {
  const w = start();
  await sampled(w);
  const seeks = [w.el('videoBefore').seeks.length, w.el('videoAfter').seeks.length];
  w.el('position').value = 'abc';
  w.el('position').fire('input');
  assert.deepEqual([w.el('videoBefore').seeks.length, w.el('videoAfter').seeks.length], seeks);
});

test('a drift above one frame is corrected on timeupdate and after a seek', async () => {
  const w = start();
  await sampled(w);
  const [before, after] = [w.el('videoBefore'), w.el('videoAfter')];
  before.time = 1;
  after.time = 1.1;
  before.fire('timeupdate');
  assert.equal(after.currentTime, 1);
  before.time = 3;
  after.time = 2.5;
  before.fire('seeked');
  assert.equal(after.currentTime, 3);
});

test('a drift of 0.02 s at 24 fps is left alone', async () => {
  const w = start();
  await sampled(w);
  const [before, after] = [w.el('videoBefore'), w.el('videoAfter')];
  before.time = 1;
  after.time = 1.02;
  const seeks = after.seeks.length;
  before.fire('timeupdate');
  assert.equal(after.seeks.length, seeks);
  assert.equal(after.currentTime, 1.02);
});

test('timeupdate moves the position bar and the time', async () => {
  const w = start();
  await sampled(w);
  w.el('videoBefore').time = 3.2;
  w.el('videoBefore').fire('timeupdate');
  assert.equal(w.el('position').value, '3.2');
  assert.equal(w.el('time').textContent, '0:03 / 0:10');
  assert.equal(w.el('position').getAttribute('aria-valuetext'), '0:03 of 0:10');
});

test('the length of a clip shorter than 10 seconds is its own', async () => {
  const w = start();
  await ready(w, { duration: 6.5 });
  assert.equal(w.el('time').textContent, '0:00 / 0:06');
  assert.equal(w.el('position').getAttribute('max'), '6.5');
  w.el('videoBefore').duration = 6.4;
  w.el('videoBefore').fire('loadedmetadata');
  assert.equal(w.el('position').getAttribute('max'), '6.4');
});

test('Play does nothing until the before clip is ready', async () => {
  const w = start();
  await load(w);
  assert.equal(w.el('play').getAttribute('aria-disabled'), 'true');
  w.el('play').click();
  await settle();
  assert.equal(w.el('videoBefore').plays, 0);
  assert.equal(w.el('play').textContent, 'Play');
});

test('Play before a sample plays the original only', async () => {
  const w = start();
  await ready(w);
  w.el('play').click();
  await settle();
  assert.equal(w.el('videoBefore').plays, 1);
  assert.equal(w.el('videoAfter').plays, 0);
});

test('a refused play of the original is logged and leaves Play', async () => {
  const w = start();
  await sampled(w);
  w.doc.refusePlay = true;
  w.el('play').click();
  await settle();
  assert.equal(w.logged.length, 2, 'both refusals are logged');
  assert.equal(w.logged[0][0], 'play');
  assert.equal(w.el('play').textContent, 'Play');
  assert.ok(w.el('videoAfter').pauses >= 1, 'the sample is stopped with it');
});

test('a refused play of the sample alone is logged and the original keeps playing', async () => {
  const w = start();
  await ready(w);
  w.el('play').click();
  await settle();
  w.doc.refusePlay = true;
  w.tauri.emit('sample', sampleOf(1));
  await settle();
  assert.deepEqual(w.logged.map(([what]) => what), ['play']);
  assert.equal(w.el('play').textContent, 'Pause');
  assert.equal(w.el('videoBefore').paused, false);
});

test('the end of the original stops the sample and shows Play', async () => {
  const w = start();
  await sampled(w);
  w.el('play').click();
  await settle();
  w.el('videoBefore').paused = true;
  w.el('videoBefore').ended = true;
  w.el('videoBefore').fire('ended');
  assert.equal(w.el('videoAfter').paused, true);
  assert.equal(w.el('play').textContent, 'Play');
  w.el('play').click();
  await settle();
  assert.equal(w.el('videoBefore').seeks.at(-1), 0, 'Play after the end starts again at 0');
  assert.equal(w.el('videoAfter').seeks.at(-1), 0);
});

test('the original video is always muted; the sample is muted only with Audio Remove', async () => {
  const w = start();
  assert.equal(w.el('videoBefore').muted, true);
  await sampled(w);
  assert.equal(w.el('videoBefore').muted, true);
  assert.equal(w.el('videoAfter').muted, false);
  pickAudio(w, 'remove');
  assert.equal(w.el('videoAfter').muted, true);
  assert.equal(w.el('videoBefore').muted, true);
  pickAudio(w, 'keep');
  assert.equal(w.el('videoAfter').muted, false);
});

// ---- zoom ----

const IMAGE = { width: 3840, height: 2160 };

test('the picture starts fitted to the area', async () => {
  const w = start();
  await ready(w);
  const fit = toFit(zoomLimits(IMAGE, AREA));
  assert.ok(fit.scale < 1);
  assert.equal(shownTransform(w), transformOf(fit));
  assert.equal(w.el('videoBefore').style.width, '3840px');
  assert.equal(w.el('videoBefore').style.height, '2160px');
});

test('Zoom 100% shows one video pixel per screen pixel', async () => {
  const w = start();
  await ready(w);
  w.el('zoom').click();
  const limits = zoomLimits(IMAGE, AREA);
  const actual = toActual(toFit(limits), limits, IMAGE, AREA);
  assert.equal(actual.scale, 1);
  assert.equal(shownTransform(w), transformOf(actual));
  assert.equal(w.el('stage').classList.contains('-zoomed'), true);
  assert.equal(w.el('zoom').textContent, 'Fit to window');
});

test('Fit to window shows the whole frame again', async () => {
  const w = start();
  await ready(w);
  w.el('zoom').click();
  w.el('zoom').click();
  assert.equal(shownTransform(w), transformOf(toFit(zoomLimits(IMAGE, AREA))));
  assert.equal(w.el('stage').classList.contains('-zoomed'), false);
  assert.equal(w.el('zoom').textContent, 'Zoom 100%');
});

test('dragging while zoomed moves around the picture with the pointer captured', async () => {
  const w = start();
  await ready(w);
  w.el('zoom').click();
  const picture = w.el('picture');
  picture.fire('pointerdown', { button: 0, pointerId: 7, clientX: 400, clientY: 300 });
  assert.deepEqual(picture.captured, [7]);
  picture.fire('pointermove', { clientX: 350, clientY: 280 });
  assert.equal(shownTransform(w), 'translate(-50px, -20px) scale(1)');
  picture.fire('pointerup', {});
  picture.fire('pointermove', { clientX: 0, clientY: 0 });
  assert.equal(shownTransform(w), 'translate(-50px, -20px) scale(1)', 'the drag ended');
});

test('a drag when not zoomed, or with another button, pans nothing', async () => {
  const w = start();
  await ready(w);
  const picture = w.el('picture');
  const fitted = shownTransform(w);
  picture.fire('pointerdown', { button: 0, pointerId: 1, clientX: 400, clientY: 300 });
  picture.fire('pointermove', { clientX: 300, clientY: 200 });
  assert.equal(shownTransform(w), fitted);
  w.el('zoom').click();
  const zoomed = shownTransform(w);
  picture.fire('pointerdown', { button: 2, pointerId: 2, clientX: 400, clientY: 300 });
  picture.fire('pointermove', { clientX: 300, clientY: 200 });
  assert.equal(shownTransform(w), zoomed);
  assert.deepEqual(picture.captured, []);
});

test('a drag on the handle while zoomed moves the divider', async () => {
  const w = start();
  await ready(w);
  w.el('zoom').click();
  const zoomed = shownTransform(w);
  const picture = w.el('picture');
  picture.fire('pointerdown', { button: 0, pointerId: 3, clientX: 500, clientY: 300, target: w.el('handle') });
  picture.fire('pointermove', { clientX: 300, clientY: 300 });
  assert.equal(w.el('layerAfter').style.clipPath, 'inset(0 0 0 25%)');
  assert.equal(w.el('divider').value, '25');
  assert.equal(shownTransform(w), zoomed, 'the picture did not move');
});

test('a resize at fit fits again; the view follows the new area', async () => {
  const w = start();
  await ready(w);
  const small = { width: 400, height: 300 };
  w.el('layerBefore').clientWidth = small.width;
  w.el('layerBefore').clientHeight = small.height;
  w.observers.resize(w.el('layerBefore'));
  assert.equal(shownTransform(w), transformOf(toFit(zoomLimits(IMAGE, small))));
});

test('start without a ResizeObserver does not throw and still fits', async () => {
  const w = start({ resizeObserver: false });
  await ready(w);
  assert.equal(shownTransform(w), transformOf(toFit(zoomLimits(IMAGE, AREA))));
});

// ---- save ----

test('Save sends the video, crf and audio', async () => {
  const w = start();
  await ready(w);
  moveSlider(w, '30');
  pickAudio(w, 'remove');
  w.el('save').click();
  await settle();
  assert.deepEqual(sent(w, 'save'), [['save', { video: 3, crf: 30, audio: 'remove' }]]);
});

test('Save disables Quality, Audio, Choose other, Choose folder and shows Cancel and the progress', async () => {
  const w = start();
  await ready(w);
  w.el('save').click();
  await settle();
  for (const id of SAVE_LOCKED) {
    assert.equal(w.el(id).disabled, true, id);
  }
  assert.equal(w.el('cancel').hidden, false);
  assert.equal(w.el('progress').hidden, false);
  assert.equal(w.el('progress').getAttribute('value'), null, 'indeterminate until a percent comes');
  assert.equal(w.el('save').getAttribute('aria-disabled'), 'true');
  w.el('save').click();
  await settle();
  assert.equal(sent(w, 'save').length, 1, 'a second Save sends nothing');
});

test('save progress shows its percent, and an unknown one shows no value', async () => {
  const w = start();
  await ready(w);
  w.el('save').click();
  w.tauri.emit('save-progress', { percent: 42 });
  assert.equal(w.el('progress').getAttribute('value'), '42');
  assert.equal(w.el('statusText').textContent, 'Saving… 42%');
  w.tauri.emit('save-progress', { percent: null });
  assert.equal(w.el('progress').getAttribute('value'), null);
});

test('save-done enables them again and hides Cancel', async () => {
  const w = start();
  await ready(w);
  w.el('save').click();
  w.tauri.emit('save-done', { name: 'optimized-Showreel V1.mp4', bytes: 40_800_000, inputBytes: 96_900_000, keptOriginal: false });
  for (const id of SAVE_LOCKED) {
    assert.equal(w.el(id).disabled, false, id);
  }
  assert.equal(w.el('cancel').hidden, true);
  assert.equal(w.el('progress').hidden, true);
  assert.equal(w.el('save').getAttribute('aria-disabled'), 'false');
  assert.equal(w.el('statusText').textContent, 'Saved optimized-Showreel V1.mp4 (40.8 MB, 58% smaller)');
});

test('a save that ends with focus on Cancel moves focus to Save', async () => {
  const w = start();
  await ready(w);
  w.el('save').click();
  w.el('cancel').focus();
  w.tauri.emit('save-cancelled', {});
  assert.equal(w.doc.activeElement, w.el('save'));
  assert.equal(w.el('statusText').textContent, 'Save cancelled');
});

test('a refused save shows Save failed and enables the controls again', async () => {
  const w = start({ refuse: { save: 'a save is already running' } });
  await ready(w);
  w.el('save').click();
  await settle();
  assert.equal(w.el('statusText').textContent, 'Save failed: a save is already running');
  for (const id of SAVE_LOCKED) {
    assert.equal(w.el(id).disabled, false, id);
  }
  assert.equal(w.el('cancel').hidden, true);
});

test('Cancel sends cancel_save while a save runs', async () => {
  const w = start();
  await ready(w);
  w.el('save').click();
  w.el('cancel').click();
  await settle();
  assert.equal(sent(w, 'cancel_save').length, 1);
});

test('Cancel sends nothing when no save runs, and a refused cancel is logged', async () => {
  const w = start({ refuse: { cancel_save: 'no window' } });
  await ready(w);
  w.el('cancel').click();
  await settle();
  assert.deepEqual(sent(w, 'cancel_save'), []);
  w.el('save').click();
  w.el('cancel').click();
  await settle();
  assert.deepEqual(w.logged, [['cancel_save', 'no window']]);
});

test('a sample cut short by a save is asked again when the save ends', async () => {
  const w = start({ hold: ['make_sample'] });
  await ready(w);
  w.el('save').click();
  w.tauri.emit('save-done', { name: 'a.mp4', bytes: 1, inputBytes: 2, keptOriginal: false });
  await settle();
  const asked = samplesAsked(w);
  assert.equal(asked.length, 2);
  assert.ok(asked[1].request > asked[0].request);
});

test('a slider move just before Save asks no sample while the save runs', async () => {
  const w = start();
  await ready(w);
  moveSlider(w, '30');
  w.el('save').click();
  w.timers.advance(400);
  assert.equal(samplesAsked(w).length, 1);
});

// ---- save to ----

test('Choose folder shows the folder it answers, and Clear brings back Next to the original', async () => {
  const w = start({ answers: { choose_folder: { folder: 'Exports' } } });
  await w.listening;
  assert.equal(w.el('saveTo').textContent, 'Next to the original');
  assert.equal(w.el('clearFolder').hidden, true);
  w.el('chooseFolder').click();
  await settle();
  assert.equal(w.el('saveTo').textContent, 'Exports');
  assert.equal(w.el('clearFolder').hidden, false);
  w.el('clearFolder').focus();
  w.el('clearFolder').click();
  await settle();
  assert.deepEqual(w.tauri.commands(), ['choose_folder', 'clear_folder']);
  assert.equal(w.el('saveTo').textContent, 'Next to the original');
  assert.equal(w.el('clearFolder').hidden, true);
  assert.equal(w.doc.activeElement, w.el('chooseFolder'));
});

test('a closed folder dialog keeps the folder in effect', async () => {
  const w = start({ answers: { choose_folder: { folder: null } } });
  await w.listening;
  w.el('chooseFolder').click();
  await settle();
  assert.equal(w.el('saveTo').textContent, 'Next to the original');
  assert.equal(w.el('clearFolder').hidden, true);
});

test('a refused choose_folder or clear_folder keeps the folder and is logged', async () => {
  const w = start({ refuse: { choose_folder: 'denied' } });
  await w.listening;
  w.el('chooseFolder').click();
  await settle();
  assert.equal(w.el('saveTo').textContent, 'Next to the original');
  assert.deepEqual(w.logged, [['choose_folder', 'denied']]);

  const v = start({ answers: { choose_folder: { folder: 'Exports' } }, refuse: { clear_folder: 'denied' } });
  await v.listening;
  v.el('chooseFolder').click();
  await settle();
  v.el('clearFolder').click();
  await settle();
  assert.equal(v.el('saveTo').textContent, 'Exports');
  assert.deepEqual(v.logged, [['clear_folder', 'denied']]);
});

test('an answer from choose_folder that is not a folder answer changes nothing', async () => {
  const w = start({ answers: { choose_folder: 'Exports' } });
  await w.listening;
  w.el('chooseFolder').click();
  await settle();
  assert.equal(w.el('saveTo').textContent, 'Next to the original');
});

test('Clear sends nothing while a save runs', async () => {
  const w = start({ answers: { choose_folder: { folder: 'Exports' } } });
  await ready(w);
  w.el('chooseFolder').click();
  await settle();
  w.el('save').click();
  assert.equal(w.el('clearFolder').disabled, true);
  w.el('clearFolder').click();
  await settle();
  assert.deepEqual(sent(w, 'clear_folder'), []);
});

// ---- events and commands match the backend ----

/** The event names in events.rs's `Event::name`. */
function backendEvents() {
  const source = readFileSync(new URL('../../src-tauri/src/events.rs', import.meta.url), 'utf8');
  return [...source.matchAll(/Event::\w+\(_\) => "([a-z-]+)"/g)].map((match) => match[1]);
}

/** The command names in lib.rs's `generate_handler!` list. */
function handlerNames(source) {
  const list = source.match(/generate_handler!\[([^\]]*)\]/);
  if (list === null) {
    return [];
  }
  return list[1].split(',').map((name) => name.trim()).filter((name) => name !== '');
}

function backendCommands() {
  return handlerNames(readFileSync(new URL('../../src-tauri/src/lib.rs', import.meta.url), 'utf8'));
}

test('the window listens to exactly the events the backend sends', async () => {
  const w = start();
  await w.listening;
  const events = backendEvents();
  assert.equal(events.length, 9);
  assert.deepEqual([...w.tauri.handlers.keys()].sort(), [...events].sort());
});

test('every command the window sends is one the backend registers', async () => {
  const commands = backendCommands();
  assert.equal(commands.length, 6);
  const w = start({ answers: { choose_folder: { folder: 'Exports' } } });
  await ready(w);
  w.el('chooseOther').click();
  w.el('chooseFolder').click();
  await settle();
  w.el('clearFolder').click();
  w.el('save').click();
  w.el('cancel').click();
  await settle();
  const used = new Set(w.tauri.commands());
  assert.equal(used.size, 6);
  assert.deepEqual([...used].filter((name) => !commands.includes(name)), []);
});

test('a handler list missing a command is seen as missing', () => {
  const names = handlerNames('generate_handler![choose_video, make_sample]');
  assert.deepEqual(names, ['choose_video', 'make_sample']);
  assert.equal(names.includes('save'), false);
  assert.deepEqual(handlerNames('no list here'), []);
});

test('a refused subscription is logged with its event name', async () => {
  const w = start({ refuseListen: ['save-done'] });
  await w.listening;
  await settle();
  assert.equal(w.logged.length, 1);
  assert.equal(w.logged[0][0], 'listen save-done');
  assert.equal(w.tauri.handlers.has('save-progress'), true, 'the other events are still heard');
});

// ---- texts are text ----

test('no element is written with innerHTML', async () => {
  const w = start({ answers: { choose_folder: { folder: '<img src=x onerror=alert(1)>' } } });
  await ready(w, { name: '<b>bold</b>.mp4' });
  w.tauri.emit('sample', sampleOf(1));
  moveSlider(w, '33');
  pickAudio(w, 'remove');
  w.timers.advance(400);
  w.el('chooseFolder').click();
  await settle();
  w.el('hideSettings').click();
  w.el('showSettings').click();
  w.el('fill').click();
  w.doc.fire('keydown', { key: 'Escape' });
  w.el('zoom').click();
  w.el('viewSideBySide').click();
  w.el('play').click();
  w.el('save').click();
  w.tauri.emit('save-progress', { percent: 10 });
  w.tauri.emit('notice', { kind: 'refused', name: '<i>x</i>.mp4', reason: '<script>' });
  w.tauri.emit('save-failed', { reason: '<b>disk</b>' });
  await settle();
  assert.equal(w.el('fileName').textContent, '<b>bold</b>.mp4', 'the name is kept as text');
  assert.equal(w.el('saveTo').textContent, '<img src=x onerror=alert(1)>');
  assert.deepEqual(w.doc.markupWrites, []);
});

test('the fake records a markup write when one is made, so the check can fail', () => {
  const w = start();
  w.el('statusText').innerHTML = '<b>x</b>';
  w.el('fileName').insertAdjacentHTML('beforeend', '<i>y</i>');
  assert.deepEqual(w.doc.markupWrites, [
    ['innerHTML', '<b>x</b>'],
    ['insertAdjacentHTML', '<i>y</i>'],
  ]);
});
