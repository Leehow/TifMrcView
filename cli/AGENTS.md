# TifMrcView Pro on a remote server (for Codex and other agents)

Copy this file into your project as `AGENTS.md` (or add it to an existing
one), or install the skill in `skills/tifmrcview-remote/` (Claude Code:
`~/.claude/skills/`, Codex: `~/.codex/skills/`). The instructions are the
same; this file is the single-file version.

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
| … for subtomogram averaging | `tmv tomo ts.mrc --for sta` (adds a RELION 5 folder and a clear `"look"` tomogram) |
| … just a quick look | `tmv tomo ts.mrc --for survey` |
| Average a particle from tomograms (subtomogram averaging) | `tmv sta ts.mrc` or `tmv sta ts_sart3_b2_flip.mrc [more.mrc …]` (step 5) |
| Membranes, vesicles, microtubules of a tomogram | `tmv membranes tomo.mrc` |
| Single-particle reconstruction | `tmv spa FOLDER` (see step 5) |
| Defocus / CTF of micrographs | `tmv ctf FOLDER` |
| Denoise an image or stack | `tmv denoise FILE` |

Options that exist (use only when needed): `--pixel Å` and `--kv kV` when
the files do not carry them (the error JSON says so), `--symmetry D7` for SPA
when the user knows the point group, `--kind N` for SPA and `--pick N` for
STA (step 5), `--out DIR` for STA (default: a folder `<first input's name>_sta`
beside the first input), `--fresh`
to redo instead of reusing finished steps. For SPA movies: a gain reference
beside them (name with gain, CountRef or `norm-`) is found and applied by
itself; otherwise `--gain FILE`. `--dose e/Å²` (total per movie) turns on
dose weighting when no config file gives the dose; with these after a first
run, add `--fresh`. `"movies"."warnings"` in the result says when to.

Membrane segmentation needs the MemBrain-seg model, which users download
themselves from the official page (https://github.com/teamtomo/membrain-seg,
a `.ckpt` file); do not fetch it from anywhere else. Copy it to the server
once and run `tmv setup --membrane ~/models/membrain.ckpt`. After that
`tmv tomo` (display) also segments membranes automatically. SPA gets better
picks with SAM 2.1: `tmv setup --sam` downloads it once (156 MB).

On a server with several GPUs, look at `nvidia-smi` first and run on an idle
card: prefix the command with `CUDA_VISIBLE_DEVICES=N` (tmv then uses CUDA on
that card). Never take a card another job is using fully.

Follow progress: `ssh HOST 'tail -n 5 ~/tmv-jobs/NAME/tmv.log'`. Check every
few minutes, more rarely for long jobs; `pgrep -f "tmv "` tells whether it is
still running. Do not start several GPU jobs at once on the same GPU.

## 5. Read the result

When the job ends, the log ends with one JSON object, also saved beside the
input as `INPUT.tmv.json` (`tmv.json` inside a folder; `tmv tomo --for sta`
and `--for survey` save `INPUT.sta.tmv.json` and `INPUT.survey.tmv.json`, so
one stack's results do not overwrite each other; `tmv sta` saves
`OUT/sta.tmv.json`, so the `.tmv.json` of a tilt series from `tmv tomo` stays):
- `"ok": true` — `"open"` is the file to look at; other fields hold the
  numbers to report (resolution, defocus, vesicle count…); `"note"`, when
  present, is something the user should know (translate it if needed).
- `"ok": false` — `"error"` (often Chinese, translate it) and maybe `"hint"`
  say what is missing; fix it (e.g. add `--pixel`) and run again. Finished
  steps are reused.
- `"needsChoice": true` (exit status 2, SPA only) — several kinds of
  particle were found. Copy the `png` strips of `"kinds"` here (quote a
  remote wildcard so the local shell leaves it alone:
  `rsync -a 'HOST:~/tmv-jobs/NAME/tifmrcview-spa/kind*.png' .`) and **show
  them to the user** (or look at them yourself if the user asked you to
  decide): pick the protein, never ice, carbon edges or clumps; different
  views of the same particle can be picked together. Then run
  `tmv spa FOLDER --kind N` (or `--kind 1,3`).
- `"needsChoice": true` (exit status 2, STA) — `tmv sta` made class-average
  "cards" of the blobs in the tomograms (`"cards"`: `id`, `diameterA`,
  `members`, `compact`, `likely`, `path`; the files are `card_N.mrc` in
  `"cardsDir"`). Copy them here, **show them to the user** in TifMrcView Pro
  (or look at them yourself if the user asked you to decide): pick the card
  that looks like the particle, never gold beads, ice or a line or layer.
  Then run the command in `"next"` (`tmv sta --pick N TOMOGRAM…`). It names
  the tomograms, so a tilt series is not reconstructed again. The cards are
  kept in `OUT/cards/` (delete it to make them again).

`tmv tomo --for sta` also makes a clear companion of the tomogram (twice the
voxel size, denoised and deconvolved) to look at and pick particles on, and
prints it as `"look"`: its `path`, and `scale` and `offset` with which a voxel
of the full-detail tomogram is `scale` × the voxel of the look tomogram +
`offset` (per axis x, y, z). Averaging still uses the full-detail tomogram.
`tmv sta` finds the cards and the particles on the look tomogram and averages
the full-detail one (`"look"` per tomogram in its output; `particles.csv`
stays in full-detail voxels); it finds the look record in the `*.tomo.json`
beside the tomogram, and goes on without it, with a line on stderr, if the
file is missing.

`tmv sta` takes tilt series (their tomogram is made first, exactly as
`tmv tomo --for sta` does) or tomograms that `tmv tomo --for sta` wrote, one
or several of the same specimen. It writes the cards to `OUT/cards/`, then,
with `--pick N` (or by itself when card 0 is the only `likely` one), the
average of everything that looks like card N: `average.mrc`, `half1.mrc`,
`half2.mrc`, `particles.csv` and `sta.json` in `OUT/` (`"open"` is
`average.mrc`; `"particles"` how many were averaged). The JSON also has
`"resolutionA"` (FSC 0.143 between the halves) with the curve `"fsc"`,
`"diameterA"` with `"diameterFrom": "card"` (1.33 × the card's size),
`"candidates"`, `"seconds"` (the whole run, a tomogram made first included)
and `"ctfLimited": true` when the FSC falls below 0.143 and then rises above
0.3 again at higher frequency: the resolution is then limited at a CTF zero
and the data holds more than the number says. (`"kinds"` and `"distinct"`
are left out; they belong to the blob search.) Progress on stderr counts the
steps of the whole run ("第 k/N 步 · …"); a line says which card was taken
and where the others are when `tmv sta` picks card 0 itself, and the last
line gives the resolution, the particle count and where `average.mrc` is.
The tilt range comes
from the `*.tomo.json` beside each tomogram; without one it assumes -60 to 60
and warns on stderr. Do not quote a resolution unless the user asks: report
the particle count and let them judge the average and the two halves.

## 6. Copy the results back and open them

Copy back the files named in the JSON, or the whole job folder without the
raw movies (`*.eer`, `*.tif(f)`, `*.frames.tif`, `*.mrcs`) and the motion-corrected
micrographs, e.g. (label maps
`*.labels.tif` are kept):

```sh
rsync -a --info=progress2 --include '*.labels.tif' --exclude '*.eer' --exclude '*.tif' --exclude '*.tiff' --exclude '*.mrcs' --exclude 'motion/' HOST:tmv-jobs/NAME/ ./NAME/
```

Then tell the user what to open in **TifMrcView Pro**:
- tomogram: the `"open"` file; its label map (`*.labels.tif`, materials
  膜 membrane / 囊泡 vesicle / 微管 microtubule) opens with it, and the 3D view
  shows them; vesicle sizes are in `*.vesicles.csv`;
- STA: `average.mrc` (and the half maps `half1.mrc`, `half2.mrc`); the
  picked particles are listed in `particles.csv`;
- SPA: the sharpened map (`"map"`) in the 3D view; opening the folder in the
  SPA workspace shows every step (project files move with the folder);
- the cryo-ET workspace likewise reopens the dataset (`*.tomo.json`).

Report briefly: what ran, how long, the key numbers from the JSON, and the
paths. Remove large intermediate files on the server only if the user agrees.
