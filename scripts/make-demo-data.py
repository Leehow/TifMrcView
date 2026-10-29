#!/usr/bin/env python3
"""Builds the sample data of the website's live demo (site/demo/data/).

Each image is box-downsampled, mapped to 8 bits with a square-root curve
between two percentiles of the whole stack (so dim fluorescence keeps its
detail), and saved as one WebP atlas per channel: the Z slices as tiles,
row by row. site/demo/tauri-mock.js maps the bytes back to the original
16-bit range, so contrast and histograms behave like the real file.

    python3 scripts/make-demo-data.py <folder with *.ome.tif> [<folder with ZEN *.xml>]

Needs numpy, tifffile, imagecodecs and Pillow.
"""
import json
import math
import re
import sys
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image

OUT = Path(__file__).resolve().parent.parent / "site" / "demo" / "data"

# (OME-TIFF stem, downsampling factor, name shown in the demo, size of the
# original CZI in MiB as the app lists it). The OME-TIFFs were exported from
# those CZI files pixel for pixel.
SAMPLES = [
    ("100x-Airyscan Processing-06", 8, "100x-Airyscan Processing-06.czi", 63.8),
    ("100x-Airyscan Processing-17", 2, "100x-Airyscan Processing-17.czi", 2.4),
    ("100x-Airyscan Processing-Zstack", 4, "100x-Airyscan Processing-Zstack.czi", 197.3),
]


def downsample(a, f):
    h, w = a.shape[-2] // f * f, a.shape[-1] // f * f
    a = a[..., :h, :w].astype(np.float32)
    return a.reshape(*a.shape[:-2], h // f, f, w // f, f).mean(axis=(-3, -1))


def tag(xml, name):
    m = re.search(rf"<{name}>([^<]*)</{name}>", xml)
    return m.group(1).strip() if m else None


def zen(xml):
    """Channel names/colours and a few acquisition settings from ZEN XML."""
    chans = []
    disp = xml[xml.find("<DisplaySetting"):]
    for m in re.finditer(r'<Channel Id="[^"]*" Name="([^"]*)">(.*?)</Channel>', disp, re.S):
        c = re.search(r"<Color>#?([0-9A-Fa-f]{8}|[0-9A-Fa-f]{6})</Color>", m.group(2))
        rgb = None
        if c:
            h = c.group(1)[-6:]
            rgb = [int(h[i:i + 2], 16) for i in (0, 2, 4)]
        chans.append((m.group(1), rgb))
    items = []
    ver = re.search(r"<Application>.*?<Version>([^<]*)</Version>", xml, re.S)
    for label, val in [
        ("显微镜", tag(xml, "CameraName")),
        ("物镜", tag(xml, "ObjectiveName")),
        ("总放大倍数", tag(xml, "TotalMagnification")),
        ("激发波长 (nm)", tag(xml, "ExcitationWavelength")),
        ("发射波长 (nm)", tag(xml, "EmissionWavelength")),
    ]:
        if val:
            items.append([label, val])
    return chans, items, ver.group(1) if ver else None


def main():
    src = Path(sys.argv[1])
    meta_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    OUT.mkdir(parents=True, exist_ok=True)
    files = []
    for stem, f, name, mib in SAMPLES:
        tif = tifffile.TiffFile(src / f"{stem}.ome.tif")
        s = tif.series[0]
        a = s.asarray()
        axes = s.axes
        if "C" not in axes:
            a = a[None]
            axes = "C" + axes
        if "Z" not in axes:
            a = a[:, None]
        elif axes.index("Z") < axes.index("C"):
            a = np.swapaxes(a, 0, 1)
        # a is now C, Z, Y, X
        om = tif.ome_metadata or ""
        px = float(re.search(r'PhysicalSizeX="([^"]+)"', om).group(1)) * f
        pz = re.search(r'PhysicalSizeZ="([^"]+)"', om)
        pz = float(pz.group(1)) if pz else 0.0  # 2D: no Z spacing, as the CZI plugin reports
        names = re.findall(r'<Channel [^>]*Name="([^"]*)"', om)
        chans_xml, items, ver = ([], [], None)
        if meta_dir and (meta_dir / f"{stem}.xml").exists():
            chans_xml, items, ver = zen((meta_dir / f"{stem}.xml").read_text("utf-8", "ignore"))
        c_n, z_n = a.shape[:2]
        small = downsample(a, f)
        h, w = small.shape[-2:]
        cols = math.ceil(math.sqrt(z_n))
        rows = math.ceil(z_n / cols)
        channels = []
        key = re.sub(r"[^a-z0-9]+", "-", stem.lower()).strip("-")
        for c in range(c_n):
            v = small[c]
            lo, hi = np.percentile(v, [0.002, 99.998])
            q = np.sqrt(np.clip((v - lo) / max(hi - lo, 1e-6), 0, 1)) * 255
            atlas = np.zeros((rows * h, cols * w), np.uint8)
            for z in range(z_n):
                r, k = divmod(z, cols)
                atlas[r * h:(r + 1) * h, k * w:(k + 1) * w] = np.round(q[z])
            fname = f"{key}-c{c + 1}.webp"
            Image.fromarray(atlas, "L").save(OUT / fname, "WEBP", quality=88, method=6)
            nm = chans_xml[c][0] if c < len(chans_xml) else (names[c] if c < len(names) else f"C{c + 1}")
            col = chans_xml[c][1] if c < len(chans_xml) and chans_xml[c][1] else ([0, 255, 0], [255, 0, 255])[c % 2]
            channels.append({"name": nm, "color": col, "atlas": fname, "lo": float(lo), "hi": float(hi)})
        group = [["像素尺寸 X", f"{px:.4f} µm"], ["像素尺寸 Y", f"{px:.4f} µm"]]
        if z_n > 1:
            group.append(["Z 步长", f"{pz:.4f} µm"])
        group.append(["网页示例", f"由原图 {a.shape[-1]} × {a.shape[-2]} 缩小 {f} 倍"])
        files.append({
            "name": name,
            "format": f"Zeiss ZEN CZI (ZEN {ver})" if ver else "Zeiss ZEN CZI",
            "width": int(w), "height": int(h), "sizeZ": int(z_n),
            "cols": cols,
            "pixelSize": [px, px, pz], "unit": "µm",
            "bytes": round(mib * 1024 * 1024),
            "channels": channels,
            "metadata": [{"name": "ZEN 图像", "items": group}] + ([{"name": "ZEN 显微镜", "items": items}] if items else []),
        })
        print(name, small.shape, [round(ch["lo"]) for ch in channels], [round(ch["hi"]) for ch in channels])
    (OUT / "samples.json").write_text(json.dumps({"files": files}, ensure_ascii=False, indent=1) + "\n", "utf-8")


if __name__ == "__main__":
    main()
