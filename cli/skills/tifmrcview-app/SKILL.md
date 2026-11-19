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
- Windows: `"C:\Program Files\TifMrcView Pro\tifmrcview-pro.exe" tmv …`
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
`polygon=[[10,10],[60,10],[60,60]]`). The answer is one JSON object; exit
status 0 when `"ok": true`. `"error"` is in Chinese: translate it for the
user if needed.

The `tmv` commands exist in the window under the same name and do the same
thing there: `tmv app tomo path=…` (Cryo-ET panel, reconstruct the ticked
series; `all=true` ticks all; `for=survey|display`), `tmv app sta`,
`tmv app spa path=…`, `tmv app ctf path=…`, `tmv app membranes path=…`,
`tmv app denoise path=…`, `tmv app info`, `tmv app setup sam=true`,
`tmv app run path=…`. Other tools open with `tmv app workspace name=<id>`
(`help` lists the ids) and are then driven with `ui` / `click` / `set`.
Label layers: `tmv app labels`, `tmv app label_set label=2 name=膜 hidden=false`,
`tmv app label_paint label=1 z=40 polygon=[[x,y],…]` (image pixels;
undoable with the window's undo).

Rules, as in the app: never invent processing parameters (the app chooses
them); what to average or which particle kind to keep is the user's choice:
after `sta` or `spa` stops at its cards, `screenshot` / describe them, ask
the user, then click their choice. Save dialogs and file pickers are the
user's: when one opens, tell them what to pick. Long jobs: `wait` in steps
and report progress from `"status"`.
