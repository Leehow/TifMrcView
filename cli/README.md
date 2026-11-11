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
| `tmv tomo TS.mrc\|TS.mdoc [--for display\|sta\|survey]` | One-click cryo-ET: motion correction (from an mdoc), alignment, CTF, reconstruction; for display also denoising, missing-wedge filling and membranes | `*_rec*.mrc`, `*.labels.tif`, RELION 5 folder and a clear `look` tomogram (sta) |
| `tmv sta TS.mrc\|TOMO.mrc [MORE …] [--pick N] [--out DIR]` | Subtomogram averaging: class-average "cards" of the particle-like blobs, each tried as a template, then the average of everything that looks like the card that gives the best average (or the one you pick) | `*_sta/cards/`, then `*_sta/average.mrc`, `average_raw.mrc`, `half1.mrc`, `half2.mrc`, `particles.csv`, `sta.json` |
| `tmv membranes TOMO.mrc [--model membrain.ckpt]` | Membranes, vesicles (split and measured) and microtubules | `TOMO.mrc.labels.tif`, `TOMO.mrc.membranes.json`, `TOMO.mrc.vesicles.csv` |
| `tmv spa FOLDER [--kind N] [--symmetry D7]` | One-click single-particle analysis | `FOLDER/tifmrcview-spa/` with the sharpened map |
| `tmv ctf FOLDER` | CTF of every micrograph (movies motion corrected first) | `FOLDER/tifmrcview-spa/ctf.tsv` |
| `tmv denoise FILE` | Self-supervised (Noise2Void) denoising trained on the file | `FILE_denoised.mrc` |
| `tmv info` | Version, GPU, models | |
| `tmv setup --membrane FILE.ckpt` / `--sam` | Remember the MemBrain-seg model / download SAM 2.1 | |

`tmv tomo --for sta` also makes a clear companion of the tomogram (twice the
voxel size, denoised and deconvolved) to look at and pick particles on, and
prints it as `"look"`: its `path`, and `scale` and `offset` with which a voxel
of the full-detail tomogram is `scale` × the voxel of the look tomogram +
`offset` (per axis x, y, z). Anything that becomes a card or an average comes from the full-detail tomogram.
`tmv sta` makes the cards from the full-detail tomogram only (the look one
loses the internal structure that classes and averages need), searches the
look tomogram for particles with the card as template, and cuts, aligns and
averages the full-detail one (`"look"` per tomogram in its output;
`particles.csv` stays in full-detail voxels); it finds the look record in the
`*.tomo.json` beside the tomogram, and goes on without it, with a line on
stderr, if the file is missing.

`tmv sta` takes tilt series (their tomogram is made first, as `tmv tomo --for
sta` does) or tomograms that `tmv tomo --for sta` wrote, one or several. It
works in three steps. First it writes the cards to `OUT/cards/` (`card_N.mrc`,
32-voxel class averages, and `cards.json`; this takes minutes, so a finished
`cards.json` is reused only when `OUT/cards/inputs.json` shows it was made
from the same tomograms, unchanged; otherwise it is made again, with a line on
stderr saying why; delete `OUT/cards/` to force it). Then, unless you give
`--pick N`, it tries every card (at most 12) as the template of a coarse run
(`OUT/trials/card_N/`, the results kept in `OUT/cards/trials.json` and reused
with the cards), because a card is judged by what it gives, not by how it
looks: a card that is a layer, a line or a small blob finds no group of
particles that stands out and fails, a particle gives an average whose two
halves agree far out. The "agreement" of a trial is the area under the FSC of
its two halves over the spatial frequency (1/Å), which can be compared between
cards of different voxel sizes; one stderr line per trial gives it. Finally it
finds everything in the tomograms that looks like card N and averages it
into `OUT/`: N is `--pick N`, or by itself the card with the best agreement when
all the cards within 0.75 of it show one kind of particle (cards that show the
same particle, from different size groups, are one kind: each pair is compared
over all orientations, taking a few seconds). A line on stderr says which
cards matched. With several kinds among the good cards, or no card that
worked, it prints the trials and cards as JSON and
exits with status 2: look at the cards (open the `card_N.mrc` files in
TifMrcView Pro), then run the same command again with `--pick N`. Rerun it
with the tomograms it printed, not the tilt series, so they are not made
again. `OUT` defaults to a folder `<first input's name>_sta` beside the first
input. The tilt range of each tomogram comes from its `*.tomo.json` (the
views the alignment kept); without one, -60 to 60 is assumed, with a warning.
Whether the average is good is for you to judge from `average.mrc` (weighted by
the half agreement, sharpened and cut at the resolution; `average_raw.mrc` is the
plain average) and the two halves; `sta.json` has the numbers. The result (`OUT/sta.tmv.json`) has
`trials` (per card: `ok`, `reason`, `particles`, `agreement`, `fsc`, when the cards were tried), `resolutionA` (FSC 0.143 between the halves) with the curve `fsc`, `particles`,
`diameterA` with `diameterFrom: "card"` (1.33 × the card's size), `seconds`
(the whole run, a tomogram made first included), `searched` (`look` or `raw`:
which tomogram the template was searched on) with `searchVoxelA`, and
`ctfLimited: true` when the FSC has a dip that rises again (a local minimum
below 0.5 followed by a value 0.15 higher): the halves disagree at a zero of
the contrast transfer and agree again behind it. `resolutionA` is then the
dip, and `signalA` is where they last agree (finer; the data holds that
much). Both are steady from run to run, where a read at 0.143 jumped
between them as the dip fell just above or below the line. Progress on stderr counts the steps of the whole run ("第 k/N
步 · …"); `tmv sta` prints a line per card tried and, when it picks a card itself, a line saying which card and where
the others are, and the last line gives the resolution, the particle count and
where `average.mrc` is.

Everything is chosen automatically, as in the app. `--pixel Å` and
`--kv kV` are needed only when neither the files nor the config files beside
them carry the pixel size and voltage.

Results are written beside the input. The result of each run is printed as
one JSON object on stdout and saved as `INPUT.tmv.json` (`tmv tomo --for sta`
and `--for survey` save `INPUT.sta.tmv.json` and `INPUT.survey.tmv.json`, so
results for one stack do not overwrite each other; `tmv sta` saves
`OUT/sta.tmv.json`, so it does not overwrite the one `tmv tomo` made for a
tilt series). Exit status: 0 done,
1 error (`"error"`, `"hint"`), 2 a choice is needed (SPA: which kind of
particle, shown as PNG strips; STA: which card).

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
