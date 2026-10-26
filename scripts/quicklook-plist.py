"""Generate exact UTI declarations from the app's associations."""
import json
import plistlib
import sys
from pathlib import Path

kind, dest = sys.argv[1:]
config = json.loads(Path("src-tauri/tauri.conf.json").read_text())
types = []
for assoc in config["bundle"]["fileAssociations"]:
    types.extend(assoc.get("contentTypes", []))
    if "exportedType" in assoc:
        types.append(assoc["exportedType"]["identifier"])
attrs = {"QLSupportedContentTypes": sorted(set(types))}
if kind == "Thumbnail":
    attrs["QLThumbnailMinimumDimension"] = 0
else:
    attrs["QLIsDataBasedPreview"] = True
plist = {
    "CFBundleIdentifier": f"com.leehow.tifmrcview.quicklook.{kind.lower()}",
    "CFBundleExecutable": f"Tmv{kind}",
    "CFBundleName": f"Tmv{kind}",
    "CFBundlePackageType": "XPC!",
    "CFBundleVersion": config["version"],
    "CFBundleShortVersionString": config["version"],
    "LSMinimumSystemVersion": "10.15" if kind == "Thumbnail" else "12.0",
    "NSExtension": {
        "NSExtensionPointIdentifier": f"com.apple.quicklook.{kind.lower()}",
        "NSExtensionPrincipalClass": f"{kind}Provider",
        "NSExtensionAttributes": attrs,
    },
}
Path(dest).write_bytes(plistlib.dumps(plist))
