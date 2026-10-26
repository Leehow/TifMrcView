import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const source = fs.readFileSync(new URL("../site/site.js", import.meta.url), "utf8");
const direct = "https://dl.tifmrcview.com:9443/tifmrcview/";
const selector = "#download a[download], #cta[download]";
const flush = () => new Promise((resolve) => setImmediate(resolve));

function page(fetcher, home = true, alreadyRendered = false) {
  const listeners = {}, timers = new Map(), calls = [];
  let links = alreadyRendered ? [{ href: "1.2.3/app.dmg", getAttribute() { return this.href; } }] : [];
  const document = {
    querySelector: (q) => q === "#download a[download]" ? links[0] : null,
    querySelectorAll: (q) => q === selector ? links : [],
    getElementById: (id) => home && id === "cta" ? {} : null,
    addEventListener: (name, callback) => { listeners[name] = callback; },
  };
  const fetch = (...args) => { calls.push(args); return fetcher(...args); };
  vm.runInNewContext(source, {
    document, window: { fetch, AbortController }, fetch, AbortController,
    setTimeout: (fn, ms) => { timers.set(fn, ms); return fn; },
    clearTimeout: (id) => timers.delete(id),
  });
  return {
    calls, timers,
    links: () => links,
    render(files = ["1.2.3/app.dmg", "1.2.3/setup.exe", "1.2.3/app.deb", "1.2.3/app.dmg"]) {
      links = files.map((file) => ({ href: file, getAttribute() { return this.href; } }));
      listeners["downloads-rendered"]?.();
      return links;
    },
  };
}

test("render relative links first, upgrade on success, preserve direct links on rerender", async () => {
  let resolve;
  const p = page(() => new Promise((r) => { resolve = r; }));
  assert.equal(p.calls.length, 0, "wait for download rendering before probing");
  const links = p.render();
  assert.equal(links[0].href, "1.2.3/app.dmg");
  assert.equal(p.calls[0][0], direct + "downloads.json");
  assert.equal([...p.timers.values()][0], 3000);
  resolve({ ok: true, json: async () => ({ files: {} }) });
  await flush();
  assert.ok(links.every((a) => a.href.startsWith(direct)));
  assert.equal(p.timers.size, 0);
  const rerendered = p.render(["1.2.3/app.rpm", "1.2.3/app.AppImage"]);
  assert.deepEqual(rerendered.map((a) => a.href), [direct + "1.2.3/app.rpm", direct + "1.2.3/app.AppImage"]);
  assert.equal(p.calls.length, 1, "language/size rerenders do not repeat the probe");
});

for (const failure of ["network", "HTTP", "JSON", "timeout"]) {
  test(`${failure} probe failure silently keeps normal links`, async () => {
    const p = page((_, { signal }) => {
      if (failure === "network") return Promise.reject(new Error("offline"));
      if (failure === "HTTP") return Promise.resolve({ ok: false });
      if (failure === "JSON") return Promise.resolve({ ok: true, json: async () => { throw new Error("invalid JSON"); } });
      return new Promise((_, reject) => signal.addEventListener("abort", () => reject(new Error("aborted"))));
    });
    const links = p.render();
    if (failure === "timeout") {
      [...p.timers.keys()][0]();
      assert.equal(p.calls[0][1].signal.aborted, true);
    }
    await flush();
    assert.equal(links[0].href, "1.2.3/app.dmg");
    assert.equal(p.timers.size, 0);
    p.render();
    assert.equal(p.calls.length, 1);
  });
}

test("plugin/other pages do not probe the direct downloads host", () => {
  const p = page(() => { throw new Error("unexpected fetch"); }, false);
  p.render();
  assert.equal(p.calls.length, 0);
});

// downloads.json can finish before a slow-loading site.js has registered its listener.
test("probe also runs when download rendering finished before site.js loaded", async () => {
  const p = page(async () => ({ ok: true, json: async () => ({ files: {} }) }), true, true);
  assert.equal(p.calls.length, 1);
  await flush();
  assert.equal(p.links()[0].href, direct + "1.2.3/app.dmg");
});
