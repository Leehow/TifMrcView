"""Verify the installed Pro folder workflow via the Windows native WebDriver."""
import base64
import pathlib
import runpy
import subprocess
import sys
import tempfile


fixture_support = runpy.run_path(str(pathlib.Path(__file__).with_name("linux-folder-smoke.py")))
call = fixture_support["call"]
until = fixture_support["until"]


def main(executable, edge_driver):
    with tempfile.TemporaryDirectory(prefix="tmv-windows-qa-") as root:
        directory = str(fixture_support["fixture"](root))
        with subprocess.Popen(["tauri-driver", "--native-driver", edge_driver]) as driver:
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
                    result = call("POST", endpoint + "/execute/async", {
                        "script": "T.app.getVersion().then(arguments[arguments.length - 1])", "args": [],
                    })
                    assert result == "0.7.331", result
                    assert js("""
                        S.folder.dir = arguments[0];
                        document.querySelector('#btn-open').click();
                        const item = [...document.querySelectorAll('#dropmenu button')]
                          .find(b => /打开本地文件夹|Open local folder/.test(b.textContent));
                        item?.click();
                        return !!item;
                    """, directory)
                    app_pid = subprocess.check_output(["powershell", "-NoProfile", "-Command",
                        "(Get-Process 'tifmrcview-pro' | Select-Object -First 1).Id",
                    ], text=True).strip()
                    subprocess.run(["powershell", "-NoProfile", "-File",
                        str(pathlib.Path(__file__).with_name("select-test-folder.ps1")), app_pid,
                    ], check=True, timeout=25)
                    until(lambda: js("return document.querySelector('#folder-name').title === arguments[0] && document.querySelector('#strip').textContent.includes('Records')", directory))
                    js("document.querySelector('#btn-cryoet').click()")
                    until(lambda: js("return [...document.querySelectorAll('.cs-tile')].some(e => e.textContent.includes('TS_native_smoke'))"))
                    image = call("GET", endpoint + "/screenshot")
                    pathlib.Path("windows-qa-series.png").write_bytes(base64.b64decode(image))
                    print("PASS: installed Windows Pro 0.7.331 folder chooser, local browser and real Cryo-ET series discovery")
                finally:
                    call("DELETE", endpoint)
            finally:
                driver.terminate()


if __name__ == "__main__":
    main(*sys.argv[1:])
