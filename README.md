# Tremista

A Wayland desktop that aims to be faster than XFCE and better looking than
GNOME, with macOS behaviour throughout.

It is not a compositor. Window management is Wayfire — a mature wlroots
compositor with floating windows, GPU-accelerated blur and animation, and an
in-compositor Exposé — configured to behave like macOS. Everything Wayfire does
not provide is a separate layer-shell client, starting with the dock.

## What v1 does

**Dock.** A `wlr-layer-shell` panel across the bottom of the screen: squircle
plate, cursor-following magnification with a raised-cosine falloff, running
indicators, launch bounce, and click-to-focus/minimise through
`wlr-foreign-toplevel-management`. Minimising animates into the app's own icon.

**Mission Control.** Wayfire's `scale` plugin, triggered by **moving the mouse
into the top-right corner** — a 12×12 px hotspot with a 180 ms dwell, so
crossing the corner on the way somewhere else does not fire it. Also on
`Super+↑` and a three-finger swipe up. `Super+↓` gives the all-workspaces view,
and four workspaces are switched with `Ctrl+Alt+←/→` or a four-finger swipe.

Other macOS defaults: traffic lights on the left, natural scrolling, tap to
click, `Super+Space` for the launcher, `Super+Shift+3`/`4` for screenshots.

Not in v1: the global menu bar, and a real Spotlight (wofi stands in).

## Layout

    config/wayfire.ini             the macOS-tuned compositor config
    crates/tremista-dock-core/     layout, rendering, icons, .desktop parsing
    crates/tremista-dock/          the Wayland client around it
    scripts/install.sh             build + install

The core crate has no Wayland dependency, so it builds and its tests run on any
platform. That is what makes the look iterable without a compositor:

    cargo run -p tremista-dock-core --example preview -- /tmp/dock

writes six PNGs — resting, hovering at each end and in the middle, and
mid-bounce.

## Install

    ./scripts/install.sh --deps

Installs into `~/.local`, adds a session file, and writes
`~/.config/tremista/{wayfire.ini,dock.conf}`. An existing `wayfire.ini` is
backed up rather than replaced; `dock.conf` is only seeded if absent. Then log
out and pick **Tremista**, or try it from a running session with:

    WAYFIRE_CONFIG_FILE=~/.config/tremista/wayfire.ini wayfire

## Testing without logging out

Wayfire is a wlroots compositor, so it can nest inside the session you are
already in. From a terminal on the machine itself:

    ./scripts/test-nested.sh --repo

That opens the whole desktop in a window. Nothing about the running session is
touched, and closing the window ends the test.

Over SSH there is no display to nest into, so that script refuses to start.
Either export the physical session's socket first:

    export XDG_RUNTIME_DIR=/run/user/$(id -u) WAYLAND_DISPLAY=wayland-0

— the window then appears on the machine's own monitor, not in your terminal —
or render to memory and screenshot the result:

    ./scripts/test-headless.sh

Headless proves the compositor starts, the dock connects and the icons resolve.
It cannot show anything driven by a pointer, because there is no seat.

## Configuring the dock

`~/.config/tremista/dock.conf` is one `.desktop` id per line, top to bottom =
left to right. Ids that are not installed are skipped. Drop a wallpaper at
`~/.config/tremista/wallpaper.jpg`.
