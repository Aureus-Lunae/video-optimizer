// Stand-ins for what the window's start function (ui/window.js) is given:
// the page, the Tauri API, the timers and ResizeObserver. Copied from
// Lunaepix and extended. They record what the window does with them, and are
// as strict as the real ones where a mistake would only show in the app: a
// timer called with the wrong `this` throws, as setTimeout does in a webview;
// asking the page for an id it does not have throws instead of giving null.
//
// Unlike Lunaepix's, the page is not a hand-kept list: it is built from
// ui/index.html itself, every element in its nesting with its attributes,
// classes and own text. So the tests run the window on the real markup, and a
// test that reads an attribute (the slider's `min`, a video's `preload`)
// reads what the app ships.

import { readFileSync } from 'node:fs';

import { fakeDocument } from './fake-document.js';

/** The elements that have no closing tag. */
const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);

/** The page's own frame: the fake document has its own root and body, so these tags are skipped. */
const PAGE_TAGS = ['html', 'head', 'body'];

const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };

function decodeEntities(text) {
  return text.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, name) => {
    if (name.startsWith('#x') || name.startsWith('#X')) {
      return String.fromCodePoint(Number.parseInt(name.slice(2), 16));
    }
    if (name.startsWith('#')) {
      return String.fromCodePoint(Number(name.slice(1)));
    }
    if (!Object.hasOwn(ENTITIES, name)) {
      throw new Error(`unknown entity ${whole}`);
    }
    return ENTITIES[name];
  });
}

/** `[name, value]` for each attribute in a start tag's attribute text; a bare attribute has the value ''. */
function attributesOf(text) {
  const found = [];
  const pattern = /([^\s=/>]+)(?:="([^"]*)")?/g;
  for (const match of text.matchAll(pattern)) {
    found.push([match[1].toLowerCase(), decodeEntities(match[2] ?? '')]);
  }
  return found;
}

/**
 * Builds the elements of `html` under `body`, strictly: an end tag that does
 * not close the innermost open element, or an element left open, throws, so
 * a broken page fails the tests. Comments and the doctype are skipped; text
 * between tags (whitespace collapsed) is added to the open element's own text.
 * Returns every element that has an id, by id.
 */
function buildPage(doc, body, html) {
  const byId = new Map();
  const open = [body];
  const token = /<!--[\s\S]*?-->|<!doctype[^>]*>|<\/([a-z][\w-]*)\s*>|<([a-z][\w-]*)((?:\s+[^\s=>]+(?:="[^"]*")?)*)\s*\/?>|([^<]+)/gi;
  let consumed = 0;
  for (const match of html.matchAll(token)) {
    if (match.index !== consumed) {
      throw new Error(`index.html could not be read at ${consumed}: ${html.slice(consumed, consumed + 40)}`);
    }
    consumed = match.index + match[0].length;
    const [, endTag, startTag, attributeText, text] = match;
    const current = open[open.length - 1];
    if (text !== undefined) {
      const words = decodeEntities(text).replace(/\s+/g, ' ').trim();
      if (words !== '') {
        current.text = current.text === '' ? words : `${current.text} ${words}`;
      }
    } else if (endTag !== undefined) {
      if (PAGE_TAGS.includes(endTag.toLowerCase())) {
        continue;
      }
      if (current.tagName !== endTag.toLowerCase()) {
        throw new Error(`</${endTag}> closes <${current.tagName}>`);
      }
      open.pop();
    } else if (startTag !== undefined) {
      const element = makeElement(doc, startTag.toLowerCase(), attributesOf(attributeText));
      if (element.id !== undefined) {
        if (byId.has(element.id)) {
          throw new Error(`the id ${element.id} is used twice`);
        }
        byId.set(element.id, element);
      }
      // html, head and body are the fake's own; their content goes under body.
      if (PAGE_TAGS.includes(element.tagName)) {
        continue;
      }
      current.append(element);
      if (!VOID.has(element.tagName)) {
        open.push(element);
      }
    }
  }
  if (consumed !== html.length) {
    throw new Error('index.html has text the reader did not take');
  }
  // Every element opened must be closed by now.
  if (open.length !== 1) {
    throw new Error(`<${open[open.length - 1].tagName}> is never closed`);
  }
  return byId;
}

/** One element with its attributes, and the properties the DOM derives from them. */
function makeElement(doc, tag, attributes) {
  const element = doc.createElement(tag);
  for (const [name, value] of attributes) {
    element.setAttribute(name, value);
  }
  const has = (name) => element.getAttribute(name) !== null;
  if (has('id')) {
    element.id = element.getAttribute('id');
  }
  if (has('class')) {
    element.classList.add(...element.getAttribute('class').split(/\s+/).filter((name) => name !== ''));
  }
  element.type = element.getAttribute('type') ?? '';
  element.valueText = element.getAttribute('value') ?? '';
  element.hidden = has('hidden');
  element.disabled = has('disabled');
  element.muted = has('muted');
  return element;
}

/** A `<select>`'s value, as the DOM gives it: the selected option's, or the first one's. */
function settleSelects(doc) {
  for (const select of doc.created.filter((element) => element.tagName === 'select')) {
    const options = select.children.filter((child) => child.tagName === 'option');
    const chosen = options.find((option) => option.getAttribute('selected') !== null) ?? options[0];
    if (chosen !== undefined) {
      select.valueText = chosen.getAttribute('value') ?? chosen.text;
    }
  }
}

/**
 * The page: the shared fake document with every element of ui/index.html
 * (or of `html`, for a test of this reader), in its place under `body` and
 * the root element (`documentElement`). `byId(id)` gives an element to the
 * test; `getElementById` is what the window calls, and it throws for an id
 * the page does not have.
 */
export function fakePage(html = readFileSync(new URL('../../ui/index.html', import.meta.url), 'utf8')) {
  const doc = fakeDocument();
  const root = doc.createElement('html');
  const body = doc.createElement('body');
  root.append(body);
  doc.documentElement = root;
  doc.body = body;
  const elements = buildPage(doc, body, html);
  settleSelects(doc);
  doc.byId = (id) => {
    if (!elements.has(id)) {
      throw new Error(`the test asks for ${id}, which the page does not have`);
    }
    return elements.get(id);
  };
  doc.getElementById = (id) => {
    if (!elements.has(id)) {
      throw new Error(`the page has no element with the id ${id}`);
    }
    return elements.get(id);
  };
  return doc;
}

/**
 * The Tauri API as `window.__TAURI__` has it. `invoke` records each call and
 * resolves with `answers[command]` (null when it has none), or rejects with
 * `refuse[command]`; a command named in `hold` waits instead until the test
 * answers it: `held` lists each such call as { command, args, resolve, reject }.
 * `listen` records each subscription, or rejects for a name in
 * `refuseListen`; `emit(type, payload)` runs the handler subscribed to
 * `type`, as the backend's event would, and throws when nobody listens.
 */
export function fakeTauri({ refuse = {}, refuseListen = [], hold = [], answers = {} } = {}) {
  const calls = [];
  const held = [];
  const handlers = new Map();
  return {
    calls,
    held,
    handlers,
    core: {
      invoke(command, args) {
        calls.push([command, args]);
        if (hold.includes(command)) {
          return new Promise((resolve, reject) => {
            held.push({ command, args, resolve, reject });
          });
        }
        if (Object.hasOwn(refuse, command)) {
          return Promise.reject(refuse[command]);
        }
        return Promise.resolve(Object.hasOwn(answers, command) ? answers[command] : null);
      },
      convertFileSrc(key, scheme) {
        return `${scheme}://localhost/${key}`;
      },
    },
    event: {
      listen(type, handler) {
        if (refuseListen.includes(type)) {
          return Promise.reject(new Error(`no ${type}`));
        }
        handlers.set(type, handler);
        return Promise.resolve(() => handlers.delete(type));
      },
    },
    emit(type, payload) {
      const handler = handlers.get(type);
      if (handler === undefined) {
        throw new Error(`nobody listens to ${type}`);
      }
      handler({ event: type, id: 1, payload });
    },
    commands() {
      return calls.map(([command]) => command);
    },
  };
}

function refuseThis(self) {
  // A webview's setTimeout and clearTimeout throw when called as a method of
  // another object.
  if (self !== undefined && self !== globalThis) {
    throw new TypeError('Illegal invocation');
  }
}

/**
 * setTimeout and clearTimeout on a clock the test moves (`advance(ms)` runs
 * every timer that is due, in order). Strict as a webview: calling either
 * with a `this` other than undefined or globalThis throws.
 * `set`: every timer set, as { id, fn, delay }; `cleared`: the ids cleared.
 */
export function strictTimers() {
  let now = 0;
  const set = [];
  const cleared = [];
  const due = new Map();
  function setTimer(fn, delay) {
    refuseThis(this);
    const id = set.length + 1;
    set.push({ id, fn, delay });
    due.set(id, { fn, at: now + delay });
    return id;
  }
  function clearTimer(id) {
    refuseThis(this);
    cleared.push(id);
    due.delete(id);
  }
  function advance(ms) {
    now += ms;
    const ready = [...due].filter(([, timer]) => timer.at <= now).sort((a, b) => a[1].at - b[1].at);
    for (const [id, timer] of ready) {
      due.delete(id);
      timer.fn();
    }
  }
  return { set, cleared, setTimer, clearTimer, advance };
}

/** A ResizeObserver class; `resize(element)` runs the callback of every observer watching it. */
export function fakeResizeObservers() {
  const observers = [];
  class FakeResizeObserver {
    constructor(callback) {
      this.callback = callback;
      this.targets = [];
      observers.push(this);
    }

    observe(target) {
      this.targets.push(target);
    }
  }
  return {
    observers,
    ResizeObserver: FakeResizeObserver,
    resize(element) {
      for (const observer of observers) {
        if (observer.targets.includes(element)) {
          observer.callback([{ target: element }], observer);
        }
      }
    },
  };
}

/** Lets every promise that is already settled run its handlers. */
export const settle = () => new Promise((resolve) => setImmediate(resolve));
