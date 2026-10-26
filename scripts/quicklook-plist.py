"""Generate macOS app and helper declarations from shared format lists."""
import json
import plistlib
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def app_types(config):
    documents = []
    imported = []
    for assoc in config["bundle"]["fileAssociations"]:
        types = list(assoc.get("contentTypes", []))
        if "exportedType" in assoc:
            types.append(assoc["exportedType"]["identifier"])
        documents.append({
            "CFBundleTypeExtensions": assoc["ext"],
            "CFBundleTypeName": assoc["name"],
            "CFBundleTypeRole": assoc.get("role", "Viewer"),
            "LSHandlerRank": assoc.get("rank", "Default"),
            "LSItemContentTypes": types,
        })
    # These formats belong to other vendors. Do not claim ownership.
    for ext, name in json.loads((ROOT / "packaging/macos/plugin-formats.json").read_text()).items():
        uti = f"com.leehow.tifmrcview.{ext}"
        documents.append({
            "CFBundleTypeExtensions": [ext], "CFBundleTypeName": name,
            "CFBundleTypeRole": "Viewer", "LSHandlerRank": "Alternate",
            "LSItemContentTypes": [uti],
        })
        imported.append({
            "UTTypeIdentifier": uti, "UTTypeDescription": name,
            "UTTypeConformsTo": ["public.data", "public.image"],
            "UTTypeTagSpecification": {"public.filename-extension": [ext]},
        })
    return {"CFBundleDocumentTypes": documents, "UTImportedTypeDeclarations": imported}


def generate(kind):
    config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
    app = app_types(config)
    if kind == "App":
        # Tauri merges top-level keys, so include the existing document types.
        # Its existing exported UTI declarations remain intact.
        return app
    types = {uti for doc in app["CFBundleDocumentTypes"] for uti in doc["LSItemContentTypes"]}
    attrs = {"QLSupportedContentTypes": sorted(types)}
    if kind == "Thumbnail":
        attrs["QLThumbnailMinimumDimension"] = 0
    else:
        attrs["QLIsDataBasedPreview"] = True
    return {
        "CFBundleIdentifier": f"com.leehow.tifmrcview.quicklook.{kind.lower()}",
        "CFBundleExecutable": f"Tmv{kind}", "CFBundleName": f"Tmv{kind}",
        "CFBundlePackageType": "XPC!", "CFBundleVersion": config["version"],
        "CFBundleShortVersionString": config["version"],
        "LSMinimumSystemVersion": "10.15" if kind == "Thumbnail" else "12.0",
        "NSExtension": {
            "NSExtensionPointIdentifier": f"com.apple.quicklook.{kind.lower()}",
            "NSExtensionPrincipalClass": f"{kind}Provider", "NSExtensionAttributes": attrs,
        },
    }


if __name__ == "__main__":
    kind, dest = sys.argv[1:]
    if kind not in ("App", "Thumbnail", "Preview"):
        raise ValueError(kind)
    Path(dest).write_bytes(plistlib.dumps(generate(kind)))
