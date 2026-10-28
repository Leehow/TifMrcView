"""Writes the small synthetic Velox .emd files used by
src-tauri/tests/thermo_plugin.rs.

    python make_fixtures.py ../../../src-tauri/tests/fixtures

Needs h5py and numpy. The pixel values follow `pattern()`, which the Rust
test repeats. thermo_velox.emd holds three images the way Velox lays them
out (rows x columns x frames, a JSON document per frame in `Metadata`):

- HAADF: u16, 5 frames, one frame per chunk, gzip + shuffle;
- BM-Ceta: i16, 3 frames interleaved in one chunk;
- DF4: f32, 70 frames, contiguous (more frames than a recipe chunk may
  have samples, so the plugin copies the frame out).

thermo_spectrum.emd only has a SpectrumStream.
"""

import json
import sys

import h5py
import numpy as np


def pattern(f, y, x, mod):
    return (x * 7 + y * 13 + f * 101) % mod


def stack(h, w, frames, dtype, mod):
    y, x, f = np.meshgrid(np.arange(h), np.arange(w), np.arange(frames), indexing="ij")
    return pattern(f, y, x, mod).astype(dtype)


def metadata(detector, pixel, frames):
    doc = {
        "Core": {"MetadataDefinitionVersion": "7.9"},
        "Instrument": {"ControlSoftwareVersion": "3.7.0", "Manufacturer": "FEI Company", "InstrumentModel": "Talos F200X", "InstrumentId": "1234"},
        "Acquisition": {"AcquisitionStartDatetime": {"DateTime": "1456073429"}, "SourceType": "XFEG"},
        "Optics": {"AccelerationVoltage": "200000", "NominalMagnification": "160000", "CameraLength": "0.098", "SpotIndex": "6", "Defocus": "-1.5e-07"},
        "Stage": {"Position": {"x": "1.25e-05", "y": "-3e-06", "z": "0"}, "AlphaTilt": "0.2617993877991494", "BetaTilt": "0", "HolderType": "Single Tilt"},
        "Scan": {"DwellTime": "2e-06", "FrameTime": "0.5"},
        "BinaryResult": {"Detector": detector, "PixelSize": {"width": str(pixel), "height": str(pixel)}, "PixelUnitX": "m", "PixelUnitY": "m"},
        "Note": "café \"quoted\"",
    }
    raw = json.dumps(doc).encode()
    # Velox uses 60000 bytes per frame; less keeps the fixture small.
    m = np.zeros((4096, frames), dtype=np.uint8)
    for k in range(frames):
        m[: len(raw), k] = np.frombuffer(raw, dtype=np.uint8)
    return m


def main(out):
    with h5py.File(f"{out}/thermo_velox.emd", "w", libver="earliest") as f:
        f.create_dataset("Version", data=np.array([b'{"format":"Velox","version":"3"}'], dtype=object), dtype=h5py.string_dtype())
        img = f.create_group("Data/Image")
        g = img.create_group("a1")
        g.create_dataset("Data", data=stack(12, 10, 5, "<u2", 60000), chunks=(12, 10, 1), compression="gzip", shuffle=True)
        g.create_dataset("Metadata", data=metadata("HAADF", 5.3e-10, 5), chunks=(4096, 1))
        g = img.create_group("b2")
        g.create_dataset("Data", data=stack(6, 9, 3, "<i2", 30000) - 5000, chunks=(6, 9, 3))
        g.create_dataset("Metadata", data=metadata("BM-Ceta", 2.5e-11, 3), chunks=(4096, 1))
        g = img.create_group("c3")
        g.create_dataset("Data", data=stack(4, 3, 70, "<f4", 1000) / 4)
        g.create_dataset("Metadata", data=metadata("DF4", 1e-9, 1))
        s = f.create_group("Data/SpectrumStream/d4")
        s.create_dataset("Data", data=np.zeros((10, 1), dtype="<u2"))
    with h5py.File(f"{out}/thermo_spectrum.emd", "w") as f:
        s = f.create_group("Data/SpectrumStream/d4")
        s.create_dataset("Data", data=np.zeros((10, 1), dtype="<u2"))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else ".")
