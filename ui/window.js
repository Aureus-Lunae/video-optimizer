// The Video optimizer window's wiring. The backend does the work and sends
// events (src-tauri/src/events.rs); `state.js` turns each event or user action
// into the next state, `quality.js` and `format.js` give the texts,
// `compare.js` the divider's clip and the sample's drift correction,
// `zoom.js` the zoom and pan maths, and `clips.js` how each video is given its
// clip (the media scheme, or a blob: address where the webview cannot play
// the scheme).
//
// `startWindow` looks up the elements, draws the page and hands every event
// to those functions. It is given the page, the Tauri API, the timers,
// ResizeObserver, fetch, the object-URL functions, the page-unload hook and a
// log, so the node tests drive it with stand-ins
// (tests/ui/fake-window.js); app.js calls it with the real ones. Nothing is
// stored between launches. Every text reaches the page with textContent only.

import {
  formatAudio,
  formatFps,
  formatLength,
  formatMB,
  formatPosition,
  formatPsnr,
  reasonText,
} from './format.js';
import { canSave, controlsDisabled, initialState, reduce, sampleRequest } from './state.js';
import { crfValueText, crfWord, readCrf } from './quality.js';
import { dividerClip, driftCorrection } from './compare.js';
import { createClipSources } from './clips.js';
import { divider, pan, toActual, toFit, zoomLimits } from './zoom.js';

/** Every event the backend sends (events.rs, `Event::name`). */
const EVENTS = [
  'video',
  'before-ready',
  'notice',
  'sample',
  'sample-failed',
  'save-progress',
  'save-done',
  'save-failed',
  'save-cancelled',
];

/** A new sample is asked this long after the slider or Audio stops changing. */
const SAMPLE_SETTLE_MS = 400;

/** The sample is the first 10 seconds, or the whole clip when it is shorter. */
const SAMPLE_SECONDS = 10;

const NEXT_TO_ORIGINAL = 'Next to the original';
const NO_VIDEO = 'No video yet';
const NO_SAMPLE = 'No sample yet';

/** The folder in `choose_folder`'s answer (`{ folder }`); `undefined` for anything else, which the state refuses. */
function folderOf(answer) {
  return answer !== null && typeof answer === 'object' ? answer.folder : undefined;
}

/** "58%"; "unknown" when it is not a whole number. */
function percentText(value) {
  return Number.isInteger(value) ? `${value}%` : 'unknown';
}

/** Writes `text` into `element` only when it differs, so a live region speaks only for news. */
function showText(element, text) {
  if (element.textContent !== text) {
    element.textContent = text;
  }
}

/** Hides or shows `element`; focus on it when it hides goes to `fallback`. */
function hideKeepingFocus(doc, element, hidden, fallback) {
  const hadFocus = hidden && !element.hidden && doc.activeElement === element;
  element.hidden = hidden;
  if (hadFocus) {
    fallback.focus();
  }
}

/**
 * Starts the window on `env.doc` and draws it.
 * `env`:
 * - `doc`: the page;
 * - `tauri`: the Tauri API (`core.invoke`, `core.convertFileSrc`, `event.listen`);
 * - `timers`: `setTimer` and `clearTimer` (setTimeout and clearTimeout);
 * - `ResizeObserver`: the class (without it the picture is fitted on each
 *   change of video or zoom only, not again when the window is resized);
 * - `fetch(address)`: the page's fetch, for a clip the webview cannot play
 *   from the media scheme (clips.js);
 * - `urls`: `{ createObjectURL, revokeObjectURL }`;
 * - `onUnload(handler)`: runs `handler` when the page goes away, so the
 *   object URLs are revoked;
 * - `log(...values)`: where the unexpected goes.
 * Returns `{ listening }`, which settles once every event subscription has
 * been answered (a refused one is logged).
 */
export function startWindow(env) {
  const { doc, tauri, log } = env;
  const { setTimer, clearTimer } = env.timers;
  const { convertFileSrc, invoke } = tauri.core;
  const { listen } = tauri.event;
  const byId = (id) => doc.getElementById(id);
  const sources = createClipSources({ fetch: env.fetch, urls: env.urls });
  env.onUnload(() => sources.dispose());

  const header = byId('header');
  const stage = byId('stage');
  const viewButtons = { 'before-after': byId('viewBeforeAfter'), 'side-by-side': byId('viewSideBySide') };
  const fillButton = byId('fill');
  const zoomButton = byId('zoom');
  const showSettings = byId('showSettings');
  const picture = byId('picture');
  const empty = byId('empty');
  const chooseVideo = byId('chooseVideo');
  const compare = byId('compare');
  const layerBefore = byId('layerBefore');
  const layerAfter = byId('layerAfter');
  const videoBefore = byId('videoBefore');
  const videoAfter = byId('videoAfter');
  const clipBefore = sources.attach(videoBefore);
  const clipAfter = sources.attach(videoAfter);
  const tagBefore = byId('tagBefore');
  const tagAfter = byId('tagAfter');
  const dividerInput = byId('divider');
  const handle = byId('handle');
  const statusFloat = byId('statusFloat');
  const playButton = byId('play');
  const position = byId('position');
  const time = byId('time');

  const settings = byId('settings');
  const hideSettings = byId('hideSettings');
  const fileEmpty = byId('fileEmpty');
  const fileFigures = byId('fileFigures');
  const file = {
    name: byId('fileName'),
    size: byId('fileSize'),
    fps: byId('fileFps'),
    length: byId('fileLength'),
    audio: byId('fileAudio'),
  };
  const chooseOther = byId('chooseOther');
  const crfInput = byId('crf');
  const crfValue = byId('crfValue');
  const crfWordText = byId('crfWord');
  const audioSelect = byId('audio');
  const chooseFolder = byId('chooseFolder');
  const saveTo = byId('saveTo');
  const clearFolder = byId('clearFolder');
  const resultEmpty = byId('resultEmpty');
  const resultFigures = byId('resultFigures');
  const result = {
    original: byId('resultOriginal'),
    optimized: byId('resultOptimized'),
    smaller: byId('resultSmaller'),
    whole: byId('resultWhole'),
    psnr: byId('resultPsnr'),
  };
  const settingsBottom = byId('settingsBottom');
  const status = byId('status');
  const statusText = byId('statusText');
  const progress = byId('progress');
  const cancel = byId('cancel');
  const save = byId('save');

  let state = initialState();
  // The request number a sample was last asked for, and the timer that waits
  // for the slider or Audio to settle.
  let askedRequest = null;
  let settleTimer = null;
  // What the two videos hold: the video id they were set up for, and the
  // media key each one plays.
  let shownVideo = null;
  let shownBefore = null;
  let shownSample = null;
  let playing = false;
  // The divider's clip, kept while Side by side shows no clip; the zoom view
  // both videos show, and what it was last fitted for; a drag in progress.
  let clip = dividerClip(divider(Number.NaN));
  let view = { scale: 1, x: 0, y: 0 };
  let fitted = null;
  let drag = null;
  // Whether the status line sits in the slot over the picture (index.html
  // starts it above Save).
  let statusFloating = false;

  // --- the sample -----------------------------------------------------------------

  /**
   * Asks `make_sample` when the state awaits one (`sampling`), once per
   * request number, and not while the slider or Audio is still settling. A
   * refusal is the sample failing.
   */
  function askSample() {
    if (!state.sampling || settleTimer !== null || state.request === askedRequest) {
      return;
    }
    const args = sampleRequest(state);
    if (args === null) {
      return;
    }
    askedRequest = args.request;
    invoke('make_sample', args).then(null, (reason) => {
      dispatch({ type: 'sample-failed', video: args.video, request: args.request, reason: reasonText(reason) });
    });
  }

  /** The slider or Audio changed: the sample is asked once they have been still for 400 ms. */
  function settleThenAsk() {
    if (settleTimer !== null) {
      clearTimer(settleTimer);
    }
    settleTimer = setTimer(() => {
      settleTimer = null;
      askSample();
    }, SAMPLE_SETTLE_MS);
  }

  /**
   * The next state after `action`, drawn. Returns whether anything changed.
   * `settle`: the action is a move of the slider or Audio, so the sample it
   * asks for waits for the next 400 ms of stillness.
   */
  function dispatch(action, settle = false) {
    const next = reduce(state, action);
    if (next === state) {
      return false;
    }
    state = next;
    if (settle) {
      settleThenAsk();
    }
    render();
    askSample();
    return true;
  }

  // --- the picture ----------------------------------------------------------------

  function image() {
    return state.video === null ? null : { width: state.video.width, height: state.video.height };
  }

  function area() {
    return { width: layerBefore.clientWidth, height: layerBefore.clientHeight };
  }

  /** Both videos at their own size, moved and scaled by the one view. */
  function writeView() {
    const size = image();
    for (const video of [videoBefore, videoAfter]) {
      if (size !== null) {
        video.style.width = `${size.width}px`;
        video.style.height = `${size.height}px`;
      }
      video.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.scale})`;
    }
  }

  /** The view for the zoom the state asks: the whole frame (Fit), or one video pixel per screen pixel. */
  function applyZoom() {
    const limits = zoomLimits(image(), area());
    view = state.zoom === 'actual' ? toActual(view, limits, image(), area()) : toFit(limits);
    fitted = { video: state.video === null ? null : state.video.video, zoom: state.zoom };
    writeView();
  }

  /** The divider at `value` percent; a value that is not a number keeps the clip there is. */
  function moveDivider(value) {
    const next = dividerClip(value);
    if (next === null) {
      return false;
    }
    clip = next;
    handle.style.left = `${divider(value)}%`;
    layerAfter.style.clipPath = state.view === 'before-after' ? clip : 'none';
    return true;
  }

  // --- playback -------------------------------------------------------------------

  /** The length of the clips: the before clip's own once known, else the sample's 10 seconds or the shorter video. */
  function span() {
    const known = videoBefore.duration;
    if (Number.isFinite(known) && known > 0) {
      return known;
    }
    const length = state.video === null ? null : state.video.duration;
    return typeof length === 'number' && Number.isFinite(length) && length > 0
      ? Math.min(SAMPLE_SECONDS, length)
      : SAMPLE_SECONDS;
  }

  function renderPosition() {
    const now = videoBefore.currentTime;
    const total = span();
    position.setAttribute('max', String(total));
    position.value = String(now);
    position.setAttribute('aria-valuetext', `${formatLength(now)} of ${formatLength(total)}`);
    showText(time, formatPosition(now, total));
  }

  function renderPlay() {
    showText(playButton, playing ? 'Pause' : 'Play');
  }

  /** The sample follows the original when they are more than a frame apart. */
  function followOriginal() {
    if (shownSample === null) {
      return;
    }
    const fps = state.video === null ? null : state.video.fps;
    const to = driftCorrection(videoBefore.currentTime, videoAfter.currentTime, fps);
    if (to !== null) {
      videoAfter.currentTime = to;
    }
  }

  function stopPlaying() {
    playing = false;
    videoBefore.pause();
    videoAfter.pause();
    renderPlay();
  }

  /** Plays the sample; a refusal is logged (the original plays on). */
  function playSample() {
    videoAfter.play().then(null, (error) => log('play', error));
  }

  function startPlaying() {
    playing = true;
    if (videoBefore.ended) {
      seekBoth(0);
    }
    videoBefore.play().then(null, (error) => {
      log('play', error);
      stopPlaying();
    });
    if (shownSample !== null) {
      playSample();
    }
    renderPlay();
  }

  function seekBoth(seconds) {
    videoBefore.currentTime = seconds;
    if (shownSample !== null) {
      videoAfter.currentTime = seconds;
    }
  }

  /**
   * The two videos follow the state: a new video clears both, a ready clip or
   * a new sample is loaded (clips.js gives each its source: the media scheme
   * address, or a blob: address where the webview cannot play the scheme).
   * A before clip that cannot be fetched is reported as an original that
   * could not be prepared, a sample's as a sample that could not be played.
   */
  function renderMedia() {
    const id = state.video === null ? null : state.video.video;
    if (id !== shownVideo) {
      playing = false;
      clipBefore.clear();
      clipAfter.clear();
      shownVideo = id;
      shownBefore = null;
      shownSample = null;
    }
    if (state.video !== null && state.video.beforeReady && shownBefore === null) {
      shownBefore = `${id}-before`;
      clipBefore.show(convertFileSrc(shownBefore, 'media'), {
        onFailed: (reason) => dispatch({ type: 'notice', kind: 'failed', reason }),
      });
    }
    const sample = state.sample;
    const sampleKey = sample === null ? null : `${sample.video}-sample-${sample.request}`;
    if (sampleKey !== null && sampleKey !== shownSample) {
      shownSample = sampleKey;
      clipAfter.show(convertFileSrc(sampleKey, 'media'), {
        // A new source starts at 0: it joins the original where it is, and
        // plays when the original plays. (A blob: source is set once its
        // bytes are fetched, which is when this runs.)
        onSet: () => {
          videoAfter.currentTime = videoBefore.currentTime;
          if (playing) {
            playSample();
          }
        },
        onFailed: (reason) =>
          dispatch({ type: 'clip-failed', video: sample.video, request: sample.request, reason }),
      });
    }
    videoAfter.hidden = shownSample === null;
    videoBefore.muted = true;
    videoAfter.muted = state.audio === 'remove';
  }

  // --- render ---------------------------------------------------------------------

  function renderStage() {
    const loaded = state.video !== null;
    const ready = loaded && state.video.beforeReady;
    const sideBySide = state.view === 'side-by-side';

    stage.classList.toggle('-full', state.fill);
    stage.classList.toggle('-zoomed', state.zoom === 'actual');
    stage.classList.toggle('-sideBySide', sideBySide);
    for (const [name, button] of Object.entries(viewButtons)) {
      button.setAttribute('aria-pressed', String(state.view === name));
    }
    fillButton.setAttribute('aria-pressed', String(state.fill));
    fillButton.setAttribute('aria-disabled', String(!loaded));
    zoomButton.setAttribute('aria-disabled', String(!loaded));
    showText(zoomButton, state.zoom === 'actual' ? 'Fit to window' : 'Zoom 100%');

    compare.hidden = !loaded;
    showText(tagBefore, loaded && state.video.displayCopy === true ? 'Original (converted for display)' : 'Original');
    showText(tagAfter, `Sample, CRF ${state.crf}`);
    dividerInput.hidden = sideBySide;
    layerAfter.style.clipPath = sideBySide ? 'none' : clip;

    playButton.setAttribute('aria-disabled', String(!ready));
    position.disabled = !ready;
    renderMedia();
    renderPlay();
    renderPosition();

    if (fitted === null || fitted.video !== (loaded ? state.video.video : null) || fitted.zoom !== state.zoom) {
      applyZoom();
    }
  }

  function renderColumn() {
    const loaded = state.video !== null;
    const locked = controlsDisabled(state);

    // The column folds away (it slides off, app.css) and nothing in it can
    // take focus then; it is out of reach too while the comparison fills the
    // window, as is the header.
    settings.classList.toggle('-hidden', !state.columnOpen);
    settings.inert = !state.columnOpen || state.fill;
    header.inert = state.fill;
    hideSettings.setAttribute('aria-expanded', String(state.columnOpen));
    showSettings.hidden = state.columnOpen || state.fill;

    fileEmpty.hidden = loaded;
    fileFigures.hidden = !loaded;
    chooseOther.hidden = !loaded;
    if (loaded) {
      const video = state.video;
      showText(file.name, typeof video.name === 'string' ? video.name : '');
      showText(file.size, `${video.width} x ${video.height}`);
      showText(file.fps, formatFps(video.fps));
      showText(file.length, formatLength(video.duration));
      showText(file.audio, formatAudio(video.audio));
    }

    crfInput.value = String(state.crf);
    crfInput.setAttribute('aria-valuetext', crfValueText(state.crf));
    showText(crfValue, `CRF ${state.crf}`);
    showText(crfWordText, crfWord(state.crf));
    audioSelect.value = state.audio;
    for (const control of [crfInput, audioSelect, chooseOther, chooseFolder, clearFolder]) {
      control.disabled = locked;
    }

    showText(saveTo, state.folder === null ? NEXT_TO_ORIGINAL : state.folder);
    hideKeepingFocus(doc, clearFolder, state.folder === null, chooseFolder);

    const sample = state.sample;
    resultFigures.hidden = sample === null;
    resultEmpty.hidden = sample !== null;
    showText(resultEmpty, loaded ? NO_SAMPLE : NO_VIDEO);
    if (sample !== null) {
      showText(result.original, formatMB(sample.originalBytes));
      showText(result.optimized, formatMB(sample.sampleBytes));
      showText(result.smaller, percentText(sample.smallerBy));
      showText(result.whole, formatMB(sample.wholeBytes));
      showText(result.psnr, formatPsnr(sample.psnr, sample.identical));
    }

    save.setAttribute('aria-disabled', String(!canSave(state)));
  }

  /** The status line, with the save's progress and Cancel; it floats over the picture while the column is out of sight. */
  function renderStatus() {
    showText(statusText, state.status);
    const saving = state.saving !== null;
    progress.hidden = !saving;
    if (saving && state.saving.percent !== null) {
      progress.setAttribute('value', String(state.saving.percent));
    } else {
      progress.removeAttribute('value');
    }
    hideKeepingFocus(doc, cancel, !saving, save);

    const floating = !state.columnOpen || state.fill;
    status.classList.toggle('-floating', floating);
    status.classList.toggle('-empty', state.status === '' && !saving);
    if (floating !== statusFloating) {
      statusFloating = floating;
      if (floating) {
        statusFloat.append(status);
      } else {
        settingsBottom.prepend(status);
      }
    }
  }

  function render() {
    renderStage();
    renderColumn();
    renderStatus();
    // The drop prompt goes once a video is loaded; focus on Choose video goes
    // to Choose other, or to Play while the column is folded.
    empty.hidden = state.video !== null;
    if (empty.hidden && doc.activeElement === chooseVideo) {
      (state.columnOpen ? chooseOther : playButton).focus();
    }
  }

  // --- commands -------------------------------------------------------------------

  /** Sends `command`; a refusal is logged. */
  function send(command) {
    return invoke(command).then(null, (reason) => {
      log(command, reason);
      return undefined;
    });
  }

  function chooseAVideo() {
    if (!controlsDisabled(state)) {
      send('choose_video');
    }
  }

  chooseVideo.addEventListener('click', chooseAVideo);
  chooseOther.addEventListener('click', chooseAVideo);

  chooseFolder.addEventListener('click', () => {
    if (controlsDisabled(state)) {
      return;
    }
    invoke('choose_folder').then(
      (answer) => dispatch({ type: 'folder', folder: folderOf(answer) }),
      (reason) => log('choose_folder', reason),
    );
  });

  clearFolder.addEventListener('click', () => {
    if (controlsDisabled(state)) {
      return;
    }
    invoke('clear_folder').then(
      () => dispatch({ type: 'folder', folder: null }),
      (reason) => log('clear_folder', reason),
    );
  });

  save.addEventListener('click', () => {
    if (!canSave(state)) {
      return;
    }
    const args = { video: state.video.video, crf: state.crf, audio: state.audio };
    dispatch({ type: 'save-started' });
    invoke('save', args).then(null, (reason) => {
      dispatch({ type: 'save-failed', reason: reasonText(reason) });
    });
  });

  cancel.addEventListener('click', () => {
    if (state.saving !== null) {
      send('cancel_save');
    }
  });

  // --- settings -------------------------------------------------------------------

  // The slider: the number, word and spoken value follow every step at once;
  // the sample waits for the slider to be still. A value that is not a CRF
  // changes nothing, and the slider is put back.
  function onCrf() {
    const crf = readCrf(crfInput.value);
    if (crf === null || !dispatch({ type: 'crf', value: crf }, true)) {
      crfInput.value = String(state.crf);
    }
  }

  crfInput.addEventListener('input', onCrf);
  crfInput.addEventListener('change', onCrf);

  audioSelect.addEventListener('change', () => {
    if (!dispatch({ type: 'audio', value: audioSelect.value }, true)) {
      audioSelect.value = state.audio;
    }
  });

  // --- the column, the view, Fill window and the zoom -----------------------------

  hideSettings.addEventListener('click', () => {
    dispatch({ type: 'column', open: false });
    showSettings.focus();
  });

  showSettings.addEventListener('click', () => {
    dispatch({ type: 'column', open: true });
    hideSettings.focus();
  });

  for (const [name, button] of Object.entries(viewButtons)) {
    button.addEventListener('click', () => dispatch({ type: 'view', view: name }));
  }

  fillButton.addEventListener('click', () => {
    if (state.video === null) {
      return;
    }
    dispatch({ type: 'fill', on: !state.fill });
    fillButton.focus();
  });

  // Escape closes Fill window from anywhere in the page; focus goes back to
  // the button that opened it.
  doc.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape' || !state.fill) {
      return;
    }
    event.preventDefault();
    dispatch({ type: 'fill', on: false });
    fillButton.focus();
  });

  zoomButton.addEventListener('click', () => {
    if (state.video !== null) {
      dispatch({ type: 'zoom', zoom: state.zoom === 'actual' ? 'fit' : 'actual' });
    }
  });

  // --- the divider, dragging and the size of the area -----------------------------

  dividerInput.addEventListener('input', () => {
    moveDivider(Number(dividerInput.value));
  });

  // While zoomed, a drag moves around the picture, or, started on the handle,
  // moves the divider (the divider input takes the pointer only at Fit,
  // app.css). Only the primary button drags.
  picture.addEventListener('pointerdown', (event) => {
    if (state.zoom !== 'actual' || event.button !== 0) {
      return;
    }
    const onHandle = event.target === handle && state.view === 'before-after';
    drag = { divider: onHandle, x: event.clientX, y: event.clientY };
    picture.setPointerCapture(event.pointerId);
  });

  picture.addEventListener('pointermove', (event) => {
    if (drag === null) {
      return;
    }
    if (drag.divider) {
      const box = picture.getBoundingClientRect();
      const percent = ((event.clientX - box.left) / box.width) * 100;
      if (moveDivider(percent)) {
        dividerInput.value = String(Math.round(divider(percent)));
      }
      return;
    }
    view = pan(view, event.clientX - drag.x, event.clientY - drag.y, image(), area());
    drag = { ...drag, x: event.clientX, y: event.clientY };
    writeView();
  });

  for (const type of ['pointerup', 'pointercancel']) {
    picture.addEventListener(type, () => {
      drag = null;
    });
  }

  // The area changes with the window, the column, Fill window and the view.
  if (typeof env.ResizeObserver === 'function') {
    new env.ResizeObserver(() => applyZoom()).observe(layerBefore);
  }

  // --- playback -------------------------------------------------------------------

  playButton.addEventListener('click', () => {
    if (state.video === null || !state.video.beforeReady) {
      return;
    }
    if (playing) {
      stopPlaying();
    } else {
      startPlaying();
    }
  });

  position.addEventListener('input', () => {
    const seconds = Number(position.value);
    if (!Number.isFinite(seconds)) {
      return;
    }
    seekBoth(seconds);
    renderPosition();
  });

  videoBefore.addEventListener('timeupdate', () => {
    renderPosition();
    followOriginal();
  });
  videoBefore.addEventListener('seeked', followOriginal);
  videoBefore.addEventListener('loadedmetadata', renderPosition);
  videoBefore.addEventListener('ended', () => {
    playing = false;
    videoAfter.pause();
    renderPlay();
  });

  // --- events ---------------------------------------------------------------------

  // Each backend event is dispatched with its name as the action's type. A
  // refused subscription is logged by name.
  const listening = Promise.all(
    EVENTS.map((type) =>
      listen(type, (event) => {
        const payload = event.payload !== null && typeof event.payload === 'object' ? event.payload : {};
        dispatch({ ...payload, type });
      }).then(null, (reason) => log(`listen ${type}`, reason)),
    ),
  ).then(() => undefined);

  moveDivider(divider(Number.NaN));
  render();

  return { listening };
}
