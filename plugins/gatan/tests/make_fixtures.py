"""Writes the small synthetic DM3 / DM4 files used by src-tauri/tests/gatan_plugin.rs.

    python make_fixtures.py ../../../src-tauri/tests/fixtures

Needs numpy. The files are written from scratch by the minimal tag writer
below (DigitalMicrograph itself is not needed); the pixel values follow
`pattern()`, which the Rust test repeats.

- gatan_u16.dm3: little-endian, an RGBA preview first (skipped), then a
  19x12 u16 image calibrated in nm, with microscope, camera and data bar
  tags, a struct, a large array that is not read and unnamed entries.
- gatan_stack.dm4: a 7x5x3 f32 stack, X/Y in µm and Z in nm.
- gatan_complex.dm3: a 4x3 complex64 diffraction pattern in 1/nm.
- gatan_rgb.dm4: a single 5x4 RGBA image.
- gatan_be.dm3: big-endian data, two i16 images (the series picker).
- gatan_packed.dm4: a preview and an FFT in packed complex (DataType 27),
  which is not supported: opening it must fail, not show the preview.
"""

import struct
import sys

import numpy as np


def pattern(z, y, x, mod):
    return (x * 7 + y * 13 + z * 101) % mod


class Writer:
    def __init__(self, version, le=True):
        self.v4 = version == 4
        self.le = le
        self.version = version

    def n(self, v):
        return struct.pack(">Q" if self.v4 else ">I", v)

    def tag(self, label, body, kind):
        lab = label.encode("latin-1")
        head = bytes([kind]) + struct.pack(">H", len(lab)) + lab
        if self.v4:
            head += struct.pack(">Q", len(body))
        return head + body

    def dir(self, entries):
        """entries: list of (label, value); value is bytes (a data tag) or list (a dir)."""
        out = b"\x00\x00" + self.n(len(entries))
        for label, v in entries:
            if isinstance(v, list):
                out += self.tag(label, self.dir(v), 20)
            else:
                out += self.tag(label, v, 21)
        return out

    def data(self, info, payload):
        return b"%%%%" + self.n(len(info)) + b"".join(self.n(i) for i in info) + payload

    def e(self, fmt):
        return ("<" if self.le else ">") + fmt

    # simple values
    def i32(self, v):
        return self.data([3], struct.pack(self.e("i"), v))

    def u32(self, v):
        return self.data([5], struct.pack(self.e("I"), v))

    def f32(self, v):
        return self.data([6], struct.pack(self.e("f"), v))

    def f64(self, v):
        return self.data([7], struct.pack(self.e("d"), v))

    def boolean(self, v):
        return self.data([8], bytes([1 if v else 0]))

    def text(self, s):
        u = s.encode("utf-16-le" if self.le else "utf-16-be")
        return self.data([20, 4, len(u) // 2], u)

    def struct_(self, types_values):
        info = [15, 0, len(types_values)]
        payload = b""
        for t, fmt, v in types_values:
            info += [0, t]
            payload += struct.pack(self.e(fmt), v)
        return self.data(info, payload)

    def array(self, t, arr):
        a = np.ascontiguousarray(arr)
        a = a.astype(a.dtype.newbyteorder("<" if self.le else ">"))
        return self.data([20, t, a.size], a.tobytes())

    def file(self, root):
        body = self.dir(root)
        order = struct.pack(">I", 1 if self.le else 0)
        if self.v4:
            head = struct.pack(">IQ", 4, len(body)) + order
        else:
            head = struct.pack(">II", 3, len(body)) + order
        # DigitalMicrograph ends the file with 8 zero bytes.
        return head + body + b"\x00" * 8


DM_ELEM = {np.dtype("u1"): 10, np.dtype("i2"): 2, np.dtype("u2"): 4, np.dtype("i4"): 3,
           np.dtype("u4"): 5, np.dtype("f4"): 6, np.dtype("f8"): 7}


def cal(w, scale, units, origin=0.0):
    return [("Origin", w.f32(origin)), ("Scale", w.f32(scale)), ("Units", w.text(units))]


def image(w, name, dm_type, pixels, dims, cals, tags=None, elem=None):
    flat = pixels.reshape(-1)
    elem = elem if elem is not None else DM_ELEM[flat.dtype]
    data = w.array(elem, flat)
    entries = [
        ("ImageData", [
            ("Calibrations", [
                ("Brightness", cal(w, 1.0, "")),
                ("Dimension", [("", c) for c in cals]),
                ("DisplayCalibratedUnits", w.boolean(True)),
            ]),
            ("Data", data),
            ("DataType", w.i32(dm_type)),
            ("Dimensions", [("", w.u32(d)) for d in dims]),
            ("PixelDepth", w.u32(flat.dtype.itemsize)),
        ]),
        ("ImageTags", tags or []),
        ("Name", w.text(name)),
        ("UniqueID", w.struct_([(3, "i", 1), (3, "i", 2), (3, "i", 3), (3, "i", 4)])),
    ]
    return entries


def gray(shape, dtype, mod):
    z, y, x = np.meshgrid(*[np.arange(n) for n in shape], indexing="ij")
    return pattern(z, y, x, mod).astype(dtype)


def preview(w):
    rgba = np.zeros((3, 4), dtype="<u4") + 0xFF102030
    return image(w, "", 23, rgba, [4, 3], [cal(w, 1.0, ""), cal(w, 1.0, "")], elem=5)


def doc(w, images):
    return [
        ("ApplicationBounds", w.array(3, np.array([0, 0, 800, 600], dtype="i4"))),
        ("DocumentObjectList", [("", [("AnnotationType", w.i32(20))])]),
        ("ImageList", [("", im) for im in images]),
        ("Image Behavior", [("ViewDisplayID", w.u32(8))]),
    ]


def u16_dm3(out):
    w = Writer(3)
    px = gray((1, 12, 19), "u2", 65536)[0]
    tags = [
        ("Acquisition", [
            ("Device", [("Name", w.text("OneView")), ("Source", w.text("Gatan OneView")),
                        ("Active Size (pixels)", w.array(5, np.array([4096, 4096], dtype="u4")))]),
            ("Parameters", [("High Level", [("Binning", w.array(5, np.array([2, 2], dtype="u4"))),
                                            ("Exposure (s)", w.f32(0.5)),
                                            ("Processing", w.text("Gain normalized"))])]),
        ]),
        ("DataBar", [("Acquisition Date", w.text("2024/5/17")), ("Acquisition Time", w.text("14:03:22")),
                     ("Exposure Time (s)", w.f64(0.5))]),
        ("GMS Version", [("Created", w.text("3.50.3584.0"))]),
        ("Microscope Info", [
            ("Actual Magnification", w.f64(61234.5)),
            ("Cs(mm)", w.f32(2.7)),
            ("Illumination Mode", w.text("TEM")),
            ("Indicated Magnification", w.f64(50000.0)),
            ("Microscope", w.text("JEOL JEM-2100F")),
            ("Operation Mode", w.text("IMAGING")),
            ("Stage Position", [("Stage Alpha", w.f32(-12.5)), ("Stage X", w.f32(101.25))]),
            ("Voltage", w.f32(200000.0)),
        ]),
        ("Big table", w.array(6, np.arange(5000, dtype="f4"))),
        ("Unnamed", [("", w.i32(7)), ("", w.i32(8))]),
    ]
    im = image(w, "u16 image", 10, px, [19, 12], [cal(w, 0.25, "nm", 1.5), cal(w, 0.25, "nm")], tags)
    open(f"{out}/gatan_u16.dm3", "wb").write(w.file(doc(w, [preview(w), im])))


def stack_dm4(out):
    w = Writer(4)
    px = gray((3, 5, 7), "f4", 1000) / 4
    im = image(w, "stack", 2, px, [7, 5, 3],
               [cal(w, 0.125, "µm"), cal(w, 0.125, "µm"), cal(w, 250.0, "nm")],
               [("GMS Version", [("Created", w.text("3.60.4441.0"))])])
    open(f"{out}/gatan_stack.dm4", "wb").write(w.file(doc(w, [preview(w), im])))


def complex_dm3(out):
    w = Writer(3)
    re = gray((1, 3, 4), "f4", 100)[0]
    c = (re + 1j * (re * 2 + 1)).astype("c8")
    pairs = np.stack([c.real, c.imag], axis=-1).astype("f4")
    im = image(w, "FFT", 3, pairs, [4, 3], [cal(w, 0.5, "1/nm"), cal(w, 0.5, "1/nm")])
    open(f"{out}/gatan_complex.dm3", "wb").write(w.file(doc(w, [im])))


def rgb_dm4(out):
    w = Writer(4)
    y, x = np.meshgrid(np.arange(4), np.arange(5), indexing="ij")
    b, g, r = x * 10, y * 20, x + y
    px = (b | (g << 8) | (r << 16) | (255 << 24)).astype("u4")
    im = image(w, "colour", 23, px, [5, 4], [cal(w, 2.0, "mm"), cal(w, 2.0, "mm")], elem=5)
    open(f"{out}/gatan_rgb.dm4", "wb").write(w.file(doc(w, [im])))


def be_dm3(out):
    w = Writer(3, le=False)
    a = (gray((1, 4, 6), "i2", 300)[0] - 150).astype("i2")
    b = (gray((1, 2, 3), "i2", 50)[0] * -1).astype("i2")
    ims = [
        image(w, "first", 1, a, [6, 4], [cal(w, 3.0, "pm"), cal(w, 3.0, "pm")],
              [("Microscope Info", [("Voltage", w.f64(300000.0))])]),
        image(w, "second", 1, b, [3, 2], [cal(w, 1.0, ""), cal(w, 1.0, "")]),
    ]
    open(f"{out}/gatan_be.dm3", "wb").write(w.file(doc(w, ims)))


def packed_dm4(out):
    w = Writer(4)
    px = np.zeros((4, 4), dtype="f4")
    im = image(w, "FFT", 27, px, [4, 4], [cal(w, 1.0, "1/nm"), cal(w, 1.0, "1/nm")])
    open(f"{out}/gatan_packed.dm4", "wb").write(w.file(doc(w, [preview(w), im])))


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    for f in (u16_dm3, stack_dm4, complex_dm3, rgb_dm4, be_dm3, packed_dm4):
        f(out)
