# ✦ motivator

**A friend in the corner of your desktop. It says the things they'd actually say.**

[![CI](https://github.com/gitu/motivator/actions/workflows/ci.yml/badge.svg)](https://github.com/gitu/motivator/actions/workflows/ci.yml)
[![Release](https://github.com/gitu/motivator/actions/workflows/release.yml/badge.svg)](https://github.com/gitu/motivator/releases)

![motivator demo — a corner avatar popping up motivational lines](assets/demo.gif)

*(demo avatar: an AI-generated person — StyleGAN2,
[public domain via Wikimedia Commons](https://commons.wikimedia.org/wiki/File:GAN_Mensch_StyleGAN2.png) —
cut out and mouth-line-detected fully automatically by the widget's photo pipeline)*

A Rust implementation of the *Motivator Widget* design (claude.ai/design,
schr.ag design system). A small always-on-top, frameless, transparent widget
sits in a screen corner. Poke the avatar (or let it nudge you on a schedule)
and it pops up a speech bubble for a few seconds with a line your friend would
actually say — with a little talking animation. Teach it which lines land with
↑ more / ↓ less.

## features

- **movable widget** — frameless, transparent, always-on-top; drag the friend
  anywhere (position persists) or snap it to a corner in config → behavior.
  Bubbles and panels open toward free screen space and the window is kept
  fully on-screen as panels open and close
- **speech bubble** — pops up for a configurable number of seconds when the
  avatar talks (poke, → next, or scheduled nudges); hovering keeps it up
- **quote learning** — ↑ more / ↓ less weights each line (0 = muted, never
  repeats); rotation is weighted random, never repeating the current line
- **chat** — talk to your friend; replies come from an OpenAI-compatible
  endpoint in *their* voice (learned from their sample lines), with canned
  fallbacks when no endpoint is configured or the call fails
- **friends roster** — multiple friends, each with their own lines, accent
  color, photo, and nudge schedule
- **schedule** — time windows that hand the widget to a different friend:
  the work friend 09:00–17:00 on workdays, the coach over lunch, the
  wind-down friend in the evening; overlaps resolve to the shortest window
- **photo avatars** — upload a photo; the background is removed automatically
  (flood fill from the borders) and the mouth is located by real face
  detection (embedded SeetaFace model, pure Rust — silhouette heuristic as
  fallback) so the head's top half can flap while talking, jaw-snap style
- **photo control** — pick per friend how uploads are processed: *auto
  cut-out*, *already cut out* (keeps your PNG's transparency, skips the flood
  fill), or *keep as-is* (stored untouched, no resize, no detection); a mouth
  line slider corrects the detected flap hinge when needed
- **animation styles** — per friend: a talking style (jaw-flap, bounce, sway,
  two-frame swap with an uploaded mouth-open still, or none) plus a continuous
  idle animation (breathe, sway, or *alive*) so the avatar never sits frozen
- **animated avatars** — upload an animated GIF / APNG / animated WebP and it
  plays as the avatar, background cut-out and all (up to 48 frames)
- **friend cards** — share a friend as a PNG with their whole config (quotes,
  weights, photo, behavior) embedded in the pixels; copy/paste or send as a
  file, import via friends → paste card
- **sizes** — avatar size 56–96 px, bubble duration, corner, all in config →
  behavior; dark/light theme follows the system preference automatically
- **start on login** — optional autostart toggle in config → behavior (XDG
  autostart on Linux, login item on macOS, registry Run key on Windows)

## install

macOS, via [Homebrew](https://brew.sh):

```sh
brew install --cask gitu/tap/motivator
```

The cask puts `motivator` on your PATH and clears the quarantine flag during
install — start it from a terminal with `motivator`.

macOS / Linux, via install script (installs to `~/.local/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/gitu/motivator/main/scripts/install.sh | bash
```

Windows, via PowerShell (installs to `%LOCALAPPDATA%\Programs\motivator`):

```powershell
irm https://raw.githubusercontent.com/gitu/motivator/main/scripts/install.ps1 | iex
```

Or grab a binary from the [latest release](https://github.com/gitu/motivator/releases)
(`.tar.gz` for macOS / Linux, `.zip` for Windows), unpack, run.

Builds are currently unsigned. On macOS the first run needs the quarantine
flag cleared (`xattr -d com.apple.quarantine motivator` — the cask and
install script do this for you); on Windows, allow the SmartScreen prompt.

Or build from source:

```sh
cargo build --release
./target/release/motivator
```

Requires only a working display (X11 or Wayland/XWayland) — no webview,
no system tray, no daemons. The opt-in *start on login* toggle registers a
plain autostart entry with your desktop; it doesn't run anything in the
background either.

## AI endpoint (OpenAI-compatible, static token)

Configure under **config → api** in the widget, or via environment:

```sh
MOTIVATOR_BASE_URL=https://api.openai.com/v1   # any /v1-compatible server
MOTIVATOR_API_KEY=sk-...                        # static bearer token ("" for local servers)
MOTIVATOR_MODEL=gpt-4o-mini
```

Works with any OpenAI-compatible `/chat/completions` server: OpenAI, a local
llama.cpp / Ollama (`http://localhost:11434/v1`), vLLM, LiteLLM, etc. The AI
powers chat replies and *generate 3 with ai* in config → quotes. Without an
endpoint everything still works with canned lines. The generation batch size
(3 / 5 / 10 lines per click) is selectable next to the generate button;
duplicates of existing lines are skipped automatically.

Settings are stored in `~/.config/motivator/config.json` (mode 0600, since it
may contain the token). Processed photos live in
`~/.local/share/motivator/photos/`.

## sharing friends

**config → friend → copy card** puts a *friend card* on the clipboard — a PNG
of your friend with their entire config (name, quotes and learned weights,
accent, behavior, and the cut-out photo) steganographically embedded in the
pixels' low bits. **save card…** writes the same PNG to disk. The recipient
imports it via **friends → paste card** (or **open card…**) and gets the
friend exactly as trained. Your LLM settings — API url, token, and model —
are global config, not friend data, and are never part of a card.

Because the data lives in the pixels, it survives clipboard round-trips and
PNG re-saves — but **not** lossy paths: screenshots of the card, resizing, or
apps that convert images to JPEG (some messengers do) destroy it. When in
doubt, send the `.png` as a file attachment.

Headless equivalents: `motivator --share <friend-id> <out.png>` and
`motivator --import <card.png>`.

## schedule: the right friend at the right time

![the schedule tab — three time windows handing the widget to different friends,
with the currently active window shown at the top](assets/schedule-tab.png)

*(the wind-down window is active, so leo has taken over and tells you to
shut it down — the work and sport windows will bring back marc and coach k
tomorrow)*

**config → schedule** holds a list of time windows, each handing the widget
to one friend: pick the days, the start/end time, and who takes over. Turn on
*switch friends on a schedule* and the avatar switches by itself — the new
friend greets you and nudges on their own interval for the whole window.

The rules are simple:

- **shortest window wins** — a 12:00–13:00 sport window inside a 09:00–17:00
  work window takes over for lunch; work resumes at 13:00
- **hand picks hold until the next boundary** — choosing a friend yourself
  mid-window sticks until any window starts or ends, then the schedule
  takes over again
- **midnight is fine** — a 22:00–01:00 window simply runs into the next day
- outside every window (and with the switch off) nothing changes: you keep
  whoever you picked

The windows are stored in the config file in a hand-editable form, so you can
also maintain them in `~/.config/motivator/config.json` directly:

```json
"schedule_enabled": true,
"schedule": [
  { "label": "work",  "friend": "marc",  "days": ["mon","tue","wed","thu","fri"],
    "start": "09:00", "end": "17:00", "enabled": true },
  { "label": "sport", "friend": "coach", "days": ["mon","tue","wed","thu","fri"],
    "start": "12:00", "end": "13:00", "enabled": true },
  { "label": "wind down — pc away", "friend": "ana",
    "days": ["mon","tue","wed","thu","fri","sat","sun"],
    "start": "18:00", "end": "22:00", "enabled": true }
]
```

A fresh install ships these three windows as a template with the master
switch off; existing configs are left untouched.

## Wayland note

Wayland compositors don't let apps position their own windows, so by default
motivator relaunches through XWayland (`prefer_x11: true` in the config),
where corner anchoring and always-on-top work. Set it to `false` to run
native-Wayland — then use your compositor's window rules to pin the widget
(app id: `motivator`).

## usage

- **click the avatar** — it speaks
- **↑ more / ↓ less** — teach it which lines land; a line weighted to 0 is
  muted and struck through in config → quotes
- **→ next** — another line
- **right-click the avatar** — chat, friends roster, config, quit; nothing
  but the face is shown until you ask for it
- **config → schedule** — let different friends take over at different times
  of day

## license

[MIT](LICENSE). The embedded face-detection model
(`assets/seeta_fd_frontal_v1.0.bin`) is the SeetaFace frontal model by the
VIPL group (ICT, Chinese Academy of Sciences), BSD-2-Clause, via the
[rustface](https://github.com/atomashpolskiy/rustface) project.
