//! The two dock preferences the context menu toggles.
//!
//! Written back to disk the moment they change, in the same one-key-per-line
//! shape as `dock.conf`: the file is small enough that a format needing a parser
//! library would cost more than it is worth, and a user editing it by hand
//! cannot get `magnification = off` wrong.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Icons grow under the cursor. Off means the dock is a plain row.
    pub magnification: bool,
    /// The dock slides out of the bottom of the screen when unhovered.
    pub hiding: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // Magnification is on out of the box even though macOS ships it off:
        // it is the thing that makes this dock look like that dock, and the
        // menu is right there for anyone who dislikes it.
        Self {
            magnification: true,
            hiding: false,
        }
    }
}

fn path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("settings.conf"))
}

impl Settings {
    /// Read the settings, falling back to the defaults for anything missing or
    /// unreadable. A broken settings file must not stop the dock from starting.
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::error!("reading {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Persist the settings. Failures are logged, not propagated: a dock that
    /// refused to toggle magnification because `$HOME` is read-only would be
    /// worse than one that forgets the choice next time.
    pub fn save(self) {
        let Some(path) = path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::error!("creating {}: {e}", parent.display());
                return;
            }
        }
        let text = format!(
            "# Written by the dock's context menu; edit freely.\n\
             magnification = {}\n\
             hiding = {}\n",
            on_off(self.magnification),
            on_off(self.hiding),
        );
        if let Err(e) = std::fs::write(&path, text) {
            log::error!("writing {}: {e}", path.display());
        }
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

fn parse(text: &str) -> Settings {
    let mut settings = Settings::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // `true`/`1`/`yes` as well as `on`, because those are what people type.
        let value = match value.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => true,
            "off" | "false" | "no" | "0" => false,
            other => {
                log::warn!("settings: ignoring unrecognised value {other:?}");
                continue;
            }
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "magnification" => settings.magnification = value,
            "hiding" => settings.hiding = value,
            other => log::warn!("settings: ignoring unknown key {other:?}"),
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_leaves_the_defaults_alone() {
        assert_eq!(parse("# nothing here\n"), Settings::default());
    }

    #[test]
    fn keys_are_read_and_spelling_of_booleans_is_forgiving() {
        let s = parse("magnification = off\nhiding = TRUE\n");
        assert!(!s.magnification);
        assert!(s.hiding);
    }

    #[test]
    fn junk_falls_back_to_the_default_for_that_key_only() {
        let s = parse("magnification = maybe\nhiding = on\nwallpaper = blue\n");
        assert_eq!(s.magnification, Settings::default().magnification);
        assert!(s.hiding);
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        for magnification in [true, false] {
            for hiding in [true, false] {
                let settings = Settings {
                    magnification,
                    hiding,
                };
                let text = format!(
                    "magnification = {}\nhiding = {}\n",
                    on_off(magnification),
                    on_off(hiding)
                );
                assert_eq!(parse(&text), settings);
            }
        }
    }
}
