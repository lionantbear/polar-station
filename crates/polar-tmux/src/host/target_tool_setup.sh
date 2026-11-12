#!/bin/sh
set -eu
resolve_codex() { @CODEX_RESOLVE@; command -v "$station_codex" >/dev/null 2>&1; }
as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }
install_package() {
 if [ "$(uname -s)" = Darwin ]; then
  for candidate in /opt/homebrew/bin/brew /usr/local/bin/brew; do
   if [ -x "$candidate" ]; then PATH="$(dirname "$candidate"):$PATH"; export PATH; break; fi
  done
  if ! command -v brew >/dev/null 2>&1; then
   station_brew=$(mktemp)
   curl --fail --show-error --location --proto '=https' --tlsv1.2 https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh -o "$station_brew"
   /bin/bash "$station_brew"; rm -f "$station_brew"
   if [ -x /opt/homebrew/bin/brew ]; then PATH="/opt/homebrew/bin:$PATH"; else PATH="/usr/local/bin:$PATH"; fi; export PATH
  fi
  brew install "$1"
 elif command -v apt-get >/dev/null 2>&1; then as_root apt-get update; as_root apt-get install -y "$1"
 elif command -v dnf >/dev/null 2>&1; then as_root dnf install -y "$1"
 elif command -v pacman >/dev/null 2>&1; then as_root pacman -S --needed --noconfirm "$1"
 elif command -v zypper >/dev/null 2>&1; then as_root zypper --non-interactive install "$1"
 elif command -v apk >/dev/null 2>&1; then as_root apk add "$1"
 else echo 'No supported system package manager available.' >&2; return 1; fi
}
if [ "$1" = 1 ]; then command -v tmux >/dev/null 2>&1 || install_package tmux; tmux -V; fi
if [ "$2" = 1 ]; then
 if ! resolve_codex; then
  command -v curl >/dev/null 2>&1 || install_package curl
  station_installer=$(mktemp); trap 'rm -f "$station_installer"' EXIT HUP INT TERM
  curl --fail --show-error --location --proto '=https' --tlsv1.2 https://chatgpt.com/codex/install.sh -o "$station_installer"
  /bin/sh "$station_installer"; rm -f "$station_installer"; trap - EXIT HUP INT TERM
 fi
 resolve_codex || { echo 'Installed Codex is unavailable.' >&2; exit 1; }
 "$station_codex" --version
 if [ "$3" = 1 ]; then
  "$station_codex" login status || "$station_codex" login
  "$station_codex" app-server daemon start
 fi
fi
