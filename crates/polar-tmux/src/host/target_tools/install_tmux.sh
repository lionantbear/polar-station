# Official payloads are staged by the Host on the original verified transport.
install_official_tmux() (
 set -eu
 : "${STATION_TMUX_ARCHIVE:?Official tmux payload is unavailable; prepare setup again.}"
 : "${STATION_TMUX_SHA256:?Missing official tmux digest}"
 : "${STATION_TMUX_LICENSES:?Missing official dependency notices}"
 digest() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1
  else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
 }
 [ "$(digest "$STATION_TMUX_ARCHIVE")" = "$STATION_TMUX_SHA256" ] || { echo 'Official tmux archive checksum mismatch.' >&2; exit 1; }
 [ "$(digest "$STATION_TMUX_LICENSES")" = "$STATION_TMUX_LICENSES_SHA256" ] || { echo 'Official tmux notices checksum mismatch.' >&2; exit 1; }
 station_bin="$HOME/.local/bin"
 mkdir -p "$station_bin" "$HOME/.local/share/tmux/licenses"
 station_unpack=$(mktemp -d "$station_bin/.tmux-install.XXXXXXXX")
 trap 'rm -rf -- "$station_unpack"' EXIT HUP INT TERM
 tar -xzf "$STATION_TMUX_ARCHIVE" -C "$station_unpack"
 chmod 755 "$station_unpack/tmux"
 [ "$("$station_unpack/tmux" -V)" = 'tmux 3.8' ] || { echo 'Official tmux cannot run on this target.' >&2; exit 1; }
 if [ -e "$station_bin/tmux" ] || [ -L "$station_bin/tmux" ]; then
  [ -x "$station_bin/tmux" ] && "$station_bin/tmux" -V || { echo 'Existing user tmux path is unavailable; it was preserved.' >&2; exit 1; }
 else
  tar -xzf "$STATION_TMUX_LICENSES" -C "$HOME/.local/share/tmux/licenses"
  # A hard-link publication fails atomically if another installer won the race.
  ln "$station_unpack/tmux" "$station_bin/tmux"
 fi
 case "${SHELL:-/bin/sh}" in
  */zsh) station_profile="$HOME/.zshrc";;
  */bash) station_profile="$HOME/.bashrc";;
  */fish) station_profile=''; mkdir -p "$HOME/.config/fish/conf.d"; printf 'fish_add_path --append "$HOME/.local/bin"\n' > "$HOME/.config/fish/conf.d/tmux-user-bin.fish";;
  *) station_profile="$HOME/.profile";;
 esac
 if [ -n "$station_profile" ] && ! grep -Fq 'export PATH="$PATH:$HOME/.local/bin"' "$station_profile" 2>/dev/null; then
  printf '\nexport PATH="$PATH:$HOME/.local/bin"\n' >> "$station_profile"
 fi
 "$station_bin/tmux" -V
 echo 'tmux is installed independently of Polar Station. Existing servers were not restarted.'
)
