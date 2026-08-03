#!/bin/sh
# Run Tremista with no display at all and screenshot it. For testing over SSH.
#
# wlroots can render to memory instead of to hardware, which is enough to prove
# the compositor starts, the dock connects, and the icons resolve. What it
# cannot show you is anything driven by a pointer -- magnification, hover,
# clicks, the hot corner -- because a headless backend has no seat.
#
#   ./scripts/test-headless.sh [output.png]
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-$repo/tremista-headless.png}

config="$HOME/.config/tremista/wayfire.ini"
[ -f "$config" ] || config="$repo/config/wayfire.ini"

command -v wayfire >/dev/null 2>&1 || { echo "wayfire is not installed" >&2; exit 1; }
command -v grim    >/dev/null 2>&1 || { echo "grim is not installed" >&2; exit 1; }

cargo build --release --manifest-path "$repo/Cargo.toml"

runtime=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
[ -d "$runtime" ] || { echo "no XDG_RUNTIME_DIR at $runtime" >&2; exit 1; }
export XDG_RUNTIME_DIR="$runtime"

# Note which sockets already exist, so we can tell which one is ours. Wayfire
# takes the first free wayland-N and gives us no way to choose it, so spotting
# the new one is the only way to point grim at the right compositor.
before=$(ls "$runtime" | grep '^wayland-[0-9]*$' || true)

PATH="$repo/target/release:$PATH"
export PATH
export WLR_BACKEND=headless
export WLR_HEADLESS_OUTPUTS=1
export WAYFIRE_CONFIG_FILE="$config"
export RUST_LOG="${RUST_LOG:-tremista_dock=debug,info}"
# Unset so wayfire does not try to attach to a session that is not there.
unset WAYLAND_DISPLAY DISPLAY 2>/dev/null || true

echo "==> starting headless wayfire"
wayfire &
wayfire_pid=$!
# shellcheck disable=SC2064  # $wayfire_pid must expand now, not at trap time
trap "kill $wayfire_pid 2>/dev/null || true" EXIT INT TERM

socket=""
i=0
while [ $i -lt 30 ]; do
    for s in $(ls "$runtime" | grep '^wayland-[0-9]*$' || true); do
        echo "$before" | grep -qx "$s" || socket=$s
    done
    [ -n "$socket" ] && break
    kill -0 "$wayfire_pid" 2>/dev/null || { echo "wayfire exited early" >&2; exit 1; }
    sleep 0.5
    i=$((i + 1))
done
[ -n "$socket" ] || { echo "wayfire never opened a socket" >&2; exit 1; }
echo "==> compositor on \$$socket; giving the dock a moment to draw"

# The dock is started from [autostart] and has to resolve and rasterise every
# icon before its first frame, which is by far the slowest part of startup.
sleep 4

WAYLAND_DISPLAY=$socket grim "$out"
echo
echo "Wrote $out"
echo "Copy it back with:  scp <this-host>:$out ."
