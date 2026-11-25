// A recording stand-in for `document`, for the node tests of the window
// (window.js, through fake-window.js's page). Copied from Lunaepix and
// extended with what a `<video>` does (`play`, `pause`, `load`,
// `currentTime`, `paused`, `muted`, `duration`, `ended`, `error`), `inert`,
// `disabled`, and listeners on the document itself. It creates plain
// objects, records every element it creates, and records any write that
// would put markup into the page. Node has no DOM, so this is what lets the
// tests see which elements are made, which were moved, where focus went,
// which style properties were written, which pointer was captured, which
// video was played or seeked and whether an event was prevented. Where the
// DOM has a default that matters (`valueAsNumber` is NaN for an empty field,
// a video starts paused with an unknown duration), the fake has the same one.

/**
 * A style object that records every write: plain properties
 * (`style.transform = …`) and custom properties (`setProperty`,
 * `removeProperty`), in order, as `[name, value]` (value null for a removal).
 */
function fakeStyle() {
  const writes = [];
  const values = new Map();
  const target = {
    writes,
    setProperty(name, value) {
      writes.push([name, String(value)]);
      values.set(name, String(value));
    },
    removeProperty(name) {
      writes.push([name, null]);
      values.delete(name);
    },
    getPropertyValue(name) {
      return values.get(name) ?? '';
    },
  };
  return new Proxy(target, {
    set(object, name, value) {
      writes.push([String(name), String(value)]);
      values.set(String(name), String(value));
      return true;
    },
    get(object, name) {
      if (name in object) {
        return object[name];
      }
      return values.get(String(name)) ?? '';
    },
  });
}

/**
 * Runs the handlers in `listeners` added for `type` with one event:
 * `event`'s fields, `target` as its target unless `event` names another, and
 * a `preventDefault()` that records itself in `defaultPrevented`. Returns that
 * event, so a test can see whether a handler prevented it.
 */
function fireOn(listeners, target, type, event) {
  const full = {
    type,
    target,
    defaultPrevented: false,
    preventDefault() {
      full.defaultPrevented = true;
    },
    ...event,
  };
  for (const [listened, handler] of listeners) {
    if (listened === type) {
      handler(full);
    }
  }
  return full;
}

export function fakeDocument() {
  const created = [];
  const markupWrites = [];
  // `strictFocus` makes `focus()` behave as a browser does: an element that is
  // hidden or disabled, or sits under a hidden or inert element, cannot take
  // focus. A refused focus is recorded and leaves `activeElement` where it was.
  // `refusePlay` makes every `play()` reject, as a webview does for media it
  // cannot play.
  const doc = {
    created,
    markupWrites,
    activeElement: null,
    strictFocus: false,
    refusedFocus: [],
    refusePlay: false,
    listeners: [],
  };

  // Listeners on the document itself (a key pressed anywhere in the page).
  doc.addEventListener = (type, handler) => {
    doc.listeners.push([type, handler]);
  };
  doc.fire = (type, event = {}) => fireOn(doc.listeners, doc, type, event);

  class FakeElement {
    constructor(tagName) {
      this.tagName = tagName;
      this.children = [];
      this.classes = [];
      this.text = '';
      this.textWrites = 0;
      this.hidden = false;
      this.parent = null;
      this.attributes = new Map();
      this.listeners = [];
      this.valueText = '';
      this.valueWrites = 0;
      this.style = fakeStyle();
      // The layout box a test gives the element (the DOM measures it).
      this.rect = { left: 0, top: 0, width: 0, height: 0 };
      this.clientWidth = 0;
      this.clientHeight = 0;
      // The pointer ids captured with `setPointerCapture`.
      this.captured = [];
      this.type = '';
      this.inert = false;
      this.disabled = false;
      // A `<video>`: paused, nothing loaded, at the start. `plays`, `pauses`
      // and `loads` count the calls; `seeks` lists every `currentTime` written.
      this.paused = true;
      this.ended = false;
      this.muted = false;
      this.duration = Number.NaN;
      this.time = 0;
      this.seeks = [];
      this.plays = 0;
      this.pauses = 0;
      this.loads = 0;
      // `error` is the MediaError a video holds after its `error` event (null
      // while none): a test sets `{ code }` and fires 'error'. `srcWrites`
      // lists every address written to the `src` attribute, in order.
      this.error = null;
      this.srcWrites = [];
      const classes = this.classes;
      this.classList = {
        add: (...names) => {
          for (const name of names) {
            if (!classes.includes(name)) {
              classes.push(name);
            }
          }
        },
        remove: (...names) => {
          for (const name of names) {
            const at = classes.indexOf(name);
            if (at >= 0) {
              classes.splice(at, 1);
            }
          }
        },
        toggle: (name, force) => {
          const on = force === undefined ? !classes.includes(name) : Boolean(force);
          if (on) {
            this.classList.add(name);
          } else {
            this.classList.remove(name);
          }
          return on;
        },
        contains: (name) => classes.includes(name),
        [Symbol.iterator]: () => classes.slice()[Symbol.iterator](),
      };
    }

    // As in the DOM, writing the text replaces the children. Reading it gives
    // the element's own text only (the DOM would add its children's text).
    get textContent() {
      return this.text;
    }

    set textContent(value) {
      this.text = String(value);
      this.textWrites += 1;
      this.children = [];
    }

    get value() {
      return this.valueText;
    }

    set value(text) {
      this.valueText = String(text);
      this.valueWrites += 1;
    }

    // As in the DOM: the number a range or number input holds, NaN for an
    // empty one and for any other kind of element.
    get valueAsNumber() {
      if ((this.type !== 'range' && this.type !== 'number') || this.valueText.trim() === '') {
        return Number.NaN;
      }
      return Number(this.valueText);
    }

    // As in the DOM: the playback position of a `<video>`; writing it seeks.
    get currentTime() {
      return this.time;
    }

    set currentTime(seconds) {
      this.time = Number(seconds);
      this.seeks.push(this.time);
    }

    // As in the DOM, `play()` answers with a promise; it rejects (and the
    // video stays paused) when the document says `refusePlay`.
    play() {
      this.plays += 1;
      if (doc.refusePlay) {
        return Promise.reject(new Error('NotSupportedError'));
      }
      this.paused = false;
      return Promise.resolve();
    }

    pause() {
      this.pauses += 1;
      this.paused = true;
    }

    load() {
      this.loads += 1;
    }

    getBoundingClientRect() {
      return { ...this.rect };
    }

    setPointerCapture(pointerId) {
      this.captured.push(pointerId);
    }

    // As in the DOM, a node that is put somewhere else leaves its old parent.
    adopt(nodes) {
      for (const node of nodes) {
        if (node.parent !== null) {
          const siblings = node.parent.children;
          const at = siblings.indexOf(node);
          if (at >= 0) {
            siblings.splice(at, 1);
          }
        }
        node.parent = this;
      }
    }

    append(...nodes) {
      this.adopt(nodes);
      this.children.push(...nodes);
    }

    prepend(...nodes) {
      this.adopt(nodes);
      this.children.unshift(...nodes);
    }

    replaceChildren(...nodes) {
      this.text = '';
      for (const child of this.children) {
        child.parent = null;
      }
      this.children = [];
      this.adopt(nodes);
      this.children = [...nodes];
    }

    replaceWith(node) {
      const index = this.parent.children.indexOf(this);
      if (node.parent !== null && node.parent !== this.parent) {
        node.parent.children.splice(node.parent.children.indexOf(node), 1);
      }
      this.parent.children[index] = node;
      node.parent = this.parent;
      this.parent = null;
    }

    contains(node) {
      for (let current = node; current; current = current.parent) {
        if (current === this) {
          return true;
        }
      }
      return false;
    }

    focus() {
      if (doc.strictFocus) {
        let refused = this.disabled;
        for (let current = this; current; current = current.parent) {
          refused = refused || current.hidden || current.inert;
        }
        if (refused) {
          doc.refusedFocus.push(this);
          return;
        }
      }
      doc.activeElement = this;
    }

    addEventListener(type, handler) {
      this.listeners.push([type, handler]);
    }

    // Runs the handlers added for `type` with one event (see `fireOn`).
    fire(type, event = {}) {
      return fireOn(this.listeners, this, type, event);
    }

    // What a click on the element runs: the handlers added for 'click'.
    click() {
      this.fire('click');
    }

    setAttribute(name, value) {
      if (name === 'src') {
        this.srcWrites.push(String(value));
      }
      this.attributes.set(name, String(value));
    }

    removeAttribute(name) {
      this.attributes.delete(name);
    }

    getAttribute(name) {
      return this.attributes.has(name) ? this.attributes.get(name) : null;
    }

    set innerHTML(value) {
      markupWrites.push(['innerHTML', value]);
    }

    set outerHTML(value) {
      markupWrites.push(['outerHTML', value]);
    }

    insertAdjacentHTML(position, value) {
      markupWrites.push(['insertAdjacentHTML', value]);
    }
  }

  doc.FakeElement = FakeElement;
  doc.createElement = (tagName) => {
    const element = new FakeElement(tagName.toLowerCase());
    created.push(element);
    return element;
  };
  return doc;
}

/** Every element in the tree under `element`, itself included. */
export function tree(element) {
  return [element, ...element.children.flatMap(tree)];
}
