
"use strict";
// ---------------------------------------------------------------------------
// registries
// ---------------------------------------------------------------------------
var __brows12Pending = {};   // fetchId -> { resolve, reject }
var __brows12Timers = {};    // timerId -> { cb, args, interval, delay }
var __brows12Listeners = {}; // "type\x1fnodeId" -> [cb]
var __brows12NextId = 1;
var __brows12NodeIdSeq = 1000000; // synthetic ids for window/document handles
var __brows12WsInstances = {}; // wsId -> WebSocket instance
var __brows12WorkerInstances = {}; // workerId -> Worker instance

function __brows12AllocId() { return __brows12NextId++; }

// ---------------------------------------------------------------------------
// fetch
// ---------------------------------------------------------------------------
function fetch(input, init) {
  let url = typeof input === "string" ? input : String(input);
  if (url.startsWith("/")) url = location.origin + url;
  return new Promise(function (resolve, reject) {
    const id = __brows12AllocId();
    __brows12Pending[id] = { resolve: resolve, reject: reject };
    const method = (init && init.method) ? String(init.method).toUpperCase() : "GET";
    const headers = {};
    if (init && init.headers) {
      for (const k in init.headers) { headers[k] = String(init.headers[k]); }
    }
    const body = (init && init.body != null) ? String(init.body) : "";
    __brows12.fetchStart(id, url, method, JSON.stringify(headers), body);
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
// XMLHttpRequest (spec subset, async only — built on the fetch machinery)
// ---------------------------------------------------------------------------
class XMLHttpRequest {
  static get UNSENT() { return 0; }
  static get OPENED() { return 1; }
  static get HEADERS_RECEIVED() { return 2; }
  static get LOADING() { return 3; }
  static get DONE() { return 4; }

  constructor() {
    this.readyState = 0;
    this.status = 0;
    this.statusText = "";
    this.responseText = "";
    this.response = "";
    this.responseType = "text";
    this.onreadystatechange = null;
    this.onload = null;
    this.onerror = null;
    this.onabort = null;
    this.__reqHeaders = {};
    this.__respHeaders = {};
    this.__method = "GET";
    this.__url = "";
    this.__aborted = false;
  }

  open(method, url /* , async — always async */) {
    this.__method = String(method || "GET").toUpperCase();
    this.__url = String(url);
    this.__setReady(1);
  }

  setRequestHeader(name, value) { this.__reqHeaders[String(name)] = String(value); }

  __setReady(n) {
    this.readyState = n;
    if (typeof this.onreadystatechange === "function") {
      try { this.onreadystatechange.call(this, { type: "readystatechange" }); } catch (e) { console.error(e); }
    }
  }

  send(body) {
    const xhr = this;
    const init = { method: this.__method, headers: this.__reqHeaders };
    if (body != null) init.body = String(body);
    fetch(this.__url, init).then(function (resp) {
      if (xhr.__aborted) return;
      xhr.status = resp.status;
      xhr.statusText = resp.statusText || "";
      xhr.__respHeaders = {};
      resp.headers.forEach(function (v, k) { xhr.__respHeaders[String(k).toLowerCase()] = v; });
      xhr.__setReady(2);
      xhr.__setReady(3);
      return resp.text().then(function (text) {
        if (xhr.__aborted) return;
        xhr.responseText = text;
        if (xhr.responseType === "json") {
          try { xhr.response = JSON.parse(text); } catch (e) { xhr.response = null; }
        } else {
          xhr.response = text;
        }
        xhr.__setReady(4);
        if (typeof xhr.onload === "function") { try { xhr.onload.call(xhr, { type: "load" }); } catch (e) { console.error(e); } }
      });
    }).catch(function (e) {
      if (xhr.__aborted) return;
      xhr.status = 0;
      xhr.__setReady(4);
      if (typeof xhr.onerror === "function") {
        try { xhr.onerror.call(xhr, { type: "error", message: e && e.message ? e.message : String(e) }); } catch (err) { console.error(err); }
      }
    });
  }

  abort() {
    this.__aborted = true;
    if (typeof this.onabort === "function") { try { this.onabort.call(this, { type: "abort" }); } catch (e) {} }
  }

  getResponseHeader(name) {
    const v = this.__respHeaders[String(name).toLowerCase()];
    return v === undefined ? null : v;
  }

  getAllResponseHeaders() {
    let out = "";
    for (const k in this.__respHeaders) out += k + ": " + this.__respHeaders[k] + "\r\n";
    return out;
  }

  overrideMimeType() {}
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------
class WebSocket {
  constructor(url, protocols) {
    this.url = String(url);
    this.readyState = 0; // CONNECTING
    this.onopen = null;
    this.onmessage = null;
    this.onerror = null;
    this.onclose = null;
    this.__id = __brows12AllocId();
    this.__listeners = {};
    __brows12WsInstances[this.__id] = this;
    let protos = [];
    if (Array.isArray(protocols)) protos = protocols.map(String);
    else if (typeof protocols === "string") protos = [protocols];
    __brows12.wsOpen(this.__id, this.url, JSON.stringify(protos));
  }

  get binaryType() { return "blob"; }
  get bufferedAmount() { return 0; }
  get extensions() { return ""; }
  get protocol() { return ""; }

  send(data) {
    if (this.readyState !== 1) throw new Error("WebSocket is not open");
    __brows12.wsSend(this.__id, String(data));
  }

  close(code, reason) {
    if (this.readyState === 2 || this.readyState === 3) return;
    this.readyState = 2; // CLOSING
    __brows12.wsClose(this.__id, Number(code) || 1000, String(reason || ""));
  }

  addEventListener(type, cb) {
    (this.__listeners[type] = this.__listeners[type] || []).push(cb);
  }

  removeEventListener(type, cb) {
    const list = this.__listeners[type] || [];
    const i = list.indexOf(cb);
    if (i >= 0) list.splice(i, 1);
  }

  __fire(type, event) {
    const list = (this.__listeners[type] || []).slice();
    for (let i = 0; i < list.length; i++) {
      try { list[i].call(this, event); } catch (e) { console.error("ws listener: " + e); }
    }
  }
}
WebSocket.CONNECTING = 0;
WebSocket.OPEN = 1;
WebSocket.CLOSING = 2;
WebSocket.CLOSED = 3;

function __brows12WsEvent(id, kind, dataJson) {
  const ws = __brows12WsInstances[id];
  if (!ws) return;
  let data = dataJson;
  try { data = JSON.parse(dataJson); } catch (e) { /* keep raw */ }
  if (kind === "open") {
    ws.readyState = 1;
    ws.__fire("open", { type: "open" });
    if (typeof ws.onopen === "function") { try { ws.onopen.call(ws, { type: "open" }); } catch (e) { console.error(e); } }
  } else if (kind === "message") {
    const ev = { type: "message", data: data };
    ws.__fire("message", ev);
    if (typeof ws.onmessage === "function") { try { ws.onmessage.call(ws, ev); } catch (e) { console.error(e); } }
  } else if (kind === "error") {
    const ev = { type: "error", message: data };
    ws.__fire("error", ev);
    if (typeof ws.onerror === "function") { try { ws.onerror.call(ws, ev); } catch (e) { console.error(e); } }
  } else if (kind === "close") {
    ws.readyState = 3;
    delete __brows12WsInstances[id];
    const ev = { type: "close", code: 1000, reason: "" };
    ws.__fire("close", ev);
    if (typeof ws.onclose === "function") { try { ws.onclose.call(ws, ev); } catch (e) { console.error(e); } }
  }
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
// crypto
// ---------------------------------------------------------------------------
var crypto = {
  getRandomValues: function (arr) {
    for (let i = 0; i < arr.length; i++) arr[i] = __brows12.randByte();
    return arr;
  }
};

// ---------------------------------------------------------------------------
// location + navigator (built from environment data at realm creation)
// ---------------------------------------------------------------------------
var location = (function () {
  const parts = JSON.parse(__brows12.locationJson());
  const o = {};
  for (const k in parts) o[k] = parts[k];
  o.assign = function () {}; o.replace = function () {}; o.reload = function () {};
  return o;
})();
var navigator = JSON.parse(__brows12.navigatorJson());

// ===========================================================================
// WORKER REALM: DedicatedWorkerGlobalScope, nothing DOM-related.
// ===========================================================================
if (__brows12.isWorker) {
  var self = globalThis;
  self.postMessage = function (data) { __brows12.workerEmit(JSON.stringify(data)); };
  self.close = function () {};
  var __brows12WorkerScope = { onmessage: null, listeners: [] };
  Object.defineProperty(self, "onmessage", {
    set: function (cb) { __brows12WorkerScope.onmessage = cb; },
    get: function () { return __brows12WorkerScope.onmessage; }
  });
  self.addEventListener = function (type, cb) {
    if (String(type) === "message") __brows12WorkerScope.listeners.push(cb);
  };
  self.removeEventListener = function (type, cb) {
    if (String(type) !== "message") return;
    const i = __brows12WorkerScope.listeners.indexOf(cb);
    if (i >= 0) __brows12WorkerScope.listeners.splice(i, 1);
  };
  // Deliver a message posted from the creating realm. Assigned to
  // globalThis explicitly: strict-mode function declarations inside blocks
  // are block-scoped and would otherwise not survive this eval.
  globalThis.__brows12WorkerDeliver = function (json) {
    let data;
    try { data = JSON.parse(json); } catch (e) { data = json; }
    const ev = { type: "message", data: data, target: self };
    if (typeof __brows12WorkerScope.onmessage === "function") {
      try { __brows12WorkerScope.onmessage.call(self, ev); } catch (e) { console.error("worker onmessage: " + e); }
    }
    const listeners = __brows12WorkerScope.listeners.slice();
    for (let i = 0; i < listeners.length; i++) {
      try { listeners[i].call(self, ev); } catch (e) { console.error("worker listener: " + e); }
    }
  };
  self.XMLHttpRequest = XMLHttpRequest;
  self.WebSocket = WebSocket;
  self.fetch = fetch;
  self.setTimeout = setTimeout;
  self.clearTimeout = clearTimeout;
  self.setInterval = setInterval;
  self.clearInterval = clearInterval;
  self.console = console;
  self.location = location;
  self.navigator = navigator;
  self.crypto = crypto;
  globalThis.window = self; // legacy scripts sometimes reference window inside workers
}

// ===========================================================================
// PAGE REALM: DOM bindings, window, storage, Worker constructor.
// ===========================================================================
if (!__brows12.isWorker) {

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
  window.XMLHttpRequest = XMLHttpRequest;
  window.WebSocket = WebSocket;
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

  // ---------------------------------------------------------------------------
  // workers (constructor lives on the page side)
  // ---------------------------------------------------------------------------
  class Worker {
    constructor(url) {
      this.onmessage = null;
      this.onerror = null;
      this.__id = __brows12AllocId();
      this.__listeners = {};
      __brows12WorkerInstances[this.__id] = this;
      let resolved = String(url);
      if (resolved.startsWith("/")) resolved = location.origin + resolved;
      __brows12.workerNew(this.__id, resolved);
    }
    postMessage(data) { __brows12.workerPost(this.__id, JSON.stringify(data)); }
    terminate() {
      __brows12.workerTerminate(this.__id);
      delete __brows12WorkerInstances[this.__id];
    }
    addEventListener(type, cb) {
      (this.__listeners[type] = this.__listeners[type] || []).push(cb);
    }
    removeEventListener(type, cb) {
      const list = this.__listeners[type] || [];
      const i = list.indexOf(cb);
      if (i >= 0) list.splice(i, 1);
    }
    __fire(type, ev) {
      const list = (this.__listeners[type] || []).slice();
      for (let i = 0; i < list.length; i++) {
        try { list[i].call(this, ev); } catch (e) { console.error("worker listener: " + e); }
      }
    }
  }
  window.Worker = Worker;

  // A message arrived from a worker realm.
  globalThis.__brows12WorkerMessage = function (id, json) {
    const w = __brows12WorkerInstances[id];
    if (!w) return;
    let data;
    try { data = JSON.parse(json); } catch (e) { data = json; }
    const ev = { type: "message", data: data, target: w };
    w.__fire("message", ev);
    if (typeof w.onmessage === "function") {
      try { w.onmessage.call(w, ev); } catch (e) { console.error("worker onmessage: " + e); }
    }
  };

  // A worker failed to load / evaluate its script.
  globalThis.__brows12WorkerError = function (id, json) {
    const w = __brows12WorkerInstances[id];
    if (!w) return;
    let message = json;
    try { message = JSON.parse(json); } catch (e) { /* keep raw */ }
    w.__fire("error", { type: "error", message: message });
    if (typeof w.onerror === "function") {
      try { w.onerror.call(w, { type: "error", message: message }); } catch (e) { console.error(e); }
    }
  };
}

function __brows12FireLoad() {
  __brows12Dispatch("DOMContentLoaded", -1);
  __brows12Dispatch("load", -1);
}
