use freedesktop_desktop_entry::{get_languages_from_env, DesktopEntry};

/// One slot in the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockItem {
    /// Matched against a toplevel's `app_id` to decide the running state.
    pub app_id: String,
    pub name: String,
    /// `Exec=` with field codes already stripped.
    pub exec: String,
    pub icon_name: String,
    pub running: bool,
    /// Pinned items stay in the dock when they have no open window.
    pub pinned: bool,
}

/// App id of the built-in "All Apps" entry.
///
/// Deliberately not a reverse-DNS name: it must never collide with a real
/// window's `app_id`, or opening that app would light up a running indicator on
/// a dock entry that is not an app at all.
pub const LAUNCHPAD_APP_ID: &str = "tremista-launchpad";

/// The "All Apps" entry that sits at the right end of the dock. Its icon is
/// compiled into the binary, so it appears with no icon theme installed.
///
/// It has no `exec`: clicking it opens the dock's own Launchpad overlay rather
/// than starting anything, so it is not an app and never gets a window.
pub fn launchpad() -> DockItem {
    DockItem {
        app_id: LAUNCHPAD_APP_ID.to_owned(),
        name: "All Apps".to_owned(),
        exec: String::new(),
        icon_name: LAUNCHPAD_APP_ID.to_owned(),
        running: false,
        pinned: true,
    }
}

/// Every launchable installed app, sorted by name -- what Launchpad shows.
///
/// Entries needing a terminal emulator are left out: the dock spawns them with
/// no terminal attached, so showing them would offer icons that do nothing.
pub fn all_apps() -> Vec<DockItem> {
    let locales = get_languages_from_env();
    let entries = freedesktop_desktop_entry::desktop_entries(&locales);

    let mut apps: Vec<DockItem> = entries
        .iter()
        .filter(|entry| !entry.terminal())
        .filter_map(|entry| from_entry(entry, &locales, false))
        .collect();

    // The same app can be installed from more than one place -- a distro
    // package and a Flatpak both shipping org.gnome.Calculator -- and two
    // identical icons side by side looks like a bug.
    apps.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    apps.dedup_by(|a, b| a.app_id == b.app_id && a.name == b.name);
    apps
}

/// Case-insensitive name, with the app id breaking ties so the order is stable
/// between runs rather than depending on directory traversal.
fn sort_key(item: &DockItem) -> (String, &str) {
    (item.name.to_lowercase(), item.app_id.as_str())
}

/// Strip `.desktop` `Exec=` field codes (`%f`, `%U`, ...).
///
/// These expand to files or URLs passed to the app; launching from a dock icon
/// passes nothing, and leaving them in means the app receives a literal "%U"
/// as its first argument.
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // "%%" is a literal percent sign.
            Some('%') => out.push('%'),
            // Every other code expands to nothing here.
            Some(_) => {}
            None => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build dock items for a list of pinned app IDs, resolving each against the
/// installed `.desktop` entries. IDs that resolve to nothing are dropped.
///
/// An entry may list alternatives separated by `|`, and the first one installed
/// wins. That is how a default dock can pin "the GNOME terminal" without
/// knowing whether this release ships Terminal, Console or Ptyxis.
pub fn resolve_pinned(app_ids: &[String]) -> Vec<DockItem> {
    let locales = get_languages_from_env();
    let entries = freedesktop_desktop_entry::desktop_entries(&locales);

    app_ids
        .iter()
        .filter_map(|spec| {
            spec.split('|')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .find_map(|id| {
                    let entry = freedesktop_desktop_entry::find_app_by_id(
                        &entries,
                        freedesktop_desktop_entry::unicase::Ascii::new(id),
                    )?;
                    from_entry(entry, &locales, true)
                })
        })
        .collect()
}

/// Convert a `.desktop` entry into a dock item, skipping entries that are not
/// meant to be shown or launched.
pub fn from_entry(entry: &DesktopEntry, locales: &[String], pinned: bool) -> Option<DockItem> {
    if entry.no_display() || entry.hidden() {
        return None;
    }
    let exec = strip_field_codes(entry.exec()?);
    if exec.is_empty() {
        return None;
    }

    Some(DockItem {
        // Prefer StartupWMClass: it is what the compositor reports as app_id,
        // and it often differs from the desktop file's own id.
        app_id: entry
            .startup_wm_class()
            .map(str::to_owned)
            .unwrap_or_else(|| entry.id().to_owned()),
        name: entry
            .name(locales)
            .map(|c| c.into_owned())
            .unwrap_or_else(|| entry.id().to_owned()),
        exec,
        icon_name: entry.icon().unwrap_or("application-x-executable").to_owned(),
        running: false,
        pinned,
    })
}

/// Case-insensitive match between a compositor `app_id` and a dock item.
///
/// Compositors report app_ids inconsistently -- `firefox`, `Firefox`, and
/// `org.mozilla.firefox` all occur for the same app -- so we also compare the
/// last dotted component.
pub fn matches_app_id(item: &DockItem, app_id: &str) -> bool {
    let trim = |s: &str| {
        s.trim_end_matches(".desktop")
            .rsplit('.')
            .next()
            .unwrap_or(s)
            .to_ascii_lowercase()
    };
    item.app_id.eq_ignore_ascii_case(app_id) || trim(&item.app_id) == trim(app_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(app_id: &str) -> DockItem {
        DockItem {
            app_id: app_id.into(),
            name: "n".into(),
            exec: "e".into(),
            icon_name: "i".into(),
            running: false,
            pinned: true,
        }
    }

    #[test]
    fn field_codes_are_stripped() {
        assert_eq!(strip_field_codes("firefox %u"), "firefox");
        assert_eq!(strip_field_codes("gimp-2.10 %U"), "gimp-2.10");
        assert_eq!(strip_field_codes("app %f --flag %i"), "app --flag");
    }

    #[test]
    fn literal_percent_survives() {
        assert_eq!(strip_field_codes("wine start %% foo"), "wine start % foo");
    }

    #[test]
    fn trailing_percent_does_not_panic() {
        assert_eq!(strip_field_codes("weird %"), "weird");
    }

    #[test]
    fn app_id_matching_tolerates_case_and_reverse_dns() {
        assert!(matches_app_id(&item("firefox"), "Firefox"));
        assert!(matches_app_id(&item("org.mozilla.firefox"), "firefox"));
        assert!(matches_app_id(&item("firefox"), "org.mozilla.Firefox"));
        assert!(matches_app_id(&item("code"), "code.desktop"));
        assert!(!matches_app_id(&item("firefox"), "chromium"));
    }
}
