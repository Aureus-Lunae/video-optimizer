// The comparison's two small calculations, as pure functions. No DOM here, so
// the node tests can load it; the window applies the results.

import { divider } from './zoom.js';

/** The frame rate to use when the file does not say one. */
const DEFAULT_FPS = 30;

function isTime(value) {
  return typeof value === 'number' && Number.isFinite(value);
}

/**
 * Where to seek the sample so it follows the original: the original's time
 * when the two are more than one frame apart, otherwise `null` (leave the
 * sample alone, as a seek on every frame would stutter). One frame is
 * `1 / fps`; a rate that is not known, or not above 0, counts as 30 fps. A
 * time that is not a number gives `null`.
 */
export function driftCorrection(originalTime, sampleTime, fps) {
  if (!isTime(originalTime) || !isTime(sampleTime)) {
    return null;
  }
  const rate = isTime(fps) && fps > 0 ? fps : DEFAULT_FPS;
  return Math.abs(originalTime - sampleTime) > 1 / rate ? originalTime : null;
}

/**
 * The `clip-path` of the sample layer for a divider at `value` percent: the
 * sample shows from the divider to the right. 0 shows only the sample, 100
 * only the original; a value outside 0 to 100 is clamped. A value that is not
 * a number gives `null`, so the page keeps the clip it has.
 */
export function dividerClip(value) {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return null;
  }
  return `inset(0 0 0 ${divider(value)}%)`;
}
