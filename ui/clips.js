// How the two <video> elements are given their clip. The window first sets
// the clip's own address (`media://…`, `http://media.localhost/…`), which
// macOS and Windows play directly. WebKitGTK on Linux cannot: its media
// player loads only http, https, blob and file addresses, and reports the
// scheme as unsupported (error code 4). The first element that reports that
// makes every clip from then on go the other way: the same address is
// fetched, and the bytes are played from a `blob:` address. Nothing sniffs
// the platform; the webview says what it cannot do.
//
// No DOM here beyond the element it is given, and `fetch` and the object-URL
// functions come in as arguments, so the node tests drive it with stand-ins.
//
// An object URL holds its bytes in memory until it is revoked, so each
// element keeps at most one, revokes it when its source is replaced (not
// before: the old clip plays until the new one is there) or removed, and
// `dispose` revokes whatever is left when the page goes away. A scheme
// address is never an object URL and is never revoked.

import { reasonText } from './format.js';

/** `MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED`: the webview cannot play the address it was given. */
export const MEDIA_ERR_SRC_NOT_SUPPORTED = 4;

/**
 * `fetch`: the page's fetch; `urls`: `{ createObjectURL, revokeObjectURL }`.
 * Returns `{ attach(video), dispose() }`. `attach` gives `{ show, clear }`
 * for one element:
 *
 * - `show(address, { onSet, onFailed })` plays the clip at `address`.
 *   `onSet()` runs each time the element has been given a source for it
 *   (at once for the scheme address; once the bytes are there for a blob
 *   address, where a new source starts at 0 just the same). `onFailed(text)`
 *   runs when the bytes could not be fetched or read; the element then keeps
 *   what it had. A newer `show` or a `clear` makes everything still pending
 *   for the older one void: it makes no object URL and reports nothing.
 * - `clear()` pauses the element and removes its source.
 */
export function createClipSources({ fetch, urls }) {
  // Set by the first element that cannot play the scheme; shared, so a clip
  // on the other element goes straight to the blob path.
  let needsBlobs = false;
  const releases = [];

  function attach(video) {
    // The object URL the element plays, if any; the clip it was last asked to
    // show (`null` once cleared); how that clip's source was set; and a count
    // that moves on whenever the clip changes, so a late answer can tell.
    let objectUrl = null;
    let current = null;
    let via = null;
    let generation = 0;

    function revoke() {
      if (objectUrl !== null) {
        urls.revokeObjectURL(objectUrl);
        objectUrl = null;
      }
    }

    function setSource(address, made) {
      video.setAttribute('src', address);
      // The element has moved to the new address, so the old bytes can go.
      const old = objectUrl;
      objectUrl = made ? address : null;
      if (old !== null) {
        urls.revokeObjectURL(old);
      }
    }

    /** Fetches the clip's address and plays the bytes from a blob address, unless the clip changed meanwhile. */
    function viaBlob(clip, mine) {
      via = 'blob';
      fetch(clip.address)
        .then((response) => {
          if (!response.ok) {
            throw new Error(`the clip could not be fetched (status ${response.status})`);
          }
          return response.blob();
        })
        .then(
          (blob) => {
            if (mine !== generation) {
              return;
            }
            setSource(urls.createObjectURL(blob), true);
            clip.onSet();
          },
          // Only the fetch and the reading of its bytes end here; a fault in
          // `onSet` is not a failed fetch and is left to be seen.
          (reason) => {
            if (mine === generation) {
              clip.onFailed(reasonText(reason));
            }
          },
        );
    }

    video.addEventListener('error', () => {
      const code = video.error === null || video.error === undefined ? null : video.error.code;
      // Only the scheme address of a clip still shown, and only once.
      if (code !== MEDIA_ERR_SRC_NOT_SUPPORTED || current === null || via !== 'direct') {
        return;
      }
      needsBlobs = true;
      viaBlob(current, generation);
    });

    const element = {
      show(address, { onSet = () => {}, onFailed = () => {} } = {}) {
        generation += 1;
        current = { address, onSet, onFailed };
        if (needsBlobs) {
          viaBlob(current, generation);
          return;
        }
        via = 'direct';
        setSource(address, false);
        onSet();
      },
      clear() {
        generation += 1;
        current = null;
        via = null;
        video.pause();
        video.removeAttribute('src');
        video.load();
        revoke();
      },
    };
    releases.push(revoke);
    return element;
  }

  return {
    attach,
    /** The page is going away: every object URL still held is revoked. */
    dispose() {
      for (const release of releases) {
        release();
      }
    },
  };
}
