//! Which apps are pinned to the dock.
//!
//! The file is a plain newline-separated list of `.desktop` ids rather than
//! TOML or JSON: it is the entire configuration surface, users edit it by hand,
//! and a one-value-per-line format cannot be got wrong.

use std::path::PathBuf;

/// Apps pinned when the user has no config file yet. Anything not installed is
/// silently dropped when the ids are resolved, so a generous list is safe.
const DEFAULTS: &[&str] = &[
    // Chromium is packaged under four different ids depending on whether it
    // came from Debian, an Ubuntu snap, or Flathub.
    "chromium|chromium-browser|chromium_chromium|org.chromium.Chromium",
    "org.gnome.Nautilus",
    // GNOME's terminal has been three different apps across releases; `|` picks
    // whichever one this machine actually has.
    "org.gnome.Terminal|org.gnome.Ptyxis|org.gnome.Console",
    "code",
    "org.gnome.Settings",
];

/// What the "All Apps" icon opens. Tried in order so the dock works with
/// whichever launcher is installed; `$TREMISTA_LAUNCHER` replaces the lot.
const LAUNCHERS: &[(&str, &str)] = &[
    ("wofi", "wofi --show drun"),
    ("fuzzel", "fuzzel"),
    ("rofi", "rofi -show drun"),
    ("bemenu-run", "bemenu-run"),
];

pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("tremista"))
}

pub fn pinned_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("dock.conf"))
}

/// The command the "All Apps" icon runs.
///
/// Resolved at startup rather than baked in, because none of these launchers is
/// present everywhere. A chain in `sh` would work too, but resolving here means
/// the log says which one was picked when the icon does nothing.
pub fn launcher_command() -> String {
    if let Some(custom) = std::env::var_os("TREMISTA_LAUNCHER") {
        let custom = custom.to_string_lossy().into_owned();
        if !custom.trim().is_empty() {
            return custom;
        }
    }
    for (binary, command) in LAUNCHERS {
        if which(binary) {
            return (*command).to_owned();
        }
    }
    log::warn!(
        "no app launcher found (tried {}); the All Apps icon will do nothing",
        LAUNCHERS.iter().map(|(b, _)| *b).collect::<Vec<_>>().join(", ")
    );
    String::new()
}

/// Is `binary` on PATH? A hand-rolled `which`, to avoid a crate for six lines.
fn which(binary: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(binary).is_file())
}

/// Directories searched for icon overrides, ahead of the system icon theme.
///
/// `$TREMISTA_ICONS` comes first so scripts/test-nested.sh can show the repo's
/// own icons without installing them; `~/.config/tremista/icons` is the one
/// users are told about.
pub fn icon_dirs() -> Vec<PathBuf> {
    std::env::var_os("TREMISTA_ICONS")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .into_iter()
        .chain(config_dir().map(|d| d.join("icons")))
        .collect()
}

/// Read the pinned app ids, falling back to [`DEFAULTS`] when there is no
/// config file. A file that exists but lists nothing yields an empty dock --
/// that is a deliberate choice by the user, not a reason to re-add defaults.
pub fn pinned() -> Vec<String> {
    let Some(path) = pinned_path() else {
        log::warn!("no HOME or XDG_CONFIG_HOME; using default pins");
        return DEFAULTS.iter().map(|s| (*s).to_owned()).collect();
    };

    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!("{} not found; using default pins", path.display());
            DEFAULTS.iter().map(|s| (*s).to_owned()).collect()
        }
        Err(e) => {
            log::error!("reading {}: {e}", path.display());
            DEFAULTS.iter().map(|s| (*s).to_owned()).collect()
        }
    }
}

fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        // `.desktop` is optional in the file; the resolver wants it off. A line
        // may hold `a|b` alternatives, so strip it from each one rather than
        // only from the end of the line.
        .map(|line| {
            line.split('|')
                .map(|id| id.trim().trim_end_matches(".desktop"))
                .filter(|id| !id.is_empty())
                .collect::<Vec<_>>()
                .join("|")
        })
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "\n# a comment\nfirefox\n\n  code  # trailing comment\n";
        assert_eq!(parse(text), vec!["firefox", "code"]);
    }

    #[test]
    fn desktop_suffix_is_optional() {
        assert_eq!(parse("code.desktop\ncode\n"), vec!["code", "code"]);
    }

    #[test]
    fn alternatives_are_normalised_individually() {
        assert_eq!(
            parse("org.gnome.Terminal.desktop | org.gnome.Console\n"),
            vec!["org.gnome.Terminal|org.gnome.Console"]
        );
        // A line of nothing but separators is not an app.
        assert!(parse("|  |\n").is_empty());
    }

    #[test]
    fn an_empty_file_means_an_empty_dock() {
        assert!(parse("# everything commented out\n").is_empty());
    }
}
