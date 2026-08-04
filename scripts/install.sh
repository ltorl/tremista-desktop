#!/bin/sh
# Build and install Tremista.
#
#   ./scripts/install.sh            build, then install into the user's home
#   ./scripts/install.sh --deps     also install distro packages (needs sudo)
#   ./scripts/install.sh --system   put the binaries in /usr/local instead
#
# Everything except --system stays inside $HOME, so an install can be undone by
# deleting ~/.config/tremista and ~/.local/bin/tremista-*.
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

install_deps=0
prefix="$HOME/.local"
sessions_dir="$HOME/.local/share/wayland-sessions"

for arg in "$@"; do
    case "$arg" in
        --deps) install_deps=1 ;;
        --system)
            prefix=/usr/local
            # Display managers only scan the system directory reliably; the
            # per-user one needs a recent GDM/SDDM.
            sessions_dir=/usr/share/wayland-sessions
            ;;
        -h|--help) sed -n '2,9p' "$0" | cut -c3-; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

# --- dependencies ----------------------------------------------------------

# Split by package manager rather than by distro name: derivatives (Mint, Pop,
# EndeavourOS, Nobara) then work without being listed.
if command -v pacman >/dev/null 2>&1; then
    pm="pacman"
    deps="wayfire wf-config swaybg chromium gnome-terminal wofi grim slurp wl-clipboard wireplumber xdg-desktop-portal-wlr ttf-inter papirus-icon-theme"
    install_cmd="sudo pacman -S --needed $deps"
elif command -v apt-get >/dev/null 2>&1; then
    pm="apt"
    deps="wayfire swaybg chromium gnome-terminal wofi grim slurp wl-clipboard wireplumber xdg-desktop-portal-wlr fonts-inter papirus-icon-theme"
    install_cmd="sudo apt-get install -y $deps"
elif command -v dnf >/dev/null 2>&1; then
    pm="dnf"
    deps="wayfire swaybg chromium gnome-terminal wofi grim slurp wl-clipboard wireplumber xdg-desktop-portal-wlr rsms-inter-fonts papirus-icon-theme"
    install_cmd="sudo dnf install -y $deps"
else
    pm=""
fi

if [ -z "$pm" ]; then
    echo "note: unrecognised package manager; install wayfire, swaybg, a terminal," >&2
    echo "      wofi, grim, slurp, wireplumber and an icon theme yourself." >&2
elif [ "$install_deps" -eq 1 ]; then
    echo "==> installing packages with $pm"
    $install_cmd
else
    echo "==> skipping packages (pass --deps to install them):"
    echo "      $install_cmd"
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found; install Rust from https://rustup.rs" >&2
    exit 1
fi

# --- build -----------------------------------------------------------------

echo "==> building"
cargo build --release --manifest-path "$repo/Cargo.toml"

# --- install ---------------------------------------------------------------

echo "==> installing into $prefix"
install -Dm755 "$repo/target/release/tremista-dock" "$prefix/bin/tremista-dock"
install -Dm755 "$repo/scripts/tremista-session"    "$prefix/bin/tremista-session"
install -Dm644 "$repo/scripts/tremista.desktop"    "$sessions_dir/tremista.desktop"

config_dir="$HOME/.config/tremista"
mkdir -p "$config_dir"

# Never clobber a config the user has edited: back it up and report it, so a
# reinstall cannot silently discard their keybindings.
if [ -f "$config_dir/wayfire.ini" ] &&
   ! cmp -s "$repo/config/wayfire.ini" "$config_dir/wayfire.ini"; then
    backup="$config_dir/wayfire.ini.bak.$(date +%Y%m%d%H%M%S)"
    cp "$config_dir/wayfire.ini" "$backup"
    echo "    kept your old config at $backup"
fi
install -Dm644 "$repo/config/wayfire.ini" "$config_dir/wayfire.ini"

# The pinned-app list is pure user data, so only seed it the first time.
if [ ! -f "$config_dir/dock.conf" ]; then
    cat > "$config_dir/dock.conf" <<'EOF'
# One .desktop id per line, top to bottom = left to right in the dock.
# Ids that are not installed are skipped silently. "a|b" pins the first of
# a and b that is installed.
# Chromium's id differs between Debian, the Ubuntu snap and Flathub.
chromium|chromium-browser|chromium_chromium|org.chromium.Chromium
org.gnome.Nautilus
# The GNOME terminal has been renamed twice; the first id installed wins.
org.gnome.Terminal|org.gnome.Ptyxis|org.gnome.Console
org.gnome.Settings
# "All Apps" is added automatically at the right end; it is not listed here.
EOF
fi

# Seed the default wallpaper, but only when the user has none -- replacing one
# they chose themselves would be the same mistake as clobbering their config.
if ! ls "$config_dir"/wallpaper.* >/dev/null 2>&1; then
    install -Dm644 "$repo/assets/wallpaper.png" "$config_dir/wallpaper.png"
    echo "    installed the default wallpaper (replace $config_dir/wallpaper.*"
    echo "    with any image to change it)"
fi

# Bundled app icons. Unlike the wallpaper these are installed every time: the
# directory is ours, and a user who wants a different icon adds their own file
# under a different name rather than editing one of these.
for icon in "$repo"/assets/icons/*; do
    # `if` rather than `[ -f ] &&`: under set -e a false test on the last
    # iteration would abort the script, which an unmatched glob would cause.
    if [ -f "$icon" ]; then
        install -Dm644 "$icon" "$config_dir/icons/$(basename "$icon")"
    fi
done

case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "    note: $prefix/bin is not on your PATH" ;;
esac

echo
echo "Done. Log out and pick \"Tremista\" at the login screen."
echo "To try it without logging out, from an existing session run:"
echo "    WAYFIRE_CONFIG_FILE=$config_dir/wayfire.ini wayfire"
