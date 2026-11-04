---
name: tifmrcview-remote
description: Process cryo-EM / cryo-ET data on a remote Linux GPU server with TifMrcView Pro's command-line tool `tmv` over SSH (tomogram reconstruction, membrane / vesicle / microtubule segmentation, single-particle analysis, CTF, denoising), then copy the results back to open in TifMrcView Pro. Use when the user asks to reconstruct, segment, denoise or run SPA/CTF on data on (or to be sent to) a server.
---

# TifMrcView Pro on a remote server (`tmv`)

`tmv` runs TifMrcView Pro's one-click processing without a window. Every
technical choice (alignment, CTF, reconstruction method, denoising,
missing-wedge filling, symmetry, GPU backend…) is made automatically, as in
the desktop app. **Never invent parameters or tuning options**: the only
inputs are the data, what the result is for, and (only when the files do not
say) the pixel size and voltage.

## What users say

Users describe the goal; you do the SSH, copying and commands. Typical requests:
- 「把 ~/data/ts01.mrc 传到 gpu1 上重构断层，做完把结果拿回来」
- 「gpu1 上 /data/tomo/ 里的断层都做一下膜分割，统计囊泡大小」
- 「用 lab-gpu 把 ~/spa/movies/ 这批电影做单颗粒重构，D7 对称」
- "Reconstruct the tilt series in ~/data/ts01 on gpu1 for subtomogram averaging."
- "Check the defocus of the micrographs in /data/session3 on gpu1."

When a user asks how to use this, show them requests like these: they only
name the data, the server and the goal.

## 1. Find out where things are

Ask only for what you cannot see:
- the SSH host (e.g. `gpu1`, `user@10.0.0.5`; `~/.ssh/config` aliases work),
- where the data is (on this computer, or already on the server),
- for tomograms: what the result is for — looking at it / figures (default),
  subtomogram averaging (`--for sta`), or a quick survey (`--for survey`).

Check the server: `ssh HOST 'uname -m; ldd --version | head -1; nvidia-smi -L; df -h ~'`.
Needs Linux x86_64 with glibc 2.34+ (Ubuntu 22.04+, Debian 12+, RHEL/Rocky 9+);
a GPU is strongly recommended (NVIDIA driver is enough, no CUDA
toolkit needed). Check free disk space before copying (a tilt series of
movies can be 20–100 GB); if the home disk is small, put the job folder on a
larger disk you can write to.

## 2. Install or update `tmv` on the server (no root needed)

```sh
ssh HOST 'curl -fsSL https://tifmrcview.com/tifmrcview/cli/install.sh | sh'
ssh HOST '~/.local/bin/tmv info'
```

`tmv info` prints JSON with the GPU found (`"gpu": "CUDA"`, `"Vulkan"`, or
`"CPU"`). With `"CPU"` warn the user that it will be very slow and suggest
installing the NVIDIA driver. Re-running the installer updates `tmv`. If the
server has no internet, download the `.tar.gz` named in
https://tifmrcview.com/tifmrcview/downloads-pro.json (`files.cli`) here,
copy it over and unpack it; the binary is `tmv` at its top level.

## 3. Copy the data to the server (skip if it is already there)

Use `rsync -a --partial --info=progress2` into a fresh folder per dataset,
e.g. `~/tmv-jobs/<name>/`. Copy everything that belongs to the acquisition:
- tilt series: the stack (`.mrc`/`.st`) **and** its `.mdoc` / `.tlt` /
  `.rawtlt` with the same name; or the movies + `.mdoc` + gain reference;
- SPA: the folder of micrographs or movies, plus any gain reference and
  config/`.xml`/`.mdoc` files beside them (they carry pixel size, voltage, dose).

## 4. Run — one command per purpose

Always in the background with a log, because jobs take minutes to hours:

```sh
ssh HOST 'cd ~/tmv-jobs/NAME && nohup ~/.local/bin/tmv COMMAND INPUT [OPTIONS] > tmv.log 2>&1 &'
```

| The user wants | Command |
| --- | --- |
| Not sure / "process this" | `tmv run INPUT` (detects the data type) |
| Tomogram from a tilt series or from movies + mdoc | `tmv tomo ts.mrc` or `tmv tomo ts.mdoc` |
| … for subtomogram averaging | `tmv tomo ts.mrc --for sta` (adds a RELION 5 folder) |
| … just a quick look | `tmv tomo ts.mrc --for survey` |
| Membranes, vesicles, microtubules of a tomogram | `tmv membranes tomo.mrc` |
| Single-particle reconstruction | `tmv spa FOLDER` (see step 5) |
| Defocus / CTF of micrographs | `tmv ctf FOLDER` |
| Denoise an image or stack | `tmv denoise FILE` |

Options that exist (use only when needed): `--pixel Å` and `--kv kV` when
the files do not carry them (the error JSON says so), `--symmetry D7` for SPA
when the user knows the point group, `--kind N` for SPA (step 5), `--fresh`
to redo instead of reusing finished steps.

Membrane segmentation needs the MemBrain-seg model, which users download
themselves from the official page (https://github.com/teamtomo/membrain-seg,
a `.ckpt` file); do not fetch it from anywhere else. Copy it to the server
once and run `tmv setup --membrane ~/models/membrain.ckpt`. After that
`tmv tomo` (display) also segments membranes automatically. SPA gets better
picks with SAM 2.1: `tmv setup --sam` downloads it once (156 MB).

Follow progress: `ssh HOST 'tail -n 5 ~/tmv-jobs/NAME/tmv.log'`. Check every
few minutes, more rarely for long jobs; `pgrep -f "tmv "` tells whether it is
still running. Do not start several GPU jobs at once on the same GPU.

## 5. Read the result

When the job ends, the log ends with one JSON object, also saved beside the
input as `INPUT.tmv.json` (`tmv.json` inside a folder):
- `"ok": true` — `"open"` is the file to look at; other fields hold the
  numbers to report (resolution, defocus, vesicle count…).
- `"ok": false` — `"error"` (often Chinese, translate it) and maybe `"hint"`
  say what is missing; fix it (e.g. add `--pixel`) and run again. Finished
  steps are reused.
- `"needsChoice": true` (exit status 2, SPA only) — several kinds of
  particle were found. Copy the `png` strips of `"kinds"` here and **show
  them to the user** (or look at them yourself if the user asked you to
  decide): pick the protein, never ice, carbon edges or clumps; different
  views of the same particle can be picked together. Then run
  `tmv spa FOLDER --kind N` (or `--kind 1,3`).

## 6. Copy the results back and open them

Copy back the files named in the JSON, or the whole job folder without the
raw movies, e.g.:

```sh
rsync -a --info=progress2 --exclude '*.eer' --exclude '*.tiff' HOST:tmv-jobs/NAME/ ./NAME/
```

Then tell the user what to open in **TifMrcView Pro**:
- tomogram: the `"open"` file; its label map (`*.labels.tif`, materials
  膜 membrane / 囊泡 vesicle / 微管 microtubule) opens with it, and the 3D view
  shows them; vesicle sizes are in `*.vesicles.csv`;
- SPA: the sharpened map (`"map"`) in the 3D view; opening the folder in the
  SPA workspace shows every step (project files move with the folder);
- the cryo-ET workspace likewise reopens the dataset (`*.tomo.json`).

Report briefly: what ran, how long, the key numbers from the JSON, and the
paths. Remove large intermediate files on the server only if the user agrees.
