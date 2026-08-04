//! Just enough text rendering for UI labels.
//!
//! Deliberately not a text stack: glyph outlines straight from the font's
//! `glyf`/`CFF` table, filled with tiny-skia, advanced by each glyph's own
//! advance width. No shaping, no bidi, no ligatures, no kerning. That is wrong
//! for Arabic or Devanagari and invisible for Latin app names, which is all
//! this draws -- and it costs one small pure-Rust dependency instead of a
//! shaper, a font database and a system font scan.
//!
//! Font discovery is a filename match over the standard font directories.
//! Fontconfig would be more correct, but it means a C dependency, and the whole
//! question here is "which regular UI sans is installed", which filenames
//! answer.

use std::path::{Path, PathBuf};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Transform};

/// Font file stems preferred for labels, best first. Inter is what
/// `scripts/install.sh` installs; the rest are the defaults of the distros and
/// desktops we might land on, with macOS at the end so previews render off
/// Linux.
const PREFERRED: &[&str] = &[
    "intervariable",
    "inter-regular",
    "inter",
    "cantarell-regular",
    "notosans-regular",
    "dejavusans",
    "liberationsans-regular",
    "ubuntu-r",
    "roboto-regular",
    "sf-pro-text-regular",
    "helveticaneue",
    "helvetica",
    "arial",
];

/// Fonts that must never be picked as the last-resort fallback: they either
/// have no Latin coverage or would render labels as pictures.
const UNSUITABLE: &[&str] = &[
    "emoji",
    "braille",
    "symbol",
    "dingbat",
    "webding",
    "wingding",
    "lastresort",
    "keyboard",
    "opensymbol",
    "notocolor",
];

const EXTENSIONS: &[&str] = &["ttf", "otf", "ttc"];

/// How deep to walk each font root. `/usr/share/fonts/truetype/dejavu/x.ttf` is
/// three levels, so four leaves room without turning a stray symlink into a
/// filesystem crawl.
const MAX_DEPTH: usize = 4;

/// A single font face, kept as its raw file bytes.
///
/// `ttf_parser::Face` borrows the bytes it parses, so storing one here would
/// make this self-referential. Re-parsing per draw call instead is a table
/// directory walk over data already in the page cache -- microseconds, against
/// a frame we only draw when something changed.
pub struct Font {
    data: Vec<u8>,
    /// Index within a `.ttc` collection; 0 for a plain font file.
    index: u32,
    source: Option<PathBuf>,
}

impl Font {
    /// Find a usable UI font, or `None` if the system has no fonts at all.
    ///
    /// `$TREMISTA_FONT` overrides the search with an explicit path.
    pub fn load() -> Option<Self> {
        if let Some(path) = std::env::var_os("TREMISTA_FONT") {
            let path = PathBuf::from(path);
            if !path.as_os_str().is_empty() {
                match Self::from_path(&path) {
                    Some(font) => return Some(font),
                    None => log::warn!("$TREMISTA_FONT ({}) is not a usable font", path.display()),
                }
            }
        }

        let path = discover()?;
        let font = Self::from_path(&path);
        match &font {
            Some(_) => log::info!("labels drawn with {}", path.display()),
            None => log::warn!("{} could not be parsed; labels disabled", path.display()),
        }
        font
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        let mut font = Self::from_bytes(data)?;
        font.source = Some(path.to_owned());
        Some(font)
    }

    /// Parse font bytes, verifying the face has outlines we can draw.
    pub fn from_bytes(data: Vec<u8>) -> Option<Self> {
        let font = Self {
            data,
            index: 0,
            source: None,
        };
        // A face that cannot map 'A' is either broken or has no Latin coverage,
        // and would draw every label as a row of nothing.
        let usable = font
            .face()
            .is_some_and(|face| face.glyph_index('A').is_some() && face.units_per_em() > 0);
        usable.then_some(font)
    }

    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    fn face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, self.index).ok()
    }

    /// Width of `text` at `size`, in the same units as `size`.
    pub fn measure(&self, text: &str, size: f32) -> f32 {
        self.face()
            .map(|face| measure_with(&face, text, size))
            .unwrap_or(0.0)
    }

    /// Shorten `text` with a trailing ellipsis until it fits `max_width`.
    ///
    /// Returns the text unchanged when it already fits, so the common case
    /// allocates nothing beyond the copy.
    pub fn ellipsize(&self, text: &str, size: f32, max_width: f32) -> String {
        let Some(face) = self.face() else {
            return text.to_owned();
        };
        if measure_with(&face, text, size) <= max_width {
            return text.to_owned();
        }

        let ellipsis = '…';
        // Fonts without U+2026 exist; three periods read the same.
        let suffix = if face.glyph_index(ellipsis).is_some() {
            ellipsis.to_string()
        } else {
            "...".to_owned()
        };

        let mut kept = String::new();
        for c in text.chars() {
            let mut candidate = kept.clone();
            candidate.push(c);
            if measure_with(&face, &(candidate.clone() + &suffix), size) > max_width {
                break;
            }
            kept = candidate;
        }
        // Trailing spaces before an ellipsis look like a typo.
        kept.truncate(kept.trim_end().len());
        kept + &suffix
    }

    /// Draw `text` with its left edge at `x` and its baseline at `baseline`.
    ///
    /// Coordinates are logical; `scale` is the output scale factor, applied at
    /// fill time so the glyph outlines are rasterised at device resolution
    /// rather than scaled up from a logical-size bitmap.
    pub fn draw(
        &self,
        target: &mut PixmapMut,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: Color,
        scale: f32,
    ) {
        let Some(face) = self.face() else { return };
        let upem = face.units_per_em();
        if upem == 0 || size <= 0.0 {
            return;
        }
        // Font units are y-up from the baseline; the pixmap is y-down.
        let k = size / upem as f32;

        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(color);
        let transform = Transform::from_scale(scale, scale);

        let mut pen = x;
        for c in text.chars() {
            let Some(glyph) = face.glyph_index(c) else {
                // Missing glyph: advance by a space rather than piling the rest
                // of the label on top of itself.
                pen += face
                    .glyph_index(' ')
                    .and_then(|g| face.glyph_hor_advance(g))
                    .unwrap_or(upem / 4) as f32
                    * k;
                continue;
            };

            let mut outline = Outline {
                builder: PathBuilder::new(),
                x: pen,
                y: baseline,
                k,
            };
            if face.outline_glyph(glyph, &mut outline).is_some() {
                if let Some(path) = outline.builder.finish() {
                    target.fill_path(&path, &paint, FillRule::Winding, transform, None);
                }
            }
            pen += face.glyph_hor_advance(glyph).unwrap_or(0) as f32 * k;
        }
    }

    /// Draw `text` centred horizontally on `center_x`.
    pub fn draw_centered(
        &self,
        target: &mut PixmapMut,
        text: &str,
        center_x: f32,
        baseline: f32,
        size: f32,
        color: Color,
        scale: f32,
    ) {
        let x = center_x - self.measure(text, size) / 2.0;
        self.draw(target, text, x, baseline, size, color, scale);
    }
}

fn measure_with(face: &ttf_parser::Face, text: &str, size: f32) -> f32 {
    let upem = face.units_per_em();
    if upem == 0 {
        return 0.0;
    }
    let k = size / upem as f32;
    text.chars()
        .map(|c| {
            face.glyph_index(c)
                .and_then(|g| face.glyph_hor_advance(g))
                .unwrap_or(upem / 4) as f32
                * k
        })
        .sum()
}

/// Turns font-unit outlines into a tiny-skia path in logical coordinates.
struct Outline {
    builder: PathBuilder,
    x: f32,
    y: f32,
    k: f32,
}

impl Outline {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.k, self.y - y * self.k)
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.map(x1, y1);
        let (x, y) = self.map(x, y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.map(x1, y1);
        let (x2, y2) = self.map(x2, y2);
        let (x, y) = self.map(x, y);
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        roots.push(home.join(".local/share/fonts"));
        roots.push(home.join(".fonts"));
    }
    roots.extend(
        [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/run/host/fonts", // Flatpak's view of the host's fonts.
            "/System/Library/Fonts",
            "/Library/Fonts",
        ]
        .iter()
        .map(PathBuf::from),
    );
    roots
}

/// Pick a font file: the best-ranked [`PREFERRED`] stem present, else any
/// plausible regular face.
fn discover() -> Option<PathBuf> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for root in roots() {
        collect(&root, 0, &mut found);
    }
    found.sort();

    for want in PREFERRED {
        if let Some((_, path)) = found.iter().find(|(stem, _)| stem == want) {
            return Some(path.clone());
        }
    }
    // Loose match second, so "Inter-Regular" is preferred over "InterTight"
    // but either beats falling through to an arbitrary font.
    for want in PREFERRED {
        if let Some((_, path)) = found.iter().find(|(stem, _)| stem.contains(want)) {
            return Some(path.clone());
        }
    }

    found
        .into_iter()
        .find(|(stem, _)| {
            !UNSUITABLE.iter().any(|bad| stem.contains(bad))
                // A bold or italic face as the *fallback* would set every label
                // in the wrong weight; only take one if it is all there is.
                && !["bold", "italic", "oblique", "light", "thin", "black"]
                    .iter()
                    .any(|w| stem.contains(w))
        })
        .map(|(_, path)| path)
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, depth + 1, out);
            continue;
        }
        let matches_extension = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|want| e.eq_ignore_ascii_case(want)));
        if !matches_extension {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            out.push((stem.to_ascii_lowercase(), path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every machine we test on has fonts, but a container might not, and a
    /// missing font is a supported state rather than a test failure.
    fn font() -> Option<Font> {
        Font::load()
    }

    #[test]
    fn a_font_is_found_and_measures_text() {
        let Some(font) = font() else { return };
        let width = font.measure("Launchpad", 13.0);
        assert!(width > 13.0, "implausibly narrow: {width}");
        assert!(width < 13.0 * 9.0, "implausibly wide: {width}");
        // Longer strings are wider; this is what the grid relies on to fit
        // labels into a cell.
        assert!(font.measure("Launchpad Launchpad", 13.0) > width);
    }

    #[test]
    fn measuring_the_empty_string_is_zero() {
        let Some(font) = font() else { return };
        assert_eq!(font.measure("", 13.0), 0.0);
    }

    #[test]
    fn short_labels_are_left_alone() {
        let Some(font) = font() else { return };
        assert_eq!(font.ellipsize("Files", 13.0, 500.0), "Files");
    }

    #[test]
    fn long_labels_are_ellipsized_to_fit() {
        let Some(font) = font() else { return };
        let long = "Advanced Network Configuration Editor";
        let fitted = font.ellipsize(long, 13.0, 60.0);
        assert!(fitted.len() < long.len(), "not shortened: {fitted}");
        assert!(fitted.ends_with('…') || fitted.ends_with("..."));
        assert!(font.measure(&fitted, 13.0) <= 60.0);
    }

    #[test]
    fn an_impossibly_narrow_cell_still_yields_an_ellipsis() {
        let Some(font) = font() else { return };
        // Not even one character fits: the result must still be drawable
        // rather than a panic or an empty label.
        let fitted = font.ellipsize("Settings", 13.0, 1.0);
        assert!(fitted.ends_with('…') || fitted.ends_with("..."));
    }

    #[test]
    fn drawing_puts_ink_on_the_pixmap() {
        let Some(font) = font() else { return };
        let mut pixmap = tiny_skia::Pixmap::new(200, 40).unwrap();
        font.draw(
            &mut pixmap.as_mut(),
            "Launchpad",
            10.0,
            28.0,
            16.0,
            Color::WHITE,
            1.0,
        );
        let inked = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(inked > 40, "only {inked} pixels covered");
    }

    #[test]
    fn garbage_is_not_accepted_as_a_font() {
        assert!(Font::from_bytes(vec![0u8; 512]).is_none());
        assert!(Font::from_bytes(Vec::new()).is_none());
    }
}
