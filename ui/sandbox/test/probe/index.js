"use strict";

// Manual native-WebView acceptance fixture, never installed automatically.
TifMrcView.register({
  id: "sandbox-ipc-probe",
  async activate(app) {
    const details = [];
    const nativeFetch = window.fetch.bind(window);
    const internals = window.__TAURI_INTERNALS__;
    let invokeKey;
    // Tauri's injected invoke closure knows the key, even though plugins do not.
    // Capture its outgoing fetch headers so raw probes use a valid key if one
    // leaked into this frame. Origin is browser-controlled and cannot be forged.
    window.fetch = (url, options) => {
      invokeKey = new Headers(options?.headers).get("Tauri-Invoke-Key") || invokeKey;
      return nativeFetch(url, options);
    };
    const attempt = async (name, fn) => {
      let timer;
      try {
        await Promise.race([
          Promise.resolve().then(fn),
          new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("no native reply (timeout)")), 1500); }),
        ]);
        details.push(`${name}: native success response`);
        return name + "!";
      } catch (e) {
        details.push(`${name}: ${String(e)}`);
        return name + "0";
      } finally { clearTimeout(timer); }
    };
    const labels = [];
    labels.push(await attempt("inv", () => internals.invoke("plugin_market_list")));
    window.fetch = nativeFetch;

    // Native postMessage returns void on both success and rejection. Returning
    // from postMessage is NOT proof of command execution. These two probes
    // therefore report 0 unless an explicit success reply is observed; inspect
    // the stock Tauri ACL / dispatch breakpoint as described in ui-plugins.md.
    const raw = (transport) => {
      if (typeof transport?.postMessage !== "function") throw new Error("native transport unavailable");
      transport.postMessage(JSON.stringify({
        cmd: "plugin_market_list", callback: 4294967294, error: 4294967293,
        payload: {}, options: { customProtocolIpcBlocked: true },
        __TAURI_INVOKE_KEY__: invokeKey || "probe-no-injected-key",
      }));
      throw new Error("posted without acknowledgement; inspect stock Tauri ACL/dispatch");
    };
    labels.push(await attempt("ipc", () => raw(window.ipc)));
    labels.push(await attempt("wv", () => raw(window.chrome?.webview)));
    const headers = {
      "Content-Type": "application/json", "Tauri-Callback": "4294967294",
      "Tauri-Error": "4294967293", "Tauri-Invoke-Key": invokeKey || "probe-no-injected-key",
    };
    const fetchIpc = async (url) => {
      const response = await nativeFetch(url, { method: "POST", headers, body: "{}" });
      if (!response.ok || response.headers.get("Tauri-Response") !== "ok") {
        throw new Error(`IPC rejected (${response.status}): ${await response.text()}`);
      }
      return response.text();
    };
    labels.push(await attempt("f1", () => fetchIpc("ipc://localhost/plugin_market_list")));
    labels.push(await attempt("f2", () => fetchIpc("http://ipc.localhost/plugin_market_list")));
    // Check a core command too; an app-only invoke_handler guard would miss it.
    labels.push(await attempt("core", () => internals.invoke("plugin:app|version")));
    console.info("Sandbox IPC probe", { frameUrl: location.href, frameOrigin: location.origin, label: labels.join(" "), keyCaptured: !!invokeKey, details });
    await app.ui.addToolbarButton({
      label: labels.join(" "), title: details.join("\n"),
      onClick: () => app.ui.toast(details.join("\n"), 12000),
    });
  },
});
