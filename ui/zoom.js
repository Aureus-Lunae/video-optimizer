// Zoom and pan maths for the compare picture as pure functions. No DOM here,
// so the node tests can load it; the window writes the result into the page.
// Copied from Lunaepix's ui/zoom.js and trimmed to what this app uses: the
// zoom limits, the "100%" and "Fit" buttons, panning and the divider.
//
// A view is `{ scale, x, y }`. `x` and `y` are how far the picture is moved
// from the centred position in the area, in pixels, so `{ scale: fit, x: 0,
// y: 0 }` is centred without knowing the area. The window shows it as
// `translate(x px, y px) scale(scale)` on a picture that sits centred in the
// area at its own size, with `transform-origin: center`.
//
// `image` is `{ width, height, svg? }`: the pixel size, or for an SVG the
// size it decoded to with `svg: true`. `area` is `{ width, height }` in
// pixels.
//
// Every function that moves the view returns a new view, except a call that
// changes nothing (a view scale that cannot be used), which returns the view
// it was given; none changes the view it is given.

const MAX_ZOOM = 8;
const DIVIDER_START = 50;

function finiteOr(value, fallback) {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function isPositive(value) {
  return typeof value === 'number' && Number.isFinite(value) && value > 0;
}

/** A `{ width, height }` that can be divided by and shown at. */
export function isUsableSize(size) {
  return size !== null && typeof size === 'object' && isPositive(size.width) && isPositive(size.height);
}

function clamp(value, low, high) {
  return Math.min(Math.max(value, low), high);
}

/**
 * The scale range. A raster image fits the area but is never enlarged above
 * 100%, and zooms up to 800%. An SVG has no 100%: it fits the area (which may
 * enlarge it) and zooms up to 8 times that. A size that cannot be used
 * (zero, missing, not a number) gives the neutral range 1 to 8, so no NaN
 * reaches the page; the real range follows once both sizes are known.
 */
export function zoomLimits(image, area) {
  if (!isUsableSize(image) || !isUsableSize(area)) {
    return { fit: 1, max: MAX_ZOOM };
  }
  const toArea = Math.min(area.width / image.width, area.height / image.height);
  if (image.svg === true) {
    return { fit: toArea, max: MAX_ZOOM * toArea };
  }
  return { fit: Math.min(1, toArea), max: MAX_ZOOM };
}

/**
 * How far the picture may be moved from the centre on one axis: not past the
 * area's edge, and not at all when the picture is not bigger than the area.
 * The `+ 0` turns a `-0` into `0`.
 */
function limitOffset(offset, shown, room) {
  const half = Math.max(0, (shown - room) / 2);
  return clamp(offset, -half, half) + 0;
}

/** The view with its offsets limited to the area. Without usable sizes it is centred. */
function bound(scale, x, y, image, area) {
  if (!isUsableSize(image) || !isUsableSize(area)) {
    return { scale, x: 0, y: 0 };
  }
  return {
    scale,
    x: limitOffset(x, image.width * scale, area.width),
    y: limitOffset(y, image.height * scale, area.height),
  };
}

/**
 * The "100%" button: one image pixel per screen pixel (a scale of exactly 1),
 * about the centre of the area, so the spot at the centre stays there. An SVG
 * has no 100%, so it goes to Fit. A view whose scale is not a positive number
 * is returned as it is.
 */
export function toActual(view, limits, image, area) {
  if (image !== null && typeof image === 'object' && image.svg === true) {
    return toFit(limits);
  }
  if (!isPositive(view.scale)) {
    return view;
  }
  // The scale is the target itself, not a product, so 1 stays exactly 1.
  const scale = clamp(1, limits.fit, limits.max);
  const ratio = scale / view.scale;
  return bound(scale, view.x * ratio, view.y * ratio, image, area);
}

/** The "Fit" button: the whole picture, centred. */
export function toFit(limits) {
  return { scale: limits.fit, x: 0, y: 0 };
}

/**
 * Move the picture by `dx`, `dy` pixels (a drag or an arrow key). The picture's
 * edge never passes the area's edge, and a picture smaller than the area stays
 * centred. A delta that is not a number counts as 0.
 */
export function pan(view, dx, dy, image, area) {
  return bound(view.scale, view.x + finiteOr(dx, 0), view.y + finiteOr(dy, 0), image, area);
}

/** The divider position in percent, within 0 to 100. A value that is not a number gives the middle. */
export function divider(value) {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return DIVIDER_START;
  }
  return clamp(value, 0, 100);
}
