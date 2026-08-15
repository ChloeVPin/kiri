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
    "kiri.clipboard.read": 23,
    "kiri.clipboard.write": 24,
    "kiri.path.dirname": 25,
    "kiri.path.basename": 26,
    "kiri.path.extname": 27,
    "kiri.path.stem": 28,
    "kiri.path.join": 29,
    "kiri.path.isAbsolute": 30,
    "kiri.os.homedir": 31,
    "kiri.os.tempdir": 32,
    "kiri.os.appConfigDir": 33,
    "kiri.os.appDataDir": 34,
    "kiri.os.appCacheDir": 35,
    "kiri.os.documentDir": 36,
    "kiri.os.appDir": 37,
    "kiri.http.get": 38,
    "kiri.shell.run": 39,
    "kiri.notification.show": 40,
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
    clipboard: {
      read: function () {
        return call("kiri.clipboard.read").then(function (r) { return r.text; });
      },
      write: function (text) {
        return call("kiri.clipboard.write", { text: text }).then(function (r) {
          return r.written;
        });
      },
    },

    path: {
      dirname: function (path) { return call("kiri.path.dirname", { path: path }).then(function (r) { return r.dirname; }); },
      basename: function (path) { return call("kiri.path.basename", { path: path }).then(function (r) { return r.basename; }); },
      extname: function (path) { return call("kiri.path.extname", { path: path }).then(function (r) { return r.extname; }); },
      stem: function (path) { return call("kiri.path.stem", { path: path }).then(function (r) { return r.stem; }); },
      join: function (base, segments) { return call("kiri.path.join", { path: base, segments: segments || [] }).then(function (r) { return r.path; }); },
      isAbsolute: function (path) { return call("kiri.path.isAbsolute", { path: path }).then(function (r) { return r.isAbsolute; }); },
    },
    os: {
      homedir: function () { return call("kiri.os.homedir").then(function (r) { return r.dir; }); },
      tempdir: function () { return call("kiri.os.tempdir").then(function (r) { return r.dir; }); },
      appConfigDir: function () { return call("kiri.os.appConfigDir").then(function (r) { return r.dir; }); },
      appDataDir: function () { return call("kiri.os.appDataDir").then(function (r) { return r.dir; }); },
      appCacheDir: function () { return call("kiri.os.appCacheDir").then(function (r) { return r.dir; }); },
      documentDir: function () { return call("kiri.os.documentDir").then(function (r) { return r.dir; }); },
      appDir: function () { return call("kiri.os.appDir").then(function (r) { return r.dir; }); },
    },

    http: {
      get: function (url, maxBytes) {
        return call("kiri.http.get", { url: url, maxBytes: maxBytes || null }).then(function (r) {
          return { status: r.status, headers: r.headers, base64: r.base64, bytes: r.bytes };
        });
      },
    },

    // Restricted, host-allowlisted command execution (kiri.shell.run). The
    // host refuses any program/arg-prefix that is not on its explicit allowlist,
    // so a granted capability still cannot spawn an unapproved binary. This
    // exceeds Tauri's shell plugin, which allows arbitrary execution once the
    // capability is present.
    shell: {
      run: function (program, args) {
        return call("kiri.shell.run", { program: program, args: args || [] }).then(function (r) {
          return {
            program: r.program,
            exitCode: r.exitCode,
            stdout: r.stdout,
            stderr: r.stderr,
            bytes: r.bytes,
          };
        });
      },
    },

    // Restricted, host-template-allowlisted notifications (kiri.notification.show).
    // The host owns the title/body text; the frontend may only trigger a pre-approved
    // template id with bounded args, so it cannot render free-form notification
    // content. This exceeds Tauri's notification plugin, which lets the frontend send
    // arbitrary title/body once the capability is present.
    notification: {
      show: function (template, args) {
        return call("kiri.notification.show", { template: template, args: args || [] }).then(function (r) {
          return { templateId: r.templateId, title: r.title, body: r.body };
        });
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
  global.kiri.clipboard = Kiri.clipboard;
  global.kiri.path = Kiri.path;
  global.kiri.os = Kiri.os;
  global.kiri.http = Kiri.http;
  global.kiri.shell = Kiri.shell;
  global.kiri.notification = Kiri.notification;
  global.__kiri = Kiri;
})(typeof window !== "undefined" ? window : this);
