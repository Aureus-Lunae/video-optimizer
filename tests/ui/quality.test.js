import test from 'node:test';
import assert from 'node:assert/strict';

import { CRF_DEFAULT, CRF_MAX, CRF_MIN, crfValueText, crfWord, isCrf, readCrf } from '../../ui/quality.js';

test('the range is 20 to 36 and starts at 28', () => {
  assert.equal(CRF_MIN, 20);
  assert.equal(CRF_MAX, 36);
  assert.equal(CRF_DEFAULT, 28);
});

test('readCrf accepts 20, 28 and 36 from the range input string', () => {
  assert.equal(readCrf('20'), 20);
  assert.equal(readCrf('28'), 28);
  assert.equal(readCrf('36'), 36);
  // A whole number is accepted as it is.
  assert.equal(readCrf(28), 28);
});

test('readCrf refuses a value outside 20 to 36, a fraction, and anything that is not a number', () => {
  for (const value of ['19', '37', '28.5', '', 'abc', ' 28', '28 ', '028', '0', '-1', '51', NaN, ['28'], null, undefined, 19, 37, 28.5, Infinity, {}]) {
    assert.equal(readCrf(value), null, String(JSON.stringify(value) ?? value));
  }
  // Control: the values next to the refused ones are taken.
  assert.equal(readCrf('21'), 21);
  assert.equal(readCrf('35'), 35);
});

test('isCrf is true for a whole number from 20 to 36 only', () => {
  for (let crf = 20; crf <= 36; crf += 1) {
    assert.equal(isCrf(crf), true, String(crf));
  }
  for (const value of [19, 37, 28.5, '28', NaN, [28], null, undefined, -1, 0, 51]) {
    assert.equal(isCrf(value), false, String(JSON.stringify(value) ?? value));
  }
});

test("crfWord gives the mock-up's words at 20, 23, 26, 28, 30 and 36", () => {
  assert.equal(crfWord(20), 'Near lossless, very big');
  assert.equal(crfWord(22), 'Near lossless, very big');
  assert.equal(crfWord(23), 'High quality');
  assert.equal(crfWord(24), 'High quality');
  assert.equal(crfWord(25), 'Good');
  assert.equal(crfWord(26), 'Good');
  assert.equal(crfWord(27), 'Good');
  assert.equal(crfWord(28), 'Recommended for the web');
  assert.equal(crfWord(29), 'Smaller');
  assert.equal(crfWord(30), 'Smaller');
  assert.equal(crfWord(31), 'Smallest, check the picture');
  assert.equal(crfWord(36), 'Smallest, check the picture');
});

test('crfWord gives no word for a value the slider cannot give', () => {
  for (const value of [19, 37, 28.5, '28', NaN, null, undefined, [28]]) {
    assert.equal(crfWord(value), '', String(JSON.stringify(value) ?? value));
  }
});

test('crfValueText at 28 is "CRF 28, Recommended for the web"', () => {
  assert.equal(crfValueText(28), 'CRF 28, Recommended for the web');
  assert.equal(crfValueText(20), 'CRF 20, Near lossless, very big');
  assert.equal(crfValueText(36), 'CRF 36, Smallest, check the picture');
});

test('crfValueText is empty for a value the slider cannot give', () => {
  for (const value of [19, 37, 28.5, '28', NaN, null, undefined]) {
    assert.equal(crfValueText(value), '', String(value));
  }
});
