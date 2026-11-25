// The Video optimizer window's entry: starts the window (window.js) with the
// real page, Tauri API, timers, ResizeObserver and console. Every value is
// handed on as it is; the wiring itself lives in window.js, where the node
// tests drive it with stand-ins. The window stores nothing between launches.

import { startWindow } from './window.js';

startWindow({
  doc: document,
  tauri: window.__TAURI__,
  timers: { setTimer: setTimeout, clearTimer: clearTimeout },
  ResizeObserver: window.ResizeObserver,
  log: (...values) => console.error(...values),
});
