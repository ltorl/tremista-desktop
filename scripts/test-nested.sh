#!/bin/sh
# Run Tremista in a window inside your current desktop session.
#
# Wayfire is a wlroots compositor, so it can use the session you are already in
# as its backend instead of taking over the hardware. Nothing about the running
# desktop is touched, and closing the window ends the test.
#
#   ./scripts/test-nested.sh            use the installed config
#   ./scripts/test-nested.sh --repo     use config/wayfire.ini straight from the
#                                       source tree, without installing
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

config="$HOME/.config/tremista/wayfire.ini"
for arg in "$@"; do
    case "$arg" in
        --repo) config="$repo/config/wayfire.ini" ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

[ -f "$config" ] || { echo "no config at $config (run scripts/install.sh, or pass --repo)" >&2; exit 1; }
command -v wayfire >/dev/null 2>&1 || { echo "wayfire is not installed" >&2; exit 1; }

# Build first so a compile error shows up here rather than as a blank window.
cargo build --release --manifest-path "$repo/Cargo.toml"

# The dock is started by wayfire's [autostart], which runs commands through the
# shell, so putting our binary first on PATH is enough to pick up this build.
PATH="$repo/target/release:$PATH"
export PATH

# Pick the backend from whatever the outer session is.
if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    export WLR_BACKEND=wayland
elif [ -n "${DISPLAY:-}" ]; then
    export WLR_BACKEND=x11
else
    echo "no WAYLAND_DISPLAY or DISPLAY; run this from inside a desktop session" >&2
    exit 1
fi

# Nested wlroots cannot use the host's hardware cursor plane.
export WLR_NO_HARDWARE_CURSORS=1
export WAYFIRE_CONFIG_FILE="$config"
# Keep the nested session out of the outer one's runtime namespace, so the two
# compositors do not fight over the same socket name.
export XDG_CURRENT_DESKTOP=Tremista:wlroots

# The dock logs at info by default; debug also reports every icon it fails to
# find, which is what you want the first time.
export RUST_LOG="${RUST_LOG:-tremista_dock=debug,info}"

# Show the repo's wallpaper without installing anything, so a --repo run looks
# like the real thing. An installed wallpaper still wins if you set this empty.
export TREMISTA_WALLPAPER="${TREMISTA_WALLPAPER:-$repo/assets/wallpaper.png}"
# Likewise for the bundled app icons, which otherwise only exist once installed.
export TREMISTA_ICONS="${TREMISTA_ICONS:-$repo/assets/icons}"

echo "==> starting nested Wayfire (close the window to stop)"
exec wayfire
