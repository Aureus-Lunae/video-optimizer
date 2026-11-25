// The quality slider's range and its words, as pure functions. No DOM here,
// so the node tests can load it; the window writes the text into the page.
//
// Quality is the x264 CRF: a whole number from 20 to 36. The backend refuses
// anything else, so the window never sends it either.

export const CRF_MIN = 20;
export const CRF_MAX = 36;
export const CRF_DEFAULT = 28;

/** Whether `value` is a number the slider can give: a whole number from 20 to 36. */
export function isCrf(value) {
  return Number.isInteger(value) && value >= CRF_MIN && value <= CRF_MAX;
}

/**
 * The CRF in the range input's `value` (a string such as "28"), or `null`
 * when it is not a whole number from 20 to 36. A number is accepted as it is.
 * Only plain digits count: " 28", "028", "28.5" and "" give `null`.
 */
export function readCrf(value) {
  const crf = typeof value === 'string' && /^[1-9]\d?$/.test(value) ? Number(value) : value;
  return isCrf(crf) ? crf : null;
}

/** The word for a CRF, as in the mock-up. '' for a value that is not a CRF. */
export function crfWord(crf) {
  if (!isCrf(crf)) {
    return '';
  }
  if (crf <= 22) {
    return 'Near lossless, very big';
  }
  if (crf <= 24) {
    return 'High quality';
  }
  if (crf <= 27) {
    return 'Good';
  }
  if (crf === 28) {
    return 'Recommended for the web';
  }
  if (crf <= 30) {
    return 'Smaller';
  }
  return 'Smallest, check the picture';
}

/** The slider's spoken value (its `aria-valuetext`): "CRF 28, Recommended for the web". '' for a value that is not a CRF. */
export function crfValueText(crf) {
  return isCrf(crf) ? `CRF ${crf}, ${crfWord(crf)}` : '';
}
