"""Verify packaged Pro's native folder picker and real Cryo-ET scanner on X11."""
import json
import pathlib
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request


def call(method, endpoint, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    request = urllib.request.Request(
        "http://127.0.0.1:4444" + endpoint,
        data=data,
        headers={"Content-Type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(request, timeout=35) as response:
        return json.load(response)["value"]


def until(action, seconds=20):
    for _ in range(seconds * 4):
        try:
            value = action()
            if value:
                return value
        except OSError:
            pass
        time.sleep(0.25)
    raise TimeoutError("Native QA condition did not become true")


def fixture(parent):
    path = pathlib.Path(parent) / "Local Cryo-ET"
    (path / "Records").mkdir(parents=True)
    header = bytearray(1024)
    struct.pack_into("<10i", header, 0, 16, 16, 3, 2, 0, 0, 0, 16, 16, 3)
    struct.pack_into("<6f", header, 40, 16, 16, 3, 90, 90, 90)
    struct.pack_into("<3i", header, 64, 1, 2, 3)
    struct.pack_into("<3f", header, 76, 0, 15, 7.5)
    header[208:216] = b"MAP DD\0\0"
    pixels = [float((i + j + z) % 16) for z in range(3) for j in range(16) for i in range(16)]
    (path / "TS_native_smoke.mrc").write_bytes(header + struct.pack("<768f", *pixels))
    (path / "TS_native_smoke.tlt").write_text("-30\n0\n30\n")
    return path


def capture(name):
    subprocess.run(["import", "-window", "root", f"linux-qa-{name}.png"], check=True)


def main(executable):
    with tempfile.TemporaryDirectory(prefix="tmv-local-qa-") as root:
        directory = str(fixture(root))
        with subprocess.Popen(["openbox"]) as wm, subprocess.Popen(["tauri-driver"]) as driver:
            try:
                until(lambda: call("GET", "/status").get("ready"))
                session = call("POST", "/session", {"capabilities": {"alwaysMatch": {
                    "browserName": "wry", "tauri:options": {"application": executable},
                }}})["sessionId"]
                endpoint = f"/session/{session}"
                def js(script, *args):
                    return call("POST", endpoint + "/execute/sync", {"script": script, "args": args})

                try:
                    until(lambda: js("return !!window.TifMrcViewPro?.cryoet?.reconFolder"))
                    js("document.querySelector('#btn-open').click()")
                    assert js("return [...document.querySelectorAll('#dropmenu button')].some(b => /打开本地文件夹|Open local folder/.test(b.textContent))")
                    js("""
                        S.folder.dir = arguments[0];
                        [...document.querySelectorAll('#dropmenu button')]
                          .find(b => /打开本地文件夹|Open local folder/.test(b.textContent)).click();
                        return true;
                    """, directory)
                    until(lambda: subprocess.check_output(["xdotool", "getactivewindow", "getwindowname"], text=True).strip().lower() in
                          {"open folder", "select folder", "open", "选择文件夹", "打开文件夹", "打开"})
                    capture("picker")
                    subprocess.run(["xdotool", "key", "--clearmodifiers", "Return"], check=True)
                    until(lambda: js("return document.querySelector('#folder-name').title === arguments[0] && document.querySelector('#strip').textContent.includes('Records')", directory))
                    js("document.querySelector('#btn-cryoet').click()")
                    until(lambda: js("return [...document.querySelectorAll('.cs-tile')].some(e => e.textContent.includes('TS_native_smoke'))"))
                    capture("series")
                    print("PASS: Linux packaged Pro 0.7.331 folder picker, local browser and real Cryo-ET series discovery")
                except Exception:
                    capture("failure")
                    raise
                finally:
                    call("DELETE", endpoint)
            finally:
                driver.terminate()
                wm.terminate()


if __name__ == "__main__":
    main(sys.argv[1])
