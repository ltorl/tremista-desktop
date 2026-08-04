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
    deps="wayfire wf-config swaybg foot wofi grim slurp wl-clipboard wireplumber xdg-desktop-portal-wlr ttf-inter papirus-icon-theme"
    install_cmd="sudo pacman -S --needed $deps"
elif command -v apt-get >/dev/null 2>&1; then
    pm="apt"
    deps="wayfire swaybg foot wofi grim slurp wl-clipboard wireplumber xdg-desktop-portal-wlr fonts-inter papirus-icon-theme"
    install_cmd="sudo apt-get install -y $deps"
elif command -v dnf >/dev/null 2>&1; then
    pm="dnf"
    deps="wayfire swaybg foot wofi grim slurp wl-clipboard wireplumber xdg-desktop-portal-wlr rsms-inter-fonts papirus-icon-theme"
    install_cmd="sudo dnf install -y $deps"
else
    pm=""
fi

if [ -z "$pm" ]; then
    echo "note: unrecognised package manager; install wayfire, swaybg, foot," >&2
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
# Ids that are not installed are skipped silently.
firefox
org.gnome.Nautilus
foot
org.gnome.TextEditor
org.gnome.Settings
EOF
fi

# Seed the default wallpaper, but only when the user has none -- replacing one
# they chose themselves would be the same mistake as clobbering their config.
if ! ls "$config_dir"/wallpaper.* >/dev/null 2>&1; then
    install -Dm644 "$repo/assets/wallpaper.png" "$config_dir/wallpaper.png"
    echo "    installed the default wallpaper (replace $config_dir/wallpaper.*"
    echo "    with any image to change it)"
fi

case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "    note: $prefix/bin is not on your PATH" ;;
esac

echo
echo "Done. Log out and pick \"Tremista\" at the login screen."
echo "To try it without logging out, from an existing session run:"
echo "    WAYFIRE_CONFIG_FILE=$config_dir/wayfire.ini wayfire"
