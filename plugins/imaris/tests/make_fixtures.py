"""Writes the small synthetic .ims files used by src-tauri/tests/imaris_plugin.rs.

    python make_fixtures.py ../../../src-tauri/tests/fixtures

Needs h5py and numpy. The pixel values follow `pattern()`, which the Rust
test repeats. Two files cover both generations of HDF5 structures:

- imaris_v0.ims: libver "earliest" (superblock 0, version 1 object headers,
  symbol-table groups, v1 B-tree chunk index), 2 channels x 2 time points,
  u16, gzip chunks 4x8x16 with padding, one dataset stored uncompressed.
- imaris_v110.ims: libver "v110" (superblock 3, version 2 object
  headers, dense link and attribute storage in fractal heaps with v2
  B-trees, a huge attribute, layout version 4 with fixed-array and
  single-chunk indexes), u8. (HDF5 2.0 "latest" would write layout
  version 5, which Imaris does not use.)
"""

import sys

import h5py
import numpy as np


def pattern(c, t, z, y, x, mod):
    return (x * 7 + y * 13 + z * 101 + c * 1000 + t * 3000) % mod


def chars(s):
    """Imaris stores strings as arrays of one-character strings."""
    return np.array([bytes([b]) for b in s.encode("utf-8")], dtype="S1")


def volume(c, t, size, padded, dtype, mod):
    z, y, x = np.meshgrid(*[np.arange(n) for n in padded], indexing="ij")
    v = pattern(c, t, z, y, x, mod).astype(dtype)
    # Padding beyond the real size is zero, as Imaris writes it.
    v[size[0]:, :, :] = 0
    v[:, size[1]:, :] = 0
    v[:, :, size[2]:] = 0
    return v


def common(f, size, nc, nt, extra_info_groups=0, huge=False):
    for k, v in [("DataSetDirectoryName", "DataSet"), ("DataSetInfoDirectoryName", "DataSetInfo"),
                 ("ImarisDataSet", "ImarisDataSet"), ("ImarisVersion", "5.5.0"), ("ThumbnailDirectoryName", "Thumbnail")]:
        f.attrs[k] = chars(v)
    f.attrs["NumberOfDataSets"] = np.array([1], dtype="<u4")
    info = f.create_group("DataSetInfo")
    img = info.create_group("Image")
    z, y, x = size
    for k, v in [("X", x), ("Y", y), ("Z", z), ("Unit", "um"), ("ExtMin0", "-1.5"), ("ExtMax0", f"{-1.5 + 0.25 * x}"),
                 ("ExtMin1", "0"), ("ExtMax1", f"{0.25 * y}"), ("ExtMin2", "2"), ("ExtMax2", f"{2 + 1.5 * z}"),
                 ("Name", "synthetic"), ("Description", "made by make_fixtures.py"), ("RecordingDate", "2026-10-26 09:30:00.000"),
                 ("MicroscopeMode", "SpinningDiskConfocal"), ("LensPower", "60"), ("NumericalAperture", "1.4")]:
        img.attrs[k] = chars(str(v))
    names = ["EGFP", "mCherry"]
    colors = ["0.000 1.000 0.000", "1.000 0.000 0.000"]
    for c in range(nc):
        g = info.create_group(f"Channel {c}")
        for k, v in [("Name", names[c]), ("Color", colors[c]), ("ColorMode", "BaseColor"), ("ColorRange", "0 4000"),
                     ("LSMExcitationWavelength", ["488 nm nm", "561 nm nm"][c]), ("LSMEmissionWavelength", "525 nm"),
                     ("LSMPinhole", "40"), ("Description", "")]:
            g.attrs[k] = chars(v)
    ti = info.create_group("TimeInfo")
    ti.attrs["DatasetTimePoints"] = chars(str(nt))
    ti.attrs["FileTimePoints"] = chars(str(nt))
    for t in range(nt):
        ti.attrs[f"TimePoint{t + 1}"] = chars(f"2026-10-26 09:30:{t * 2.5:06.3f}")
    ds = info.create_group("ImarisDataSet")
    ds.attrs["Creator"] = chars("Imaris")
    ds.attrs["Version"] = chars("10.2")
    for i in range(extra_info_groups):
        info.create_group(f"Vendor {i}").attrs["Index"] = chars(str(i))
    if huge:
        info.create_group("CustomData").attrs["Settings"] = chars("{" + "x" * 6000 + "}")


def write(path, libver, dtype, mod, nc, nt, size, padded, chunks, compress, extra_info_groups=0, huge=False):
    with h5py.File(path, "w", libver=libver) as f:
        common(f, size, nc, nt, extra_info_groups, huge)
        for t in range(nt):
            for c in range(nc):
                g = f.create_group(f"DataSet/ResolutionLevel 0/TimePoint {t}/Channel {c}")
                for k, v in [("ImageSizeX", size[2]), ("ImageSizeY", size[1]), ("ImageSizeZ", size[0]),
                             ("HistogramMin", "0.000"), ("HistogramMax", f"{mod - 1}.000")]:
                    g.attrs[k] = chars(str(v))
                kw = {"compression": "gzip", "compression_opts": 2} if compress(c, t) else {}
                g.create_dataset("Data", data=volume(c, t, size, padded, dtype, mod), chunks=chunks(c, t), **kw)
                # A lower resolution level, which the plugin skips.
                h = f.create_group(f"DataSet/ResolutionLevel 1/TimePoint {t}/Channel {c}")
                h.create_dataset("Data", data=np.zeros((1, 2, 2), dtype=dtype))
        f.create_dataset("Thumbnail/Data", data=np.zeros((4, 8), dtype=np.uint8))


def main(out):
    write(f"{out}/imaris_v0.ims", "earliest", np.uint16, 65536, 2, 2, (5, 12, 19), (8, 16, 32),
          lambda c, t: (4, 8, 16), lambda c, t: not (c == 1 and t == 1))
    write(f"{out}/imaris_v110.ims", ("v110", "v110"), np.uint8, 256, 2, 1, (3, 12, 14), (4, 16, 16),
          lambda c, t: (2, 8, 8) if c == 0 else (4, 16, 16), lambda c, t: c == 0, extra_info_groups=9, huge=True)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else ".")
