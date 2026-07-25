# ✦ motivator

**A friend in the corner of your desktop. It says the things they'd actually say.**

[![CI](https://github.com/gitu/motivator/actions/workflows/ci.yml/badge.svg)](https://github.com/gitu/motivator/actions/workflows/ci.yml)
[![Release](https://github.com/gitu/motivator/actions/workflows/release.yml/badge.svg)](https://github.com/gitu/motivator/releases)

![motivator demo — a corner avatar popping up motivational lines](assets/demo.gif)

*(demo avatar: an AI-generated person — StyleGAN,
[public domain via Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Woman_1.jpg) —
run through the widget's own photo pipeline)*

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
  (flood fill from the borders) and a mouth line is estimated so the head's
  top half can flap while talking, jaw-snap style
- **sizes** — avatar size 56–96 px, bubble duration, corner, dark/light theme,
  all in config → behavior

## install

Grab a binary from the [latest release](https://github.com/gitu/motivator/releases)
(`.tar.gz` for macOS / Linux, `.zip` for Windows), unpack, run. Builds are
unsigned — on macOS clear the quarantine flag first:
`xattr -d com.apple.quarantine motivator`.

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
- **chat · friends · config** — chips next to the avatar
- **right-click the avatar** — quit

## license

[MIT](LICENSE)
