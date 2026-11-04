# tmv — TifMrcView Pro on servers without a display

`tmv` is TifMrcView Pro's processing as a command-line tool for Linux
servers (x86_64, glibc 2.34+: Ubuntu 22.04+, Debian 12+, RHEL/Rocky 9+) with an NVIDIA GPU (or Vulkan, or only CPUs). It is meant
to be driven by coding agents such as Claude Code or Codex over SSH: the
agent copies the data to the server, runs one command, and copies the
results back for you to open in the TifMrcView Pro desktop app.

Install for the current user (no root, no CUDA toolkit needed; the NVIDIA
driver is enough):

    curl -fsSL https://tifmrcview.com/tifmrcview/cli/install.sh | sh

## Commands

| Command | What it does | Main output |
| --- | --- | --- |
| `tmv run PATH` | Detects the data type and runs the command below | |
| `tmv tomo TS.mrc\|TS.mdoc [--for display\|sta\|survey]` | One-click cryo-ET: motion correction (from an mdoc), alignment, CTF, reconstruction; for display also denoising, missing-wedge filling and membranes | `*_rec*.mrc`, `*.labels.tif`, RELION 5 folder (sta) |
| `tmv membranes TOMO.mrc [--model membrain.ckpt]` | Membranes, vesicles (split and measured) and microtubules | `TOMO.mrc.labels.tif`, `TOMO.mrc.membranes.json`, `TOMO.mrc.vesicles.csv` |
| `tmv spa FOLDER [--kind N] [--symmetry D7]` | One-click single-particle analysis | `FOLDER/tifmrcview-spa/` with the sharpened map |
| `tmv ctf FOLDER` | CTF of every micrograph (movies motion corrected first) | `FOLDER/tifmrcview-spa/ctf.tsv` |
| `tmv denoise FILE` | Self-supervised (Noise2Void) denoising trained on the file | `FILE_denoised.mrc` |
| `tmv info` | Version, GPU, models | |
| `tmv setup --membrane FILE.ckpt` / `--sam` | Remember the MemBrain-seg model / download SAM 2.1 | |

Everything is chosen automatically, as in the app. `--pixel Å` and
`--kv kV` are needed only when neither the files nor the config files beside
them carry the pixel size and voltage.

Results are written beside the input. The result of each run is printed as
one JSON object on stdout and saved as `INPUT.tmv.json`. Exit status: 0 done,
1 error (`"error"`, `"hint"`), 2 a choice is needed (SPA: which kind of
particle, shown as PNG strips).

Project files (`*.tomo.json`, `tifmrcview-spa/spa.json`) keep working when
the folder is copied to another computer, so the app's cryo-ET and SPA
workspaces show every step of a run made on the server.

## With Claude Code or Codex

Install the skill on the computer where the agent runs (not the server):

    # Claude Code
    mkdir -p ~/.claude/skills && cp -R skills/tifmrcview-remote ~/.claude/skills/
    # Codex
    mkdir -p ~/.codex/skills && cp -R skills/tifmrcview-remote ~/.codex/skills/

or copy `AGENTS.md` into the project folder. Then ask in plain words, e.g.
"Reconstruct ~/data/ts01.mrc on gpu1 and bring the result back" or
"在 gpu1 上把 /data/tomo/ 里的断层做膜分割，统计囊泡大小".

## Membrane model

Membrane segmentation uses the published MemBrain-seg model, which you
download yourself from https://github.com/teamtomo/membrain-seg (its
weights are not ours to redistribute). Copy the `.ckpt` to the server once
and run `tmv setup --membrane membrain.ckpt`.

## Building

    cd src-tauri
    cargo build --release --no-default-features --features cli --bin tmv

`node scripts/pack-cli.mjs` makes the release tarball (binary, NVIDIA's
libdevice for CUDA, this README, the skill).
