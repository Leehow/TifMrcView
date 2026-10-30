#!/usr/bin/env python3
"""Writes src-tauri/src/xray_data.rs, the X-ray lines and absorption edges
used by spectrum quantification (spectro.rs), from xraydb (pip install
xraydb), which holds the Elam, Ravel & Sieber (2002) tables (public domain).

    python3 scripts/gen-xray-data.py > src-tauri/src/xray_data.rs
"""
import xraydb

EDGES = ["K", "L1", "L2", "L3", "M1", "M2", "M3", "M4", "M5", "N4", "N5", "O2", "O3"]

print("""//! X-ray emission lines and absorption edges, Z = 3–92, generated from the
//! Elam, Ravel & Sieber (2002) tables as published in xraydb (public domain).
//! Energies in eV. Each line: (name, energy, intensity among the lines of its
//! initial level, family). Lines under 1 % or 100 eV are left out.
//! Written by scripts/gen-xray-data.py; do not edit by hand.

pub struct Element { pub z: u32, pub sym: &'static str, pub mass: f64, pub edges: &'static [Edge], pub lines: &'static [Line] }
pub struct Edge { pub name: &'static str, pub energy: f64, pub fyield: f64 }
pub struct Line { pub name: &'static str, pub energy: f64, pub intensity: f64, pub family: &'static str }

pub static ELEMENTS: &[Element] = &[""")
for z in range(3, 93):
    sym = xraydb.atomic_symbol(z)
    edges = xraydb.xray_edges(sym)
    es = ", ".join(
        f'Edge {{ name: "{n}", energy: {edges[n].energy:.1f}, fyield: {edges[n].fyield:.5f} }}' for n in EDGES if n in edges
    )
    lines = [(n, l) for n, l in xraydb.xray_lines(sym).items() if n[0] in "KLM" and l.intensity >= 0.01 and l.energy >= 100]
    lines.sort(key=lambda t: t[1].energy)
    ls = ", ".join(
        f'Line {{ name: "{n}", energy: {l.energy:.1f}, intensity: {l.intensity:.4f}, family: "{n[0]}" }}' for n, l in lines
    )
    print(f'    Element {{ z: {z}, sym: "{sym}", mass: {xraydb.atomic_mass(sym):.4f}, edges: &[{es}], lines: &[{ls}] }},')
print("];")
