// The window's state as pure functions: `reduce` turns one backend event or
// one user action into the next state, and the small functions below read
// what the window needs from a state. No DOM here, so the node tests can load
// it; `window.js` writes the state into the page.
//
// An action is `{ type, ...fields }`. A backend event is its payload with the
// event's name as `type`: `video`, `before-ready`, `notice`, `sample`,
// `sample-failed`, `save-progress`, `save-done`, `save-failed` and
// `save-cancelled` (see src-tauri/src/events.rs). A user action is one of:
//
//   { type: 'crf', value }          the slider moved: a whole number 20 to 36
//   { type: 'audio', value }        'keep' or 'remove'
//   { type: 'folder', folder }      `choose_folder`'s answer: a folder name, or null
//   { type: 'view', view }          'before-after' or 'side-by-side'
//   { type: 'column', open }        the settings column is open (true) or folded (false)
//   { type: 'fill', on }            Fill window on or off
//   { type: 'zoom', zoom }          'fit' or 'actual' (Zoom 100%)
//   { type: 'save-started' }        Save was pressed
//
// The texts the window shows are composed here from the backend's data: the
// status line is `state.status`. An action that does not apply (a stale
// event, a value out of range, a wrong type) returns the state it was given,
// as the same object, so the window can tell that nothing changed.
//
// `reduce` never changes the state it is given.
//
// Samples. Each change of quality or audio gets a new `request` number, above
// every one before it: the backend starts no sample for a request that is not
// above the last it saw for the video, and tells the window which request a
// result belongs to. The window asks for a sample (after its 400 ms wait)
// when `sampling` is true, with `sampleRequest(state)`, once per request
// number. A `sample` or `sample-failed` event counts only for the current
// request of the current video; an older one is ignored.
//
// The end of a save ("Saved …", "Save failed: …", "Save cancelled") stays on
// the status line while `keepStatus` is true: a sample the window asks by
// itself after the save (one the save cut short, or the first once the
// before clip is ready) is stored without replacing it. A change of quality
// or audio, a new video, a new save or a failed sample puts its own text
// there again.

import { formatMB } from './format.js';
import { isCrf } from './quality.js';

const STATUS_PREPARING = 'Preparing the original…';
const STATUS_SAMPLING = 'Making the sample…';
const STATUS_SAMPLE_READY = 'Sample ready. Save encodes the whole video.';
const STATUS_SAVING = 'Saving…';
const STATUS_SAVE_CANCELLED = 'Save cancelled';

const AUDIO_CHOICES = ['keep', 'remove'];
const VIEWS = ['before-after', 'side-by-side'];
const ZOOMS = ['fit', 'actual'];

export function initialState() {
  return {
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
  };
}

/** `value` when it is text, otherwise '' (so a missing name never prints as "undefined"). */
function text(value) {
  return typeof value === 'string' ? value : '';
}

/** The action's fields without its `type`. */
function payload(action) {
  const { type: _type, ...fields } = action;
  return fields;
}

/** A sample can be asked: there is a video whose before clip is ready, and no save runs. */
function canSample(state) {
  return state.video !== null && state.video.beforeReady && state.saving === null;
}

/**
 * `state` with `changes`, and a new request number. A sample is asked for it
 * when one can be, and the status says so.
 */
function asked(state, changes) {
  const next = { ...state, ...changes, request: state.request + 1, keepStatus: false };
  const sampling = canSample(next);
  return { ...next, sampling, status: sampling ? STATUS_SAMPLING : next.status };
}

/** Whether `action` is about the current request of the current video, and a sample is awaited. */
function isCurrentRequest(state, action) {
  return (
    state.sampling &&
    state.video !== null &&
    action.video === state.video.video &&
    action.request === state.request
  );
}

function loadVideo(state, action) {
  const id = action.video;
  if (!Number.isSafeInteger(id) || id < 0) {
    return state;
  }
  return {
    ...state,
    video: { ...payload(action), beforeReady: false },
    sample: null,
    sampling: false,
    zoom: 'fit',
    // A sample of the new video is a new request, above any asked before.
    request: state.request + 1,
    status: action.displayCopy === true ? STATUS_PREPARING : STATUS_SAMPLING,
    keepStatus: false,
  };
}

function beforeReady(state, action) {
  if (state.video === null || action.video !== state.video.video || state.video.beforeReady) {
    return state;
  }
  const next = { ...state, video: { ...state.video, beforeReady: true } };
  const sampling = canSample(next);
  return { ...next, sampling, status: sampling && !next.keepStatus ? STATUS_SAMPLING : next.status };
}

function notice(state, action) {
  switch (action.kind) {
    case 'several-files':
      return { ...state, status: 'One video at a time' };
    case 'saving':
      return { ...state, status: 'Wait for the save to finish, or cancel it' };
    case 'refused':
      return { ...state, status: `${text(action.name)} cannot be used: ${text(action.reason)}` };
    case 'too-large':
      return { ...state, status: `${text(action.name)} is larger than 10 GiB` };
    case 'failed': {
      const status = `The original could not be prepared: ${text(action.reason)}`;
      // A before clip that fails is one that is not ready yet: nothing is
      // awaited then. The backend also sends this notice when it could not
      // load a new video; the current video, its clip and a sample in flight
      // are fine, and the sample must still count.
      const clipFailed = state.video !== null && !state.video.beforeReady;
      return clipFailed ? { ...state, sampling: false, status } : { ...state, status };
    }
    default:
      return state;
  }
}

function sampleArrived(state, action) {
  if (!isCurrentRequest(state, action)) {
    return state;
  }
  const status = state.keepStatus ? state.status : STATUS_SAMPLE_READY;
  return { ...state, sample: payload(action), sampling: false, status, keepStatus: false };
}

function sampleFailed(state, action) {
  if (!isCurrentRequest(state, action)) {
    return state;
  }
  return { ...state, sampling: false, status: `The sample failed: ${text(action.reason)}`, keepStatus: false };
}

function saveStarted(state) {
  if (state.video === null || state.saving !== null) {
    return state;
  }
  return {
    ...state,
    // The backend cancels a running sample. It is asked again when the save
    // ends: when one was running, or when the before clip was not ready yet.
    saving: { percent: null, resume: state.sampling || !state.video.beforeReady },
    sampling: false,
    // A late event of the cancelled sample is now stale.
    request: state.request + 1,
    status: STATUS_SAVING,
    keepStatus: false,
  };
}

function saveProgress(state, action) {
  if (state.saving === null) {
    return state;
  }
  const percent = action.percent;
  const known = Number.isInteger(percent) && percent >= 0 && percent <= 100;
  return {
    ...state,
    saving: { ...state.saving, percent: known ? percent : null },
    status: known ? `Saving… ${percent}%` : STATUS_SAVING,
  };
}

/** The save is over: its controls come back, and a sample it cut short is asked again. */
function endSave(state, status) {
  const resume = state.saving !== null && state.saving.resume;
  const next = { ...state, saving: null, status, keepStatus: true };
  if (!resume || !canSample(next)) {
    return next;
  }
  // The save's own message stays on the status line, also when this sample
  // arrives (`keepStatus`).
  return { ...next, request: next.request + 1, sampling: true };
}

/** "Saved <name> (40.8 MB, 58% smaller)", or the kept form. */
function savedText(action) {
  const name = text(action.name);
  if (action.keptOriginal === true) {
    return `Saved ${name}: original kept: result was not smaller`;
  }
  const size = formatMB(action.bytes);
  const input = action.inputBytes;
  const usable = Number.isFinite(input) && input > 0 && Number.isFinite(action.bytes);
  const percent = usable ? Math.round((1 - action.bytes / input) * 100) : 0;
  return percent > 0 ? `Saved ${name} (${size}, ${percent}% smaller)` : `Saved ${name} (${size})`;
}

/** The slider or Audio changed: store it and ask a new sample. Not while saving, not for the same value. */
function setting(state, key, value, allowed) {
  if (state.saving !== null || !allowed(value) || state[key] === value) {
    return state;
  }
  return asked(state, { [key]: value });
}

/** Stores `value` under `key` when `allowed` accepts it and it is a change. */
function simple(state, key, value, allowed) {
  return allowed(value) && state[key] !== value ? { ...state, [key]: value } : state;
}

const isBoolean = (value) => typeof value === 'boolean';

export function reduce(state, action) {
  if (action === null || typeof action !== 'object') {
    return state;
  }
  switch (action.type) {
    case 'video':
      return loadVideo(state, action);
    case 'before-ready':
      return beforeReady(state, action);
    case 'notice':
      return notice(state, action);
    case 'sample':
      return sampleArrived(state, action);
    case 'sample-failed':
      return sampleFailed(state, action);
    case 'save-progress':
      return saveProgress(state, action);
    case 'save-done':
      return endSave(state, savedText(action));
    case 'save-failed':
      return endSave(state, `Save failed: ${text(action.reason)}`);
    case 'save-cancelled':
      return endSave(state, STATUS_SAVE_CANCELLED);
    case 'crf':
      return setting(state, 'crf', action.value, isCrf);
    case 'audio':
      return setting(state, 'audio', action.value, (value) => AUDIO_CHOICES.includes(value));
    case 'folder':
      return simple(state, 'folder', action.folder, (value) => value === null || typeof value === 'string');
    case 'view':
      return simple(state, 'view', action.view, (value) => VIEWS.includes(value));
    case 'column':
      return simple(state, 'columnOpen', action.open, isBoolean);
    case 'fill':
      return simple(state, 'fill', action.on, isBoolean);
    case 'zoom':
      return simple(state, 'zoom', action.zoom, (value) => ZOOMS.includes(value));
    case 'save-started':
      return saveStarted(state);
    default:
      return state;
  }
}

/**
 * The `make_sample` arguments for the current request, or `null` when there is
 * no video, its before clip is not ready yet, or a save runs (the backend
 * starts no sample then). The window sends it once per request number.
 */
export function sampleRequest(state) {
  if (!canSample(state)) {
    return null;
  }
  return { video: state.video.video, request: state.request, crf: state.crf, audio: state.audio };
}

/** Save needs a video, and no save already running. */
export function canSave(state) {
  return state.video !== null && state.saving === null;
}

/** Quality, Audio, Choose other, Choose folder and dropping are off while a save runs. */
export function controlsDisabled(state) {
  return state.saving !== null;
}
