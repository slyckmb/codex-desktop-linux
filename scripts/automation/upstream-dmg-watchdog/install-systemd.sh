#!/usr/bin/env bash
# Installs the codex-desktop-watchdog systemd units as SYMLINKS to the
# repository-tracked copies under ~/.config/systemd/user/.
#
# Linking (rather than copying) means there is a single source of truth: the
# repo copy. Editing the repo unit updates the live config, so the live systemd
# config can never drift from what is committed. This mirrors the hashall
# systemd pattern already used on this host.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../../.." && pwd)"
unit_dir="$HOME/.config/systemd/user"
mkdir -p "$unit_dir"

for unit in codex-desktop-watchdog.service codex-desktop-watchdog.timer; do
  src="$repo_root/scripts/automation/upstream-dmg-watchdog/$unit"
  dst="$unit_dir/$unit"
  if [ -L "$dst" ]; then
    # Already a symlink; just point it at the repo copy (idempotent).
    ln -sfn "$src" "$dst"
  elif [ -f "$dst" ]; then
    # A real (non-symlink) copy exists; replace it with a symlink so future
    # drift is impossible.
    rm -f "$dst"
    ln -s "$src" "$dst"
  else
    ln -s "$src" "$dst"
  fi
  echo "linked $unit -> $src"
done

systemctl --user daemon-reload
echo "systemd reloaded; units are now symlinks to the repo."