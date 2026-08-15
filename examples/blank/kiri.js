// Kiri frontend API (R-3): capability-gated surface mirroring the most-used
// Tauri modules (os, app, event). This file is served over kiri:// and is
// transport-agnostic: it posts commands through whatever native bridge the
// host injects (window.kiri.invoke) and falls back to a postMessage shim so
// the same code works under the direct Win32/WebView2 host, the wry/tao host,
// and a Tauri-interop test harness.
//
// Every command carries its numeric id from the generated catalog
// (gen/commands.ts) so the JS and native routers stay in lockstep.
(function (global) {
  "use strict";

  var IDS = {
    "kiri.platform.os": 5,
    "kiri.platform.arch": 6,
    "kiri.app.version": 7,
    "kiri.event.emit": 8,
    "kiri.event.listen": 9,
    "kiri.fs.read": 10,
    "kiri.fs.write": 11,
    "kiri.fs.exists": 12,
    "kiri.fs.remove": 13,
    "kiri.window.title.get": 14,
    "kiri.window.title.set": 15,
    "kiri.window.show": 16,
    "kiri.window.hide": 17,
    "kiri.window.minimize": 18,
    "kiri.window.maximize": 19,
    "kiri.window.restore": 20,
    "kiri.window.close": 21,
    "kiri.window.focus": 22,
  };

  // Resolve the host bridge. The direct Kiri host injects window.kiri with an
  // invoke(name, payload) -> Promise. Otherwise we shim through postMessage.
  function bridge() {
    if (global.kiri && typeof global.kiri.invoke === "function") {
      return function (name, payload) {
        return global.kiri.invoke(name, payload);
      };
    }
    return function (name, payload) {
      return new Promise(function (resolve, reject) {
        var reqId = "k" + Math.random().toString(36).slice(2);
        function onMsg(e) {
          var d = e.data;
          if (!d || d.type !== "kiri-response" || d.request_id !== reqId) return;
          global.removeEventListener("message", onMsg);
          if (d.error) reject(new Error(d.error));
          else resolve(d.payload);
        }
        global.addEventListener("message", onMsg);
        var msg = JSON.stringify({
          type: "kiri-command",
          request_id: reqId,
          command: name,
          payload: payload,
        });
        if (global.chrome && global.chrome.webview && global.chrome.webview.postMessage) {
          global.chrome.webview.postMessage(msg);
        } else if (global.ipc && global.ipc.postMessage) {
          global.ipc.postMessage(msg);
        } else {
          reject(new Error("no kiri bridge available"));
        }
      });
    };
  }

  var invoke = bridge();

  function call(name, payload) {
    return invoke(name, payload || null).then(function (resp) {
      // The native command envelope returns { payload: ... } for success.
      return resp && typeof resp === "object" && "payload" in resp
        ? resp.payload
        : resp;
    });
  }

  var listeners = Object.create(null);

  var Kiri = {
    platform: {
      os: function () {
        return call("kiri.platform.os").then(function (r) { return r.os; });
      },
      arch: function () {
        return call("kiri.platform.arch").then(function (r) { return r.arch; });
      },
    },
    app: {
      version: function () {
        return call("kiri.app.version").then(function (r) { return r.version; });
      },
    },
    event: {
      emit: function (event, payload) {
        return call("kiri.event.emit", { event: event, payload: payload || null }).then(function (
          r
        ) {
          return r.emitted;
        });
      },
      listen: function (event, handler) {
        return call("kiri.event.listen", { event: event }).then(function (r) {
          var id = r.listener_id;
          (listeners[event] = listeners[event] || []).push({ id: id, handler: handler });
          return id;
        });
      },
    },
    fs: {
      read: function (path) {
        return call("kiri.fs.read", { path: path }).then(function (r) { return r.base64; });
      },
      write: function (path, base64, createNew) {
        return call("kiri.fs.write", { path: path, base64: base64, create_new: !!createNew }).then(function (r) {
          return r.written;
        });
      },
      exists: function (path) {
        return call("kiri.fs.exists", { path: path }).then(function (r) { return r.exists; });
      },
      remove: function (path) {
        return call("kiri.fs.remove", { path: path }).then(function (r) { return r.removed; });
      },
    },

    window: {
      title: function () {
        return call("kiri.window.title.get").then(function (r) { return r.title; });
      },
      setTitle: function (title) {
        return call("kiri.window.title.set", { title: title }).then(function (r) { return r.title; });
      },
      show: function () {
        return call("kiri.window.show");
      },
      hide: function () {
        return call("kiri.window.hide");
      },
      minimize: function () {
        return call("kiri.window.minimize");
      },
      maximize: function () {
        return call("kiri.window.maximize");
      },
      restore: function () {
        return call("kiri.window.restore");
      },
      close: function () {
        return call("kiri.window.close");
      },
      focus: function () {
        return call("kiri.window.focus");
      },
    },
    // Expose raw command ids for tooling/debugging parity with the catalog.
    commandIds: IDS,
  };

  // Deliver native -> JS event publications routed back through the bridge.
  global.addEventListener("message", function (e) {
    var d = e.data;
    if (!d || d.type !== "kiri-event" || !listeners[d.event]) return;
    listeners[d.event].forEach(function (l) {
      try { l.handler(d.payload); } catch (err) { /* listener errors are non-fatal */ }
    });
  });

  global.kiri = global.kiri || {};
  global.kiri.platform = Kiri.platform;
  global.kiri.app = Kiri.app;
  global.kiri.event = Kiri.event;
  global.kiri.fs = Kiri.fs;
  global.kiri.window = Kiri.window;
  global.__kiri = Kiri;
})(typeof window !== "undefined" ? window : this);
