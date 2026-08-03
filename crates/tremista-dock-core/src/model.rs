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
pub fn resolve_pinned(app_ids: &[String]) -> Vec<DockItem> {
    let locales = get_languages_from_env();
    let entries = freedesktop_desktop_entry::desktop_entries(&locales);

    app_ids
        .iter()
        .filter_map(|id| {
            let entry = freedesktop_desktop_entry::find_app_by_id(
                &entries,
                freedesktop_desktop_entry::unicase::Ascii::new(id.as_str()),
            )?;
            from_entry(entry, &locales, true)
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
