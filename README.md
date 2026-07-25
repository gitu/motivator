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

- **corner widget** — frameless, transparent, always-on-top; anchors to any of
  the four screen corners and resizes dynamically as panels open and close
- **speech bubble** — pops up for a configurable number of seconds when the
  avatar talks (poke, → next, or scheduled nudges); hovering keeps it up
- **quote learning** — ↑ more / ↓ less weights each line (0 = muted, never
  repeats); rotation is weighted random, never repeating the current line
- **chat** — talk to your friend; replies come from an OpenAI-compatible
  endpoint in *their* voice (learned from their sample lines), with canned
  fallbacks when no endpoint is configured or the call fails
- **friends roster** — multiple friends, each with their own lines, accent
  color, photo, and nudge schedule
- **photo avatars** — upload a photo; the background is removed automatically
  (flood fill from the borders) and the mouth is located by real face
  detection (embedded SeetaFace model, pure Rust — silhouette heuristic as
  fallback) so the head's top half can flap while talking, jaw-snap style
- **friend cards** — share a friend as a PNG with their whole config (quotes,
  weights, photo, behavior) embedded in the pixels; copy/paste or send as a
  file, import via friends → paste card
- **sizes** — avatar size 56–96 px, bubble duration, corner, all in config →
  behavior; dark/light theme follows the system preference automatically

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
no system tray, no daemons.

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

## license

[MIT](LICENSE). The embedded face-detection model
(`assets/seeta_fd_frontal_v1.0.bin`) is the SeetaFace frontal model by the
VIPL group (ICT, Chinese Academy of Sciences), BSD-2-Clause, via the
[rustface](https://github.com/atomashpolskiy/rustface) project.
