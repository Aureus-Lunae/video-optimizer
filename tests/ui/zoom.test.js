// Copied from Lunaepix's tests/ui/zoom.test.js, trimmed to the functions this
// app keeps: zoomLimits, isUsableSize, pan, toActual, toFit and divider.

import test from 'node:test';
import assert from 'node:assert/strict';

import { divider, isUsableSize, pan, toActual, toFit, zoomLimits } from '../../ui/zoom.js';

// Positions are computed with floating point; they are compared within this.
const EPSILON = 1e-9;

const AREA = { width: 800, height: 600 };
const CENTRE = { x: 400, y: 300 };
const BIG = { width: 4000, height: 3000 };
const BIG_LIMITS = zoomLimits(BIG, AREA); // fit 0.2, max 8

function near(actual, expected, message) {
  assert.ok(Math.abs(actual - expected) < EPSILON, `${message ?? ''} expected ${expected}, got ${actual}`);
}

/** The image point (from the picture's centre) that is under `point` in `view`. */
function spotUnder(view, point, area) {
  return {
    x: (point.x - area.width / 2 - view.x) / view.scale,
    y: (point.y - area.height / 2 - view.y) / view.scale,
  };
}

function assertSameSpot(before, after, message) {
  near(after.x, before.x, `${message} x:`);
  near(after.y, before.y, `${message} y:`);
}

// zoomLimits

test('fit of a wide, a tall and a 1x1 image is never above 1', () => {
  const wide = zoomLimits({ width: 4000, height: 1000 }, AREA);
  near(wide.fit, 0.2, 'wide');
  const tall = zoomLimits({ width: 1000, height: 4000 }, AREA);
  near(tall.fit, 0.15, 'tall');
  // A 1x1 image is not enlarged to fill the area.
  assert.equal(zoomLimits({ width: 1, height: 1 }, AREA).fit, 1);
  assert.notEqual(zoomLimits({ width: 1, height: 1 }, AREA).fit, 600);
  assert.equal(zoomLimits({ width: 100, height: 50 }, AREA).fit, 1);
  for (const image of [{ width: 4000, height: 1000 }, { width: 1000, height: 4000 }, { width: 1, height: 1 }]) {
    assert.equal(zoomLimits(image, AREA).max, 8);
  }
});

test('an svg fits and may go up to 8 times fit', () => {
  const wide = zoomLimits({ width: 100, height: 50, svg: true }, AREA);
  near(wide.fit, 8, 'width-bound fit');
  near(wide.max, 64, 'width-bound max');
  const tall = zoomLimits({ width: 100, height: 200, svg: true }, AREA);
  near(tall.fit, 3, 'height-bound fit');
  near(tall.max, 24, 'height-bound max');
  // The same size as a raster is never enlarged, and tops out at 8.
  assert.deepEqual(zoomLimits({ width: 100, height: 50 }, AREA), { fit: 1, max: 8 });
});

test('limits are neutral for an unusable image or area', () => {
  const bad = [
    { width: 0, height: 10 },
    { width: 10, height: 0 },
    { width: null, height: null },
    { width: NaN, height: 5 },
    { width: -5, height: 5 },
    { width: Infinity, height: 5 },
    null,
    undefined,
  ];
  for (const value of bad) {
    assert.deepEqual(zoomLimits(value, AREA), { fit: 1, max: 8 }, `image ${JSON.stringify(value)}`);
    assert.deepEqual(zoomLimits(BIG, value), { fit: 1, max: 8 }, `area ${JSON.stringify(value)}`);
  }
  assert.deepEqual(zoomLimits(null, null), { fit: 1, max: 8 });
  // An SVG whose decoded size is not known yet is neutral too, not Infinity.
  assert.deepEqual(zoomLimits({ width: null, height: null, svg: true }, AREA), { fit: 1, max: 8 });
  // A usable pair does not.
  assert.notDeepEqual(zoomLimits(BIG, AREA), { fit: 1, max: 8 });
});

test('isUsableSize accepts a positive finite width and height', () => {
  assert.equal(isUsableSize(BIG), true);
  assert.equal(isUsableSize({ width: 0.5, height: 1, svg: true }), true);
});

test('isUsableSize rejects a zero, negative, missing or non-number side, and no size', () => {
  const bad = [
    { width: 0, height: 10 },
    { width: 10, height: 0 },
    { width: null, height: null },
    { width: NaN, height: 5 },
    { width: -5, height: 5 },
    { width: Infinity, height: 5 },
    { width: '800', height: '600' },
    { height: 5 },
    null,
    undefined,
    800,
  ];
  for (const value of bad) {
    assert.equal(isUsableSize(value), false, JSON.stringify(value));
  }
});

// pan

test('pan clamps at each edge', () => {
  const view = { scale: 1, x: 0, y: 0 }; // 4000 x 3000 shown in 800 x 600
  near(pan(view, 5000, 0, BIG, AREA).x, 1600, 'right');
  near(pan(view, -5000, 0, BIG, AREA).x, -1600, 'left');
  near(pan(view, 0, 5000, BIG, AREA).y, 1200, 'bottom');
  near(pan(view, 0, -5000, BIG, AREA).y, -1200, 'top');
  // Inside the limits it moves exactly by the delta.
  assert.deepEqual(pan(view, 30, -20, BIG, AREA), { scale: 1, x: 30, y: -20 });
});

test('pan centres a picture smaller than the area', () => {
  const small = { width: 400, height: 300 };
  assert.deepEqual(pan({ scale: 1, x: 0, y: 0 }, 100, -100, small, AREA), { scale: 1, x: 0, y: 0 });
  // Wide only: x is free up to its bound, y stays centred.
  const wide = pan({ scale: 1, x: 0, y: 0 }, 5000, 5000, { width: 4000, height: 300 }, AREA);
  near(wide.x, 1600, 'wide x');
  assert.equal(wide.y, 0);
  // Tall only: y is free up to its bound, x stays centred.
  const tall = pan({ scale: 1, x: 0, y: 0 }, 5000, 5000, { width: 400, height: 3000 }, AREA);
  assert.equal(tall.x, 0);
  near(tall.y, 1200, 'tall y');
});

test('pan ignores a delta that is not a number', () => {
  const view = { scale: 1, x: 10, y: 20 };
  assert.deepEqual(pan(view, NaN, 5, BIG, AREA), { scale: 1, x: 10, y: 25 });
  assert.deepEqual(pan(view, 5, undefined, BIG, AREA), { scale: 1, x: 15, y: 20 });
  assert.deepEqual(pan(view, 5, 5, BIG, AREA), { scale: 1, x: 15, y: 25 });
});

test('pan with an unusable image or area gives no offset', () => {
  const view = { scale: 1, x: 50, y: 50 };
  assert.deepEqual(pan(view, 10, 10, null, AREA), { scale: 1, x: 0, y: 0 });
  assert.deepEqual(pan(view, 10, 10, BIG, { width: 0, height: 0 }), { scale: 1, x: 0, y: 0 });
  assert.deepEqual(pan(view, 10, 10, BIG, AREA), { scale: 1, x: 60, y: 60 });
});

// toActual and toFit

test('100 percent gives scale 1 for a raster', () => {
  const zoomed = { scale: 2, x: 100, y: 50 };
  const actual = toActual(zoomed, BIG_LIMITS, BIG, AREA);
  near(actual.scale, 1, 'zoomed in');
  assertSameSpot(spotUnder(zoomed, CENTRE, AREA), spotUnder(actual, CENTRE, AREA), 'centre spot');
  const fromFit = toActual(toFit(BIG_LIMITS), BIG_LIMITS, BIG, AREA);
  near(fromFit.scale, 1, 'from Fit');
  near(fromFit.x, 0, 'centred x');
  near(fromFit.y, 0, 'centred y');

  // An SVG has no 100%: the button gives Fit.
  const svg = { width: 100, height: 50, svg: true };
  const svgLimits = zoomLimits(svg, AREA);
  assert.deepEqual(toActual({ scale: 32, x: 10, y: 10 }, svgLimits, svg, AREA), { scale: 8, x: 0, y: 0 });
});

test('100 percent gives exactly scale 1 for a raster', () => {
  // 0.203 * (1 / 0.203) is 0.9999999999999999 in floating point.
  assert.notEqual(0.203 * (1 / 0.203), 1);
  for (const scale of [0.203, 0.2, 2, 7.3, 8]) {
    const actual = toActual({ scale, x: 40, y: -30 }, BIG_LIMITS, BIG, AREA);
    assert.equal(actual.scale, 1, `from ${scale}`);
  }
  // The spot at the centre stays there.
  const from = { scale: 0.203, x: 40, y: -30 };
  assertSameSpot(spotUnder(from, CENTRE, AREA), spotUnder(toActual(from, BIG_LIMITS, BIG, AREA), CENTRE, AREA), '0.203');
  // A view without a usable scale is left as it is.
  const broken = { scale: 0, x: 0, y: 0 };
  assert.deepEqual(toActual(broken, BIG_LIMITS, BIG, AREA), broken);
  // An SVG still goes to Fit, not 1.
  const svg = { width: 100, height: 50, svg: true };
  assert.notEqual(toActual({ scale: 0.203, x: 0, y: 0 }, zoomLimits(svg, AREA), svg, AREA).scale, 1);
});

test('100 percent keeps the picture inside the area, and centres it without a usable area', () => {
  // At 4x and panned past what 100% allows (8000 / 4 = 2000, bound 1600), the offset stops at the bound.
  const edge = { scale: 4, x: 8000, y: 0 };
  assert.deepEqual(toActual(edge, BIG_LIMITS, BIG, AREA), { scale: 1, x: 1600, y: 0 });
  assert.deepEqual(toActual({ scale: 2, x: 100, y: 50 }, BIG_LIMITS, BIG, { width: 0, height: 0 }), {
    scale: 1,
    x: 0,
    y: 0,
  });
});

test('fit returns the fit view', () => {
  assert.deepEqual(toFit({ fit: 0.2, max: 8 }), { scale: 0.2, x: 0, y: 0 });
  assert.deepEqual(toFit({ fit: 3, max: 24 }), { scale: 3, x: 0, y: 0 });
  assert.notEqual(toFit({ fit: 0.2, max: 8 }).scale, 8);
});

// divider

test('divider clamps below 0 and above 100', () => {
  assert.equal(divider(-5), 0);
  assert.equal(divider(105), 100);
  assert.equal(divider(0), 0);
  assert.equal(divider(100), 100);
  assert.equal(divider(37), 37);
  assert.equal(divider(37.5), 37.5);
});

test('divider gives the middle for a value that is not a number', () => {
  for (const junk of [NaN, undefined, '30', null, Infinity]) {
    assert.equal(divider(junk), 50, String(junk));
  }
  assert.equal(divider(30), 30);
});
