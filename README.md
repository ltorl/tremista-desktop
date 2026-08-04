# Tremista

A Wayland desktop that aims to be faster than XFCE and better looking than
GNOME, with macOS behaviour throughout.

It is not a compositor. Window management is Wayfire — a mature wlroots
compositor with floating windows, GPU-accelerated blur and animation, and an
in-compositor Exposé — configured to behave like macOS. Everything Wayfire does
not provide is a separate layer-shell client, starting with the dock.

## What v1 does

**Dock.** A `wlr-layer-shell` panel across the bottom of the screen: squircle
plate over blurred wallpaper, cursor-following magnification with a
raised-cosine falloff, running indicators, launch bounce, optional auto-hide,
and click-to-focus/minimise through `wlr-foreign-toplevel-management`.
Minimising animates into the app's own icon.

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

## Configuring the dock

`~/.config/tremista/dock.conf` is one `.desktop` id per line, top to bottom =
left to right. Ids that are not installed are skipped, and a line may list
alternatives as `a|b|c` — the first installed one is pinned, which is how the
default list pins whichever GNOME terminal your release ships.

### Context menus

Right-clicking the dock's **background** — the plate, not an icon — offers
**Turn Magnification Off** and **Turn Hiding On** (or the reverse, depending on
where they currently stand). Both take effect immediately and are written to
`~/.config/tremista/settings.conf`, which is two `key = on` lines and can be
edited by hand instead. With hiding on the dock slides out of the bottom of the
screen, gives its screen space back to windows, and comes back when the pointer
touches the bottom edge.

Right-clicking an **app** offers **New Window**, **Pin to Dock** or **Unpin**,
and — for something that is running — **Quit**. The same menu appears on a
right-click in Launchpad, minus Quit: the grid is for starting things. Pinning
and unpinning rewrite `dock.conf`, which means the comments and `a|b|c`
alternatives in a hand-edited file are replaced by the ids actually in the dock.
`Escape` or a click elsewhere dismisses any of these menus.

### Launchpad

**All Apps** is always the rightmost icon and is not configurable away. Clicking
it opens **Launchpad**: a full-screen, paged grid of every installed app, drawn
by the dock itself. It is not a separate program and never has a window, so it
cannot appear in the dock or in a window switcher. Click an app to launch it,
click the background or press `Escape` to dismiss, and scroll or press the arrow
keys to change page. The dock stays visible and clickable along the bottom, so
clicking All Apps again also closes it.

Labels use the first suitable font it finds; set `$TREMISTA_FONT` to a `.ttf`,
`.otf` or `.ttc` file to pick one. With no font at all the grid still works, it
just has no captions. The All Apps icon is built into the binary; drop a
`tremista-launchpad.svg` in the icons directory below to replace it.

Icons come from the system icon theme, but `~/.config/tremista/icons` is
searched first: a file named after the app's `Icon=` name — `firefox.svg`,
`org.gnome.Nautilus.png` — replaces the theme's version of that icon. SVG wins
over a bitmap of the same name. `assets/icons/` is installed there.

The wallpaper is `~/.config/tremista/wallpaper.<ext>` — any format swaybg can
read. `assets/wallpaper.png` is installed there the first time, and never
overwritten afterwards.
