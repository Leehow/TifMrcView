---
name: tifmrcview-app
description: Drive the TifMrcView Pro window open on the user's own computer (open files, switch tabs, press the Cryo-ET / subtomogram averaging / single-particle / denoising buttons, paint or name label layers, take a picture of the view) with `tmv app …` or the `tifmrcview` MCP server, so the user watches each step. Use when the user asks you to do something "in TifMrcView Pro" / "在 TifMrcView Pro 里…" on this computer; for a remote GPU server use the tifmrcview-remote skill instead.
---

# Driving the open TifMrcView Pro window (`tmv app`)

While TifMrcView Pro runs, it accepts calls from this computer only (port on
127.0.0.1, a token in its config folder that `tmv` reads). Each call is done
by the window's own buttons and fields: the control you press is outlined
and a strip at the bottom says what you are doing. The first call of a run
asks the user to allow you; if they refuse, stop and say so.

## Where `tmv` is

- `tmv` on the PATH (the app's ⚙ → AI 助手 → 安装 tmv 命令 links
  `~/.local/bin/tmv` to the app), or
- macOS: `"/Applications/TifMrcView Pro.app/Contents/MacOS/tifmrcview-pro" tmv …`
- Windows: `& "C:\Program Files\TifMrcView Pro\tifmrcview-pro.exe" tmv … | Out-String`
  in PowerShell, or the same path in Git Bash. Plain cmd.exe does not wait
  for it (it is a window program) and shows nothing; MCP is not affected.
- Linux: `tifmrcview-pro tmv …`

The same binary also runs every headless command (`tmv tomo`, `tmv sta`, …;
see the tifmrcview-remote skill). Use `tmv app` when the user wants to see it
happen or the work needs their eyes (choosing cards, painting labels); use
the headless commands for unattended batches.

With MCP instead of a shell (`claude mcp add tifmrcview -- <binary> tmv mcp`,
`codex mcp add tifmrcview -- <binary> tmv mcp`): tools `app_actions`,
`app_call {action, args}` and `app_screenshot` do the same.

## How to work

```sh
tmv app help                     # every action and its arguments, and the feature table
tmv app status                   # open image, tabs, open panels, running jobs, status lines
tmv app open path=/data/ts01.mrc # opens like the user would (new tab)
tmv app ui                       # the visible buttons, ticks, fields: ref, label, state, in=panel
tmv app ui in=sta-workspace      # only one panel
tmv app click text=找候选         # or ref=<ref from ui>
tmv app set ref=dn-method value=learned
tmv app wait --timeout 1800      # until no job runs (or text=… appears)
tmv app screenshot               # PNG of the image view (path in the JSON)
```

Arguments are `key=value` (JSON when it parses: `z=12`, `all=true`,
`'region=[10,10,60,60]'`; quote anything with brackets or spaces), and
`key=@file.json` reads a JSON file. The answer is one JSON object; exit
status 0 when `"ok": true`. `"error"` is in Chinese: translate it for the
user if needed.

The `tmv` commands exist in the window under the same name and do the same
thing there: `tmv app tomo path=…` (Cryo-ET panel, reconstruct the ticked
series; `all=true` ticks all; `for=survey|display`), `tmv app sta`,
`tmv app spa path=…`, `tmv app ctf path=…`, `tmv app membranes path=…`,
`tmv app denoise path=…`, `tmv app info`, `tmv app setup sam=true`,
`tmv app run path=…`. Other tools open with `tmv app workspace name=<id>`
(`help` lists the ids) and are then driven with `ui` / `click` / `set`.
Label layers: `tmv app labels` lists them, `tmv app label_set label=2
name=膜 hidden=false` renames or hides one.

## Segmenting by looking (you mark, the app makes the layers)

You can often see structures better than an automatic tool. The app gives
you a clear picture with coordinates and turns your marks into the same
label layers its Labels brush paints (`<image>.labels.tif`), which the user
sees at once, can undo in one step, and STA / SPA / the 3D view use.

1. **Look.** `tmv app look z=40` (or headless `tmv look IMAGE --z 40`): a
   PNG of the slice, contrast stretched, existing layers outlined in colour,
   a blue grid with image-pixel numbers (x along the top, y down the
   left). Start with the whole slice, then zoom where it matters:
   `tmv app look z=40 'region=[200,120,160,160]'` (x, y, width, height;
   quote arguments with brackets, zsh would expand them). Read
   coordinates off the grid; the JSON says how (`scale`, `region`).
2. **Mark**, one layer per kind of structure, named in the user's words:
   ```sh
   tmv app labels 'layers=[{"name":"线粒体","points":[[260,180,40],[410,95,40]],"size":60}]'
   tmv app labels layers=@marks.json      # same format as tmv labels --marks
   ```
   - `points` `[x, y, z]`: one point well inside each compact object (a
     vesicle, a particle, a mitochondrion); the smart selection tool finds
     its outline. Give `size` ≈ the object's diameter in pixels.
   - `polygons` `{"z", "pts": [[x,y],…]}`: an outline you trace yourself,
     8–40 points along the boundary, for objects the picker gets wrong.
   - `strokes` `{"z", "pts", "radius"}`: thin things (membranes,
     filaments): points along the centre line, `radius` ≈ half the
     thickness in pixels.
   - `"erase": true` on a layer takes those marks away (fix mistakes).
   - `"masks": ["mask.tif"]` beside `"layers"`: a whole mask you computed
     yourself (same size as the image, or a whole fraction) becomes layers.
3. **Check.** `look` again: your layers are outlined on the picture. Fix
   with more marks or `erase`, then go on to the next slices (marking every
   few slices and asking the user is better than guessing a whole volume).
4. **Say what you marked** and let the user correct it; they can paint over
   your layers with the brush.

Headless, the same: `tmv look IMAGE …`, `tmv labels IMAGE --marks
marks.json` (writes `<image>.labels.tif`, the layers the app opens).

Rules, as in the app: never invent processing parameters (the app chooses
them); what to average or which particle kind to keep is the user's choice:
after `sta` or `spa` stops at its cards, `screenshot` / describe them, ask
the user, then click their choice. Save dialogs and file pickers are the
user's: when one opens, tell them what to pick. Long jobs: `wait` in steps
and report progress from `"status"`.
