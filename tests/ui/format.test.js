import test from 'node:test';
import assert from 'node:assert/strict';

import { formatAudio, formatFps, formatLength, formatMB, formatPosition, formatPsnr } from '../../ui/format.js';

const NOT_A_NUMBER = [null, undefined, NaN, -1, Infinity, '5', [5], {}];

test('formatMB gives decimal megabytes with one decimal', () => {
  assert.equal(formatMB(26_916_667), '26.9 MB');
  assert.equal(formatMB(96_900_000), '96.9 MB');
  assert.equal(formatMB(1_000_000), '1.0 MB');
  assert.equal(formatMB(0), '0.0 MB');
});

test('formatMB below 1 MB keeps one decimal, and from 1000 MB it goes to GB', () => {
  assert.equal(formatMB(400_000), '0.4 MB');
  assert.equal(formatMB(999_000_000), '999.0 MB');
  assert.equal(formatMB(1_000_000_000), '1.0 GB');
  assert.equal(formatMB(1_234_000_000), '1.2 GB');
  // 999.96 MB would read "1000.0 MB", a unit too small.
  assert.equal(formatMB(999_960_000), '1.0 GB');
});

test('formatMB shows unknown for a value that is not a size', () => {
  for (const value of NOT_A_NUMBER) {
    assert.equal(formatMB(value), 'unknown', String(JSON.stringify(value) ?? value));
  }
});

test('formatLength gives minutes and seconds, with hours from one hour', () => {
  assert.equal(formatLength(36.04), '0:36');
  assert.equal(formatLength(0), '0:00');
  assert.equal(formatLength(62), '1:02');
  assert.equal(formatLength(3723), '1:02:03');
  assert.equal(formatLength(3600), '1:00:00');
  // A player counts whole seconds that have passed.
  assert.equal(formatLength(59.9), '0:59');
});

test('formatLength shows unknown for a value that is not a time', () => {
  for (const value of NOT_A_NUMBER) {
    assert.equal(formatLength(value), 'unknown', String(JSON.stringify(value) ?? value));
  }
});

test('formatFps gives a whole rate as it is and a fractional one with two decimals', () => {
  assert.equal(formatFps(24), '24 fps');
  assert.equal(formatFps(29.97002997), '29.97 fps');
  assert.equal(formatFps(23.976023976), '23.98 fps');
  assert.equal(formatFps(25.0), '25 fps');
});

test('formatFps shows unknown for a value that is not a rate', () => {
  for (const value of [...NOT_A_NUMBER, 0]) {
    assert.equal(formatFps(value), 'unknown', String(JSON.stringify(value) ?? value));
  }
});

test('formatAudio gives the codec in capitals and the bitrate in kbps', () => {
  assert.equal(formatAudio({ codec: 'aac', bitrate: 317_000 }), 'AAC, 317 kbps');
  assert.equal(formatAudio({ codec: 'opus', bitrate: 128_400 }), 'OPUS, 128 kbps');
});

test('formatAudio without a bitrate gives the codec only', () => {
  for (const bitrate of [null, undefined, NaN, 0, -5, '317000']) {
    assert.equal(formatAudio({ codec: 'aac', bitrate }), 'AAC', String(bitrate));
  }
});

test('formatAudio gives None for no audio, and unknown for an audio without a codec', () => {
  assert.equal(formatAudio(null), 'None');
  assert.equal(formatAudio(undefined), 'None');
  assert.equal(formatAudio({ bitrate: 100_000 }), 'unknown');
  assert.equal(formatAudio({ codec: '', bitrate: 100_000 }), 'unknown');
  assert.equal(formatAudio({ codec: 7, bitrate: 100_000 }), 'unknown');
});

test('formatPsnr gives decibels with one decimal, and identical when the pictures are', () => {
  assert.equal(formatPsnr(42.41, false), '42.4 dB');
  assert.equal(formatPsnr(30, false), '30.0 dB');
  assert.equal(formatPsnr(null, true), 'identical');
  assert.equal(formatPsnr(42.41, true), 'identical');
});

test('formatPsnr shows unknown for a value that is not a measure', () => {
  for (const value of [null, undefined, NaN, -1, Infinity, '42', [42]]) {
    assert.equal(formatPsnr(value, false), 'unknown', String(JSON.stringify(value) ?? value));
  }
  // Only an explicit true means identical.
  assert.equal(formatPsnr(null, 'true'), 'unknown');
  assert.equal(formatPsnr(null, undefined), 'unknown');
});

test('formatPosition gives the position and the total', () => {
  assert.equal(formatPosition(3, 10), '0:03 / 0:10');
  assert.equal(formatPosition(0, 10), '0:00 / 0:10');
  assert.equal(formatPosition(3.7, 10), '0:03 / 0:10');
});

test('formatPosition shows unknown for the part that is not a time', () => {
  assert.equal(formatPosition(null, 10), 'unknown / 0:10');
  assert.equal(formatPosition(3, NaN), '0:03 / unknown');
});
