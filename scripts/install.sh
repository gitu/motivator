#!/usr/bin/env bash
# Install motivator from the latest GitHub release (macOS / Linux).
#
#   curl -fsSL https://raw.githubusercontent.com/gitu/motivator/main/scripts/install.sh | bash
#
# Options via environment:
#   MOTIVATOR_VERSION      release to install, e.g. 0.1.0 (default: latest)
#   MOTIVATOR_INSTALL_DIR  target directory (default: ~/.local/bin)
set -euo pipefail

repo="gitu/motivator"
version="${MOTIVATOR_VERSION:-latest}"
install_dir="${MOTIVATOR_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux)  os=linux ;;
  *) echo "error: unsupported OS $(uname -s) — use scripts/install.ps1 on Windows" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64)  arch=x86_64 ;;
  *) echo "error: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
if [ "$os/$arch" = "linux/aarch64" ]; then
  echo "error: no prebuilt Linux aarch64 binary — build from source: cargo build --release" >&2
  exit 1
fi

asset="motivator-${arch}-${os}.tar.gz"
if [ "$version" = "latest" ]; then
  url="https://github.com/${repo}/releases/latest/download/${asset}"
else
  url="https://github.com/${repo}/releases/download/v${version#v}/${asset}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading ${url}"
curl -fSL --progress-bar -o "${tmp}/${asset}" "$url"
tar xzf "${tmp}/${asset}" -C "$tmp"

mkdir -p "$install_dir"
install -m 755 "${tmp}/motivator" "${install_dir}/motivator"
if [ "$os" = macos ]; then
  xattr -d com.apple.quarantine "${install_dir}/motivator" 2>/dev/null || true
fi

echo "installed ${install_dir}/motivator"
case ":$PATH:" in
  *":${install_dir}:"*) ;;
  *) echo "note: ${install_dir} is not on your PATH" ;;
esac
