
"use strict";
// ---------------------------------------------------------------------------
// registries
// ---------------------------------------------------------------------------
var __brows12Pending = {};   // fetchId -> { resolve, reject }
var __brows12Timers = {};    // timerId -> { cb, args, interval, delay }
var __brows12Listeners = {}; // "type\x1fnodeId" -> [cb]
var __brows12NextId = 1;
var __brows12NodeIdSeq = 1000000; // synthetic ids for window/document handles

function __brows12AllocId() { return __brows12NextId++; }

// ---------------------------------------------------------------------------
// fetch
// ---------------------------------------------------------------------------
function fetch(input, init) {
  const url = typeof input === "string" ? input : String(input);
  return new Promise(function (resolve, reject) {
    const id = __brows12AllocId();
    __brows12Pending[id] = { resolve: resolve, reject: reject };
    const method = (init && init.method) ? String(init.method).toUpperCase() : "GET";
    const headers = {};
    if (init && init.headers) {
      for (const k in init.headers) { headers[k] = String(init.headers[k]); }
    }
    __brows12.fetchStart(id, url, method, JSON.stringify(headers));
  });
}

function __brows12FetchDone(id, err, status, headersJson, bodyText) {
  const entry = __brows12Pending[id];
  if (!entry) return;
  delete __brows12Pending[id];
  if (err) { entry.reject(new Error(err)); return; }
  const headers = JSON.parse(headersJson);
  const response = {
    ok: status >= 200 && status < 300,
    status: status,
    statusText: "",
    url: "",
    headers: {
      get: function (name) {
        for (const k in headers) { if (k.toLowerCase() === String(name).toLowerCase()) return headers[k]; }
        return null;
      },
      has: function (name) { return this.get(name) !== null; },
      forEach: function (cb) { for (const k in headers) cb(headers[k], k); }
    },
    text: function () { return Promise.resolve(bodyText); },
    json: function () {
      return new Promise(function (res, rej) {
        try { res(JSON.parse(bodyText)); } catch (e) { rej(e); }
      });
    },
    arrayBuffer: function () {
      const buf = new Uint8Array(bodyText.length);
      for (let i = 0; i < bodyText.length; i++) buf[i] = bodyText.charCodeAt(i) & 0xff;
      return Promise.resolve(buf.buffer);
    }
  };
  entry.resolve(response);
}

// ---------------------------------------------------------------------------
// timers
// ---------------------------------------------------------------------------
function setTimeout(cb, delay) {
  const args = Array.prototype.slice.call(arguments, 2);
  const id = __brows12.timerStart(Number(delay) || 0, false);
  __brows12Timers[id] = { cb: cb, args: args, interval: false, delay: Number(delay) || 0 };
  return id;
}
function setInterval(cb, delay) {
  const args = Array.prototype.slice.call(arguments, 2);
  const id = __brows12.timerStart(Number(delay) || 1, true);
  __brows12Timers[id] = { cb: cb, args: args, interval: true, delay: Number(delay) || 1 };
  return id;
}
function clearTimeout(id) { __brows12.timerCancel(id); delete __brows12Timers[id]; }
function clearInterval(id) { clearTimeout(id); }

function __brows12TimerFire(id) {
  const entry = __brows12Timers[id];
  if (!entry) return;
  if (entry.interval) {
    __brows12.timerReschedule(id, entry.delay);
  } else {
    delete __brows12Timers[id];
  }
  if (typeof entry.cb === "function") entry.cb.apply(null, entry.args);
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------
function __brows12Fmt(v) {
  if (typeof v === "string") return v;
  try { return JSON.stringify(v); } catch (e) { return String(v); }
}
function __brows12MakeConsole(level) {
  return function () {
    const parts = [];
    for (let i = 0; i < arguments.length; i++) parts.push(__brows12Fmt(arguments[i]));
    __brows12.consoleLog(level, parts.join(" "));
  };
}
var console = {
  log: __brows12MakeConsole("log"),
  info: __brows12MakeConsole("info"),
  warn: __brows12MakeConsole("warn"),
  error: __brows12MakeConsole("error"),
  debug: __brows12MakeConsole("debug"),
  trace: __brows12MakeConsole("log"),
  dir: __brows12MakeConsole("log"),
  table: __brows12MakeConsole("log")
};

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------
function __brows12ListenerKey(type, nodeId) { return type + "\x1f" + nodeId; }
function __brows12AddListener(type, nodeId, cb) {
  const key = __brows12ListenerKey(type, nodeId);
  (__brows12Listeners[key] = __brows12Listeners[key] || []).push(cb);
}
function __brows12RemoveListener(type, nodeId, cb) {
  const key = __brows12ListenerKey(type, nodeId);
  const list = __brows12Listeners[key] || [];
  const i = list.indexOf(cb);
  if (i >= 0) list.splice(i, 1);
}
function __brows12Dispatch(type, nodeId, eventInit) {
  const list = (__brows12Listeners[__brows12ListenerKey(type, nodeId)] || []).slice();
  const event = {
    type: type,
    target: null,
    defaultPrevented: false,
    preventDefault: function () { this.defaultPrevented = true; },
    stopPropagation: function () {},
    stopImmediatePropagation: function () {}
  };
  if (eventInit) { for (const k in eventInit) event[k] = eventInit[k]; }
  for (let i = 0; i < list.length; i++) {
    try { list[i].call(event.target || null, event); }
    catch (e) { __brows12.consoleLog("error", "event handler error: " + (e && e.message ? e.message : String(e))); }
  }
  return list.length > 0;
}

// ---------------------------------------------------------------------------
// document / window object model (backed by native fns)
// ---------------------------------------------------------------------------
class Brows12Node {
  constructor(nodeId) {
    this.__nodeId = nodeId;
  }
  get nodeType() { return __brows12.nodeType(this.__nodeId); }
  get parentNode() {
    const id = __brows12.parentNode(this.__nodeId);
    return id === null ? null : new Brows12Node(id);
  }
  appendChild(child) { __brows12.appendChild(this.__nodeId, child.__nodeId); return child; }
  removeChild(child) { __brows12.removeChild(this.__nodeId, child.__nodeId); return child; }
  insertBefore(node, ref) { __brows12.insertBefore(this.__nodeId, node.__nodeId, ref.__nodeId); return node; }
  contains(other) { return __brows12.contains(this.__nodeId, other.__nodeId); }
  hasChildNodes() { return __brows12.childCount(this.__nodeId) > 0; }
  get textContent() { return __brows12.getTextContent(this.__nodeId); }
  set textContent(v) { __brows12.setTextContent(this.__nodeId, String(v)); }
  get innerHTML() { return __brows12.getInnerHtml(this.__nodeId); }
  set innerHTML(v) { __brows12.setInnerHtml(this.__nodeId, String(v)); }
  getAttribute(name) { return __brows12.getAttribute(this.__nodeId, String(name)); }
  setAttribute(name, value) { __brows12.setAttribute(this.__nodeId, String(name), String(value)); }
  removeAttribute(name) { __brows12.removeAttribute(this.__nodeId, String(name)); }
  addEventListener(type, cb) { __brows12AddListener(String(type), this.__nodeId, cb); }
  removeEventListener(type, cb) { __brows12RemoveListener(String(type), this.__nodeId, cb); }
  dispatchEvent(type) { return __brows12Dispatch(String(type), this.__nodeId); }
  click() { __brows12Dispatch("click", this.__nodeId); }
  get classList() {
    const self = this;
    const classes = (__brows12.getAttribute(this.__nodeId, "class") || "").split(/\s+/).filter(Boolean);
    return {
      contains: function (c) { return classes.indexOf(c) >= 0; },
      add: function (c) { if (classes.indexOf(c) < 0) { classes.push(c); __brows12.setAttribute(self.__nodeId, "class", classes.join(" ")); } },
      remove: function (c) { const i = classes.indexOf(c); if (i >= 0) { classes.splice(i, 1); __brows12.setAttribute(self.__nodeId, "class", classes.join(" ")); } },
      toggle: function (c) { if (classes.indexOf(c) >= 0) { this.remove(c); return false; } this.add(c); return true; }
    };
  }
  styleSet(prop, value) { __brows12.styleSet(this.__nodeId, prop, value); }
}

class Brows12Element extends Brows12Node {
  get tagName() { return (__brows12.tagName(this.__nodeId) || "").toUpperCase(); }
  get id() { return __brows12.getAttribute(this.__nodeId, "id") || ""; }
  set id(v) { __brows12.setAttribute(this.__nodeId, "id", String(v)); }
  get className() { return __brows12.getAttribute(this.__nodeId, "class") || ""; }
  set className(v) { __brows12.setAttribute(this.__nodeId, "class", String(v)); }
  querySelector(sel) {
    const id = __brows12.querySelector(this.__nodeId, String(sel));
    return id === null ? null : new Brows12Element(id);
  }
  querySelectorAll(sel) {
    return __brows12.querySelectorAll(this.__nodeId, String(sel)).map(function (id) { return new Brows12Element(id); });
  }
  get children() {
    return __brows12.childElements(this.__nodeId).map(function (id) { return new Brows12Element(id); });
  }
  get firstElementChild() {
    const id = __brows12.firstElementChild(this.__nodeId);
    return id === null ? null : new Brows12Element(id);
  }
}

class Brows12Document extends Brows12Node {
  get documentElement() {
    const id = __brows12.documentElement();
    return id === null ? null : new Brows12Element(id);
  }
  get body() {
    const id = __brows12.body();
    return id === null ? null : new Brows12Element(id);
  }
  get head() {
    const id = __brows12.head();
    return id === null ? null : new Brows12Element(id);
  }
  get title() { return __brows12.title(); }
  set title(v) { __brows12.setTitle(String(v)); }
  createElement(tag) { return new Brows12Element(__brows12.createElement(String(tag))); }
  createTextNode(text) { return new Brows12Node(__brows12.createTextNode(String(text))); }
  createComment(text) { return new Brows12Node(__brows12.createComment(String(text))); }
  getElementById(id) {
    const n = __brows12.getElementById(String(id));
    return n === null ? null : new Brows12Element(n);
  }
  getElementsByTagName(tag) {
    return __brows12.getElementsByTagName(String(tag)).map(function (id) { return new Brows12Element(id); });
  }
  querySelector(sel) {
    const id = __brows12.querySelector(this.__nodeId, String(sel));
    return id === null ? null : new Brows12Element(id);
  }
  querySelectorAll(sel) {
    return __brows12.querySelectorAll(this.__nodeId, String(sel)).map(function (id) { return new Brows12Element(id); });
  }
  addEventListener(type, cb) { __brows12AddListener(String(type), -1, cb); }
  removeEventListener(type, cb) { __brows12RemoveListener(String(type), -1, cb); }
  get cookie() { return __brows12.cookieGet(); }
  set cookie(v) { __brows12.cookieSet(String(v)); }
}

var document = new Brows12Document(0);
var window = globalThis;
window.document = document;
window.setTimeout = setTimeout;
window.setInterval = setInterval;
window.clearTimeout = clearTimeout;
window.clearInterval = clearInterval;
window.fetch = fetch;
window.addEventListener = function (type, cb) { __brows12AddListener(String(type), -1, cb); };
window.removeEventListener = function (type, cb) { __brows12RemoveListener(String(type), -1, cb); };
window.requestAnimationFrame = function (cb) { return setTimeout(function () { cb(Date.now()); }, 0); };
window.btoa = function (s) { return __brows12.btoa(String(s)); };
window.atob = function (s) { return __brows12.atob(String(s)); };
window.getComputedStyle = function (el) {
  const json = __brows12.computedStyleJson(el.__nodeId);
  const obj = JSON.parse(json);
  return { getPropertyValue: function (p) { return obj[p] || ""; } };
};
window.history = { pushState: function(){}, replaceState: function(){}, back: function(){}, forward: function(){} };
window.alert = function (msg) { __brows12.consoleLog("alert", String(msg)); };
window.matchMedia = function () { return { matches: false, addListener: function(){}, removeListener: function(){} }; };

// location + navigator are built from environment data at realm creation.
var location = (function () {
  const parts = JSON.parse(__brows12.locationJson());
  const o = {};
  for (const k in parts) o[k] = parts[k];
  o.assign = function () {}; o.replace = function () {}; o.reload = function () {};
  return o;
})();
var navigator = JSON.parse(__brows12.navigatorJson());

// storage
function __brows12MakeStorage(native) {
  return {
    getItem: function (k) { const v = native.get(String(k)); return v == null ? null : v; },
    setItem: function (k, v) { native.set(String(k), String(v)); },
    removeItem: function (k) { native.remove(String(k)); },
    clear: function () { native.clear(); },
    key: function (i) { return native.key(i | 0); },
    get length() { return native.length(); }
  };
}
var localStorage = __brows12MakeStorage({
  get: function (k) { return __brows12.storageGet(k); },
  set: function (k, v) { __brows12.storageSet(k, v); },
  remove: function (k) { __brows12.storageRemove(k); },
  clear: function () { __brows12.storageClear(); },
  key: function (i) { return __brows12.storageKey(i); },
  length: function () { return __brows12.storageLength(); }
});
var sessionStorage = __brows12MakeStorage({
  get: function (k) { return null; },
  set: function () {},
  remove: function () {},
  clear: function () {},
  key: function () { return null; },
  length: function () { return 0; }
});

// crypto (entropy for tests; engine can restrict under strict FP policy)
var crypto = {
  getRandomValues: function (arr) {
    for (let i = 0; i < arr.length; i++) arr[i] = __brows12.randByte();
    return arr;
  }
};

function __brows12FireLoad() {
  __brows12Dispatch("DOMContentLoaded", -1);
  __brows12Dispatch("load", -1);
}
