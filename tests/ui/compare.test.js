import test from 'node:test';
import assert from 'node:assert/strict';

import { dividerClip, driftCorrection } from '../../ui/compare.js';

test('driftCorrection seeks the sample to the original time when they are 0.05 s apart at 24 fps', () => {
  assert.equal(driftCorrection(3.05, 3.0, 24), 3.05);
  // The sample ahead of the original is corrected too.
  assert.equal(driftCorrection(3.0, 3.05, 24), 3.0);
});

test('driftCorrection does not seek when they are 0.02 s apart at 24 fps', () => {
  assert.equal(driftCorrection(3.02, 3.0, 24), null);
  assert.equal(driftCorrection(3.0, 3.0, 24), null);
  assert.equal(driftCorrection(3.0, 3.02, 24), null);
});

test('driftCorrection allows a drift of up to one frame and no more', () => {
  // 1 / 16 is 0.0625, exact in binary: exactly one frame is left alone, a little more is not.
  assert.equal(driftCorrection(3.0625, 3.0, 16), null);
  assert.equal(driftCorrection(3.07, 3.0, 16), 3.07);
});

test('driftCorrection uses 30 fps when the rate is not known', () => {
  // One frame at 30 fps is 0.0333 s: 0.03 is left alone, 0.04 is corrected.
  for (const fps of [null, undefined, NaN, 0, -24, '24']) {
    assert.equal(driftCorrection(3.03, 3.0, fps), null, `0.03 s with fps ${String(fps)}`);
    assert.equal(driftCorrection(3.04, 3.0, fps), 3.04, `0.04 s with fps ${String(fps)}`);
  }
  // Control: at 24 fps (0.0417 s) 0.04 is left alone, so 30 was what decided above.
  assert.equal(driftCorrection(3.04, 3.0, 24), null);
});

test('driftCorrection does not seek for a time that is not a number', () => {
  for (const time of [null, undefined, NaN, Infinity, '3']) {
    assert.equal(driftCorrection(time, 3.0, 24), null, `original ${String(time)}`);
    assert.equal(driftCorrection(3.0, time, 24), null, `sample ${String(time)}`);
  }
});

test('dividerClip keeps the sample layer from the divider to the right', () => {
  assert.equal(dividerClip(0), 'inset(0 0 0 0%)');
  assert.equal(dividerClip(50), 'inset(0 0 0 50%)');
  assert.equal(dividerClip(100), 'inset(0 0 0 100%)');
  assert.equal(dividerClip(37.5), 'inset(0 0 0 37.5%)');
});

test('dividerClip clamps a value outside 0 to 100', () => {
  assert.equal(dividerClip(-1), 'inset(0 0 0 0%)');
  assert.equal(dividerClip(101), 'inset(0 0 0 100%)');
});

test('dividerClip gives null for a value that is not a number, so the page keeps its last clip', () => {
  for (const value of ['abc', '50', '', NaN, Infinity, -Infinity, null, undefined, [50], {}]) {
    assert.equal(dividerClip(value), null, String(JSON.stringify(value) ?? value));
  }
});
