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
    "kiri.dialog.open": 41,
    "kiri.shortcut.register": 42,
    "kiri.autostart.set": 43,
    "kiri.autostart.get": 44,
    "kiri.store.get": 45,
    "kiri.store.set": 46,
    "kiri.deeplink.register": 47,
    "kiri.opener.open": 48,
    "kiri.window.state.save": 49,
    "kiri.window.state.load": 50,
    "kiri.tray.setMenu": 51,
    "kiri.tray.invoke": 52,
    "kiri.sidecar.spawn": 53,
    "kiri.sidecar.stop": 54,
    "kiri.sidecar.list": 55,
    "kiri.event.publish": 56,
    "kiri.event.subscribe": 57,
    "kiri.event.channels": 58,
    "kiri.config.get": 59,
    "kiri.config.keys": 60,
    "kiri.updater.check": 61,
    "kiri.http.post": 62,
    "kiri.http.put": 63,
    "kiri.http.patch": 64,
    "kiri.http.delete": 65,
    "kiri.cli.args": 66,
    "kiri.fs.watch": 67,
    "kiri.fs.unwatch": 68,
    "kiri.ws.connect": 69,
    "kiri.ws.send": 70,
    "kiri.ws.close": 71,
    "kiri.menu.set": 72,
    "kiri.menu.invoke": 73,
    "kiri.plugin.list": 74,
  };

  // Host injects window.kiri.send(WireRequest). Talk that path first so a
  // live WebView actually reaches Router::dispatch. Fall back to a named
  // invoke helper, then to a postMessage shim for harnesses.
  function invokeViaSend(name, payload) {
    return new Promise(function (resolve, reject) {
      var cmdId = IDS[name];
      if (cmdId == null) {
        reject(new Error("unknown command " + name));
        return;
      }
      if (!global.kiri || typeof global.kiri.send !== "function") {
        reject(new Error("window.kiri.send is not available"));
        return;
      }
      global.kiri.pending = global.kiri.pending || Object.create(null);
      global.__kiriIpcSeq = global.__kiriIpcSeq || 1;
      var id = global.__kiriIpcSeq++;
      var timer = setTimeout(function () {
        delete global.kiri.pending[id];
        reject(new Error("ipc timeout"));
      }, 15000);
      global.kiri.pending[id] = function (resp) {
        clearTimeout(timer);
        if (resp && resp.error) {
          reject(new Error((resp.error && resp.error.message) || "ipc error"));
        } else {
          resolve(resp && resp.payload !== undefined ? resp.payload : resp);
        }
      };
      var body = payload === undefined ? null : payload;
      var payloadJson = JSON.stringify(body);
      // Host validates against serde UTF-8 bytes, not JS UTF-16 length.
      // "Kiri — desk" is 23 code units and 25 bytes.
      var payloadLen = typeof TextEncoder === "function"
        ? new TextEncoder().encode(payloadJson).length
        : payloadJson.length;
      global.kiri.send({
        magic: "KRI1",
        version: 1,
        flags: 1,
        command_id: cmdId,
        request_id: id,
        payload_len: payloadLen,
        codec: 1,
        payload: body,
      });
    });
  }

  function bridge() {
    if (global.kiri && typeof global.kiri.send === "function") {
      return invokeViaSend;
    }
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

  function httpResult(r) {
    return { status: r.status, headers: r.headers, base64: r.base64, bytes: r.bytes };
  }

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
      state: {
        save: function (geometry) {
          return call("kiri.window.state.save", geometry || {}).then(function (r) {
            return { geometry: r };
          });
        },
        load: function () {
          return call("kiri.window.state.load", {}).then(function (r) {
            return { geometry: r };
          });
        },
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
        return call("kiri.http.get", { url: url, maxBytes: maxBytes || null }).then(httpResult);
      },
      post: function (url, body, maxBytes) {
        return call("kiri.http.post", { url: url, body: body || null, maxBytes: maxBytes || null }).then(httpResult);
      },
      put: function (url, body, maxBytes) {
        return call("kiri.http.put", { url: url, body: body || null, maxBytes: maxBytes || null }).then(httpResult);
      },
      patch: function (url, body, maxBytes) {
        return call("kiri.http.patch", { url: url, body: body || null, maxBytes: maxBytes || null }).then(httpResult);
      },
      del: function (url, maxBytes) {
        return call("kiri.http.delete", { url: url, maxBytes: maxBytes || null }).then(httpResult);
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

    // Restricted, host-allowlisted native dialogs (kiri.dialog.open). The host
    // owns the title text and only pre-approved dialog kinds may open, so the
    // frontend cannot fabricate a free-form native prompt. This exceeds Tauri's
    // dialog plugin, which lets the frontend open arbitrary native dialogs once
    // the capability is present.
    dialog: {
      open: function (kind, args, ext) {
        return call("kiri.dialog.open", { kind: kind, args: args || [], ext: ext }).then(function (r) {
          return { kind: r.kind, title: r.title, confirmed: r.confirmed, paths: r.paths };
        });
      },
    },

    // Restricted, host-allowlisted global shortcuts (kiri.shortcut.register). The
    // host owns the accelerator->action mapping; the frontend may only enable a
    // pre-approved accelerator, so it cannot register an arbitrary global hotkey.
    // This exceeds Tauri's global-shortcut plugin, which lets the frontend bind
    // arbitrary global combos once the capability is present.
    shortcut: {
      register: function (accelerator) {
        return call("kiri.shortcut.register", { accelerator: accelerator }).then(function (r) {
          return { accelerator: r.accelerator, action: r.action };
        });
      },
    },

    // Restricted, host-policy-gated autostart (kiri.autostart.set/get). The host owns
    // the policy (default-deny) and the target binary; the frontend can only toggle
    // launch-at-login. This exceeds Tauri's autostart plugin, which lets the frontend
    // enable login launch freely once the capability is present.
    autostart: {
      set: function (enabled) {
        return call("kiri.autostart.set", { enabled: enabled }).then(function (r) {
          return { enabled: r.enabled, managed: r.managed };
        });
      },
      get: function () {
        return call("kiri.autostart.get", {}).then(function (r) {
          return { enabled: r.enabled, managed: r.managed };
        });
      },
    },

    // Restricted, host-namespace-allowlisted store (kiri.store.get/set). The host owns
    // the namespace allowlist; the frontend may only address an approved namespace, so
    // one module cannot reach another's persisted state. This exceeds Tauri's store
    // plugin, which lets the frontend read/write the whole store once the capability is
    // present.
    store: {
      get: function (namespace, key) {
        return call("kiri.store.get", { namespace: namespace, key: key }).then(function (r) {
          return r.value;
        });
      },
      set: function (namespace, key, value) {
        return call("kiri.store.set", { namespace: namespace, key: key, value: value }).then(function (r) {
          return r.value;
        });
      },
    },

    // Restricted, host-scheme-allowlisted deep-link registration
    // (kiri.deeplink.register). The host owns the scheme allowlist; the frontend
    // may only register a host-approved scheme, never squat on an arbitrary scheme.
    deeplink: {
      register: function (scheme) {
        return call("kiri.deeplink.register", { scheme: scheme }).then(function (r) {
          return { scheme: r.scheme };
        });
      },
    },

    // Restricted, host-allowlisted opener (kiri.opener.open). The host owns the
    // scheme/extension allowlist; the frontend may only open a host-approved URL
    // scheme or file extension, never an arbitrary target.
    opener: {
      open: function (target) {
        return call("kiri.opener.open", { target: target }).then(function (r) {
          return { target: r.target };
        });
      },
    },

    // Restricted, host-allowlisted system tray (kiri.tray.*). The frontend may
    // only reference pre-approved item ids; the host owns every item's label and
    // action, so JavaScript can never draw or invoke an arbitrary native menu.
    // This exceeds Tauri's tray on the security axis (frontend cannot forge menu
    // items or redirect actions once the capability is present).
    tray: {
      setMenu: function (ids) {
        return call("kiri.tray.setMenu", { ids: ids || [] }).then(function (r) {
          return { items: r.items };
        });
      },
      invoke: function (id) {
        return call("kiri.tray.invoke", { id: id }).then(function (r) {
          return { id: r.id, action: r.action };
        });
      },
    },

    // Restricted, host-allowlisted sidecar processes (kiri.sidecar.*). The
    // frontend may only spawn a pre-approved sidecar by its host-owned name and
    // cannot pass arbitrary argv or address a binary path, so JavaScript can
    // never fork an unapproved companion executable. This exceeds Tauri's
    // sidecar API (which lets the frontend name an arbitrary binary once the
    // capability is present).
    sidecar: {
      spawn: function (name, args) {
        return call("kiri.sidecar.spawn", { name: name, args: args || [] }).then(function (r) {
          return { handle: r.handle, name: r.name, exitCode: r.exit_code, stdout: r.stdout, stderr: r.stderr };
        });
      },
      stop: function (handle) {
        return call("kiri.sidecar.stop", { handle: handle }).then(function (r) {
          return { stopped: r.stopped };
        });
      },
      list: function () {
        return call("kiri.sidecar.list", {}).then(function (r) {
          return { names: r.names };
        });
      },
    },

    // Restricted, channel-allowlisted events (kiri.event.publish/subscribe/
    // channels, audit item 16). The frontend may only publish/subscribe
    // pre-approved channel names whose namespace is host-owned, so it cannot
    // forge or snoop cross-module events. This exceeds Tauri's unrestricted
    // event module on the security axis.
    event: {
      publish: function (channel, payload) {
        return call("kiri.event.publish", { event: channel, payload: payload || null }).then(function (r) {
          return { emitted: r.emitted };
        });
      },
      subscribe: function (channel) {
        return call("kiri.event.subscribe", { event: channel }).then(function (r) {
          return { listenerId: r.listener_id, channel: r.channel };
        });
      },
      channels: function () {
        return call("kiri.event.channels", {}).then(function (r) {
          return { channels: r.channels };
        });
      },
    },

    // Restricted, key-allowlisted config (kiri.config.get/keys, audit item 17).
    // The frontend may only read pre-approved config key paths whose namespace is
    // host-owned, so it cannot read arbitrary host config. This exceeds Tauri's
    // unrestricted getConfig() on the security axis.
    config: {
      get: function (key) {
        return call("kiri.config.get", { key: key }).then(function (r) {
          return { key: r.key, value: r.value };
        });
      },
      keys: function () {
        return call("kiri.config.keys", {}).then(function (r) {
          return { keys: r.keys };
        });
      },
    },

    // Restricted, host-pinned-key signed-update check (kiri.updater.check, audit
    // item 18). The frontend submits a manifest and learns only whether a newer,
    // correctly-signed release exists for this OS; the signing key is host-pinned
    // and never visible to JavaScript, so this exceeds Tauri's frontend-keyed
    // updater on the security axis.
    updater: {
      check: function (manifest) {
        var payload = manifest ? { manifest: manifest } : {};
        return call("kiri.updater.check", payload).then(function (r) {
          return {
            available: r.available,
            version: r.version,
            platform: r.platform,
            notes: r.notes,
          };
        });
      },
    },

    fsWatch: {
      watch: function (path, kind) {
        return call("kiri.fs.watch", { path: path, kind: kind || "all" }).then(function (r) {
          return { watchId: r.watchId, path: r.path };
        });
      },
      unwatch: function (watchId) {
        return call("kiri.fs.unwatch", { watchId: watchId }).then(function (r) {
          return { unwatched: r.unwatched, watchId: r.watchId };
        });
      },
    },
    ws: {
      connect: function (url) {
        return call("kiri.ws.connect", { url: url }).then(function (r) {
          return { connId: r.connId, url: r.url };
        });
      },
      send: function (connId, message) {
        return call("kiri.ws.send", { connId: connId, message: message }).then(function (r) {
          return { sent: r.sent, connId: r.connId };
        });
      },
      close: function (connId) {
        return call("kiri.ws.close", { connId: connId }).then(function (r) {
          return { closed: r.closed, connId: r.connId };
        });
      },
    },
    menu: {
      onAction: function (callback) {
        window.kiri.onMenuAction = typeof callback === "function" ? callback : function () {};
      },
      set: function (ids) {
        return call("kiri.menu.set", { ids: ids }).then(function (r) {
          return { items: r.items };
        });
      },
      invoke: function (id) {
        return call("kiri.menu.invoke", { id: id }).then(function (r) {
          return { id: r.id, action: r.action };
        });
      },
    },
    plugin: {
      list: function () {
        return call("kiri.plugin.list", {}).then(function (r) {
          return { plugins: r.plugins };
        });
      },
    },
    cli: {
      args: function (full) {
        return call("kiri.cli.args", { full: !!full }).then(function (r) {
          return {
            raw: r.raw,
            positionals: r.positionals,
            flags: r.flags,
            options: r.options,
          };
        });
      },
    },
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
  global.kiri.cli = Kiri.cli;
  global.kiri.menu = Kiri.menu;
  global.kiri.ws = Kiri.ws;
  global.kiri.fsWatch = Kiri.fsWatch;

  global.kiri.http = Kiri.http;
  global.kiri.shell = Kiri.shell;
  global.kiri.notification = Kiri.notification;
  global.kiri.dialog = Kiri.dialog;
  global.kiri.shortcut = Kiri.shortcut;
  global.kiri.autostart = Kiri.autostart;
  global.kiri.store = Kiri.store;
  global.kiri.deeplink = Kiri.deeplink;
  global.kiri.opener = Kiri.opener;
  global.kiri.window = Kiri.window;
  global.kiri.tray = Kiri.tray;
  global.kiri.sidecar = Kiri.sidecar;
  global.kiri.fsWatch = Kiri.fsWatch;
  global.kiri.ws = Kiri.ws;
  global.kiri.menu = Kiri.menu;
  global.kiri.plugin = Kiri.plugin;
  global.kiri.cli = Kiri.cli;
  global.kiri.updater = Kiri.updater;
  global.kiri.invoke = invoke;
  global.__kiri = Kiri;
})(typeof window !== "undefined" ? window : this);
