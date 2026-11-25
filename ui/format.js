// Pure text formatting for the figures in the column, and the text of a
// refusal. No DOM here, so the node tests can load it. A figure that is not known shows as "unknown", and
// "no sound" as "None": the window never prints NaN or "undefined".

const MB = 1_000_000;
const GB = 1_000_000_000;

function isNonNegative(value) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

/** Decimal sizes, as macOS and Windows show them: "26.9 MB", "0.4 MB", "1.2 GB". */
export function formatMB(bytes) {
  if (!isNonNegative(bytes)) {
    return 'unknown';
  }
  const mb = (bytes / MB).toFixed(1);
  // 999 960 000 B would read "1000.0 MB", a unit too small, so it moves to GB.
  if (bytes < GB && Number(mb) < 1000) {
    return `${mb} MB`;
  }
  return `${(bytes / GB).toFixed(1)} GB`;
}

/**
 * "0:36", "1:02", "1:02:03": whole seconds that have passed, as a player
 * counts them (59.9 s is "0:59").
 */
export function formatLength(seconds) {
  if (!isNonNegative(seconds)) {
    return 'unknown';
  }
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = String(total % 60).padStart(2, '0');
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, '0')}:${rest}`;
  }
  return `${minutes}:${rest}`;
}

/** "24 fps", "29.97 fps": two decimals at most, none for a whole rate. */
export function formatFps(fps) {
  if (!isNonNegative(fps) || fps === 0) {
    return 'unknown';
  }
  return `${Number(fps.toFixed(2))} fps`;
}

/** "AAC, 317 kbps"; "AAC" when the bitrate is not known; "None" for a video without sound. */
export function formatAudio(audio) {
  if (audio === null || audio === undefined) {
    return 'None';
  }
  if (typeof audio.codec !== 'string' || audio.codec === '') {
    return 'unknown';
  }
  const codec = audio.codec.toUpperCase();
  const bitrate = audio.bitrate;
  if (typeof bitrate !== 'number' || !Number.isFinite(bitrate) || bitrate <= 0) {
    return codec;
  }
  return `${codec}, ${Math.round(bitrate / 1000)} kbps`;
}

/** "42.4 dB"; "identical" when the pictures are (`identical` is exactly `true`); "unknown" when it could not be measured. */
export function formatPsnr(psnr, identical) {
  if (identical === true) {
    return 'identical';
  }
  if (!isNonNegative(psnr)) {
    return 'unknown';
  }
  return `${psnr.toFixed(1)} dB`;
}

/** The transport's text: "0:03 / 0:10". */
export function formatPosition(seconds, total) {
  return `${formatLength(seconds)} / ${formatLength(total)}`;
}

/** The text of a refusal: Tauri rejects a command with its error string, a failed fetch with an Error. */
export function reasonText(reason) {
  if (typeof reason === 'string') {
    return reason;
  }
  return reason instanceof Error ? reason.message : String(reason);
}
