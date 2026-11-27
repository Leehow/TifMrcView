import json, sys, csv, struct
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

d = sys.argv[1]            # .../membrane
image = sys.argv[2]        # labelled image (mrc)
out = sys.argv[3]          # folder for pngs
r = json.load(open(f"{d}/membrane.json"))
plt.rcParams["font.sans-serif"] = ["Arial Unicode MS", "Noto Sans CJK SC", "DejaVu Sans"]

def read_mrc(p):
    with open(p, "rb") as f:
        h = f.read(1024)
        nx, ny, nz, mode = struct.unpack("4i", h[:16])
        ext = struct.unpack("i", h[92:96])[0]
        f.seek(1024 + ext)
        dt = {0: np.int8, 1: np.int16, 2: np.float32, 6: np.uint16}[mode]
        return np.fromfile(f, dtype=dt, count=nx * ny * nz).reshape(nz, ny, nx).astype(np.float32)

# 1. profiles
fig, ax = plt.subplots(1, 2 if r.get("fine") else 1, figsize=(12 if r.get("fine") else 6, 4), squeeze=False)
a = ax[0][0]
for i, m in enumerate(r["membranes"]):
    if m["profile"]:
        a.plot(r["offsetsA"], m["profile"], lw=0.8, alpha=0.6, label=f"膜{i+1} ({m['kept']}/{m['patches']})")
a.plot(r["offsetsA"], r["profile"], "k", lw=2.5, label="共同")
a.axvline(0, color="gray", lw=0.5)
a.set_xlabel("离膜中心 Å（负=内侧，正=外侧）")
a.set_title(f"断层图上（{r['voxelA']:.1f} Å 体素）膜厚 {r['thicknessA'] or 0:.0f} Å")
a.legend(fontsize=7)
if r.get("fine"):
    f = r["fine"]
    b = ax[0][1]
    b.plot(f["offsetsA"], f["profile"], "k", lw=2)
    b.axvline(0, color="gray", lw=0.5)
    lf = f"，双层间距 {f['leafletsA']:.0f} Å" if f.get("leafletsA") else "，双层未分开"
    b.set_title(f"倾斜图像平均（{f['voxelA']:.1f} Å）膜厚 {f['thicknessA'] or 0:.0f} Å{lf}")
    b.set_xlabel("离膜中心 Å")
fig.tight_layout()
fig.savefig(f"{out}/1-profile.png", dpi=150)

# 2. patches on the labelled image, per section that has them
vol = read_mrc(image)
rows = list(csv.DictReader(open(f"{d}/patches.csv")))
colors = {"kept": "lime", "unlike": "red", "off": "orange", "hidden": "deepskyblue", "edge": "gray"}
names = {"kept": "保留", "unlike": "不像膜", "off": "偏离膜", "hidden": "缺失楔看不见", "edge": "靠边"}
zs = sorted({round(float(x["image_z"])) for x in rows})
zmid = max(zs, key=lambda z: sum(1 for x in rows if round(float(x["image_z"])) == z))
sl = vol[min(max(zmid, 0), vol.shape[0] - 1)]
lo, hi = np.percentile(sl, [1, 99])
fig, a = plt.subplots(figsize=(9, 9))
a.imshow(sl, cmap="gray", vmin=lo, vmax=hi)
for k, c in colors.items():
    pts = [x for x in rows if x["result"] == k and abs(float(x["image_z"]) - zmid) <= 2]
    if pts:
        a.scatter([float(p["image_x"]) for p in pts], [float(p["image_y"]) for p in pts], s=14, c=c, label=f"{names[k]} {len(pts)}")
for m in set(x["membrane"] for x in rows):
    pts = [x for x in rows if x["membrane"] == m and abs(float(x["image_z"]) - zmid) <= 2]
    if pts:
        p = pts[len(pts) // 2]
        a.text(float(p["image_x"]) + 6, float(p["image_y"]), f"膜{m}", color="yellow", fontsize=10)
a.set_title(f"第 {zmid} 层：每个点是一小片膜")
a.legend(loc="lower right", fontsize=9)
a.axis("off")
fig.tight_layout()
fig.savefig(f"{out}/2-patches.png", dpi=130)

# 3. average patch(es): side view (xz) and top view
maps = [("断层图上平均", r.get("map"))] + ([("倾斜图像平均", r["fine"]["map"])] if r.get("fine") else [])
maps = [m for m in maps if m[1]]
fig, ax = plt.subplots(len(maps), 2, figsize=(8, 4 * len(maps)), squeeze=False)
for i, (t, p) in enumerate(maps):
    v = read_mrc(p)
    n = v.shape[0]
    side = v[:, n // 2 - 2:n // 2 + 3, :].mean(axis=1)
    top = v[n // 2 - 1:n // 2 + 2].mean(axis=0)
    ax[i][0].imshow(side, cmap="gray", origin="lower")
    ax[i][0].set_title(f"{t}：侧面（上=外侧）")
    ax[i][1].imshow(top, cmap="gray")
    ax[i][1].set_title(f"{t}：膜面")
    for a in ax[i]:
        a.axis("off")
fig.tight_layout()
fig.savefig(f"{out}/3-average.png", dpi=130)
print("ok", zmid)
