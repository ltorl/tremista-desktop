use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

/// Rasterises icons once, at the largest size the dock can display, and keeps
/// them around. Scaling down a big pixmap looks fine; scaling a small one up
/// to a magnified icon does not, so we always rasterise at the peak.
pub struct IconCache {
    /// Edge length icons are rasterised at.
    resolution: u32,
    theme: Option<String>,
    /// Searched before the icon theme, so a user can replace one app's icon
    /// without installing or forking a whole theme.
    overrides: Vec<PathBuf>,
    by_name: HashMap<String, Option<Pixmap>>,
}

/// Extensions an override is looked up under, best first: SVG scales to any
/// magnification, so it wins over a bitmap of the same name.
const OVERRIDE_EXTENSIONS: &[&str] = &["svg", "png", "webp", "jpg", "jpeg"];

/// Icons compiled into the binary, for dock entries that are ours rather than
/// an installed app's and so have nothing in any icon theme. Consulted last, so
/// an override directory or a theme icon of the same name still wins.
const BUILTIN_ICONS: &[(&str, &[u8])] = &[(
    "tremista-launchpad",
    include_bytes!("../assets/launchpad.svg"),
)];

impl IconCache {
    pub fn new(resolution: u32, theme: Option<String>) -> Self {
        Self {
            resolution,
            theme,
            overrides: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    /// Add directories searched ahead of the icon theme. A file named
    /// `<icon name>.<ext>` in one of them replaces that icon.
    pub fn with_overrides(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.overrides.extend(dirs);
        self
    }

    /// Look up an icon by its freedesktop name (or absolute path, which
    /// `.desktop` files are allowed to use) and rasterise it.
    ///
    /// Failures are cached as `None` so a missing icon costs one lookup rather
    /// than one per frame.
    pub fn get(&mut self, name: &str) -> Option<&Pixmap> {
        if !self.by_name.contains_key(name) {
            let loaded = self.load(name).unwrap_or_else(|e| {
                log::debug!("icon {name:?} unavailable: {e:#}");
                None
            });
            self.by_name.insert(name.to_owned(), loaded);
        }
        self.by_name.get(name).and_then(|slot| slot.as_ref())
    }

    /// Look up an already-cached icon without resolving anything.
    ///
    /// The renderer needs to borrow several icons at once, which `get` cannot
    /// give it because inserting requires `&mut self`. Callers warm the cache
    /// with `get` first, then read it back through `peek`.
    pub fn peek(&self, name: &str) -> Option<&Pixmap> {
        self.by_name.get(name).and_then(|slot| slot.as_ref())
    }

    /// Drop every rasterised icon. Used when the output scale changes and the
    /// cached resolution is no longer the right one.
    pub fn set_resolution(&mut self, resolution: u32) {
        if resolution != self.resolution {
            self.resolution = resolution;
            self.by_name.clear();
        }
    }

    fn load(&self, name: &str) -> Result<Option<Pixmap>> {
        let path = match self.resolve(name) {
            Some(p) => p,
            None => {
                return match BUILTIN_ICONS.iter().find(|(n, _)| *n == name) {
                    Some((_, data)) => rasterize_svg(data, self.resolution).map(Some),
                    None => Ok(None),
                }
            }
        };
        rasterize(&path, self.resolution).map(Some)
    }

    fn resolve(&self, name: &str) -> Option<PathBuf> {
        // Absolute paths are legal in Icon= and must bypass theme lookup.
        let as_path = Path::new(name);
        if as_path.is_absolute() && as_path.exists() {
            return Some(as_path.to_owned());
        }

        // A name with a separator in it would escape the override directory,
        // so only bare icon names are eligible.
        if !name.contains('/') && !name.contains('\\') && name != ".." {
            for dir in &self.overrides {
                for ext in OVERRIDE_EXTENSIONS {
                    let candidate = dir.join(format!("{name}.{ext}"));
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }

        let mut lookup = freedesktop_icons::lookup(name)
            .with_size(self.resolution.min(u16::MAX as u32) as u16)
            .with_cache();
        if let Some(theme) = &self.theme {
            lookup = lookup.with_theme(theme);
        }
        lookup.find()
    }
}

/// Rasterise an icon file to a square `resolution`-pixel premultiplied pixmap.
///
/// SVG is rendered directly at the target size rather than rasterised small
/// and scaled, which is the whole reason magnified icons stay crisp.
pub fn rasterize(path: &Path, resolution: u32) -> Result<Pixmap> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

    let is_svg = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"));

    if is_svg {
        rasterize_svg(&data, resolution)
    } else {
        rasterize_bitmap(&data, resolution)
    }
}

fn rasterize_svg(data: &[u8], resolution: u32) -> Result<Pixmap> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(data, &options).context("parsing SVG")?;

    let mut pixmap =
        Pixmap::new(resolution, resolution).ok_or_else(|| anyhow!("bad icon resolution"))?;

    // Fit the SVG's natural size into the square, preserving aspect ratio.
    let size = tree.size();
    let scale = (resolution as f32 / size.width()).min(resolution as f32 / size.height());
    let tx = (resolution as f32 - size.width() * scale) / 2.0;
    let ty = (resolution as f32 - size.height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_translate(tx, ty).pre_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

fn rasterize_bitmap(data: &[u8], resolution: u32) -> Result<Pixmap> {
    let decoded = image::load_from_memory(data).context("decoding bitmap icon")?;
    let rgba = decoded
        .resize(
            resolution,
            resolution,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();

    let mut pixmap =
        Pixmap::new(resolution, resolution).ok_or_else(|| anyhow!("bad icon resolution"))?;

    // `resize` preserves aspect ratio, so the result is usually not square.
    // Centre it in the square pixmap.
    let ox = (resolution - rgba.width()) / 2;
    let oy = (resolution - rgba.height()) / 2;

    let dst = pixmap.pixels_mut();
    for (x, y, px) in rgba.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        // tiny-skia stores premultiplied alpha; image gives us straight alpha.
        let premultiplied = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
        let idx = ((y + oy) * resolution + (x + ox)) as usize;
        dst[idx] = premultiplied;
    }

    Ok(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_rasterises_to_the_requested_square() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <rect width="16" height="16" fill="#ff0000"/></svg>"##;
        let pixmap = rasterize_svg(svg, 64).unwrap();
        assert_eq!(pixmap.width(), 64);
        assert_eq!(pixmap.height(), 64);
        // Centre pixel should be opaque red, proving it scaled up rather than
        // rendering 16px into a corner.
        let centre = pixmap.pixel(32, 32).unwrap();
        assert_eq!(centre.alpha(), 255);
        assert!(centre.red() > 200 && centre.green() < 50);
    }

    #[test]
    fn non_square_svg_is_letterboxed_not_stretched() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16">
            <rect width="32" height="16" fill="#0000ff"/></svg>"##;
        let pixmap = rasterize_svg(svg, 64).unwrap();
        // Full width at mid height, transparent at top and bottom.
        assert_eq!(pixmap.pixel(32, 32).unwrap().alpha(), 255);
        assert_eq!(pixmap.pixel(32, 1).unwrap().alpha(), 0);
        assert_eq!(pixmap.pixel(32, 62).unwrap().alpha(), 0);
    }

    #[test]
    fn bitmap_icons_are_centred_and_premultiplied() {
        let mut img = image::RgbaImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = image::Rgba([0, 255, 0, 255]);
        }
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();

        let pixmap = rasterize_bitmap(encoded.get_ref(), 32).unwrap();
        assert_eq!(pixmap.width(), 32);
        let centre = pixmap.pixel(16, 16).unwrap();
        assert_eq!(centre.alpha(), 255);
        assert!(centre.green() > 200);
    }

    #[test]
    fn peek_only_sees_what_get_has_warmed() {
        let mut cache = IconCache::new(64, None);
        let name = "tremista-definitely-not-a-real-icon-name";
        assert!(cache.peek(name).is_none());
        cache.get(name);
        // Still absent, but now it is a cached absence rather than a miss.
        assert!(cache.peek(name).is_none());
        assert!(cache.by_name.contains_key(name));
    }

    #[test]
    fn changing_resolution_invalidates_the_cache() {
        let mut cache = IconCache::new(64, None);
        cache.get("tremista-definitely-not-a-real-icon-name");
        assert!(!cache.by_name.is_empty());
        cache.set_resolution(64); // no change, must not clear
        assert!(!cache.by_name.is_empty());
        cache.set_resolution(128);
        assert!(cache.by_name.is_empty());
    }

    #[test]
    fn an_override_directory_beats_the_icon_theme() {
        let dir = std::env::temp_dir().join(format!("tremista-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <rect width="16" height="16" fill="#ff0000"/></svg>"##;
        std::fs::write(dir.join("firefox.svg"), svg).unwrap();

        let mut cache = IconCache::new(32, None).with_overrides([dir.clone()]);
        // firefox may or may not be installed here; either way the override
        // must be what comes back, so check the pixels rather than mere Some.
        let pixmap = cache.get("firefox").expect("override should resolve");
        let centre = pixmap.pixel(16, 16).unwrap();
        assert!(centre.red() > 200 && centre.green() < 50);

        // A name that could climb out of the directory must not be joined onto it.
        assert!(cache.get("../firefox").is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_launchpad_icon_is_available_without_a_theme() {
        let mut cache = IconCache::new(64, None);
        let pixmap = cache
            .get(crate::model::LAUNCHPAD_APP_ID)
            .expect("built-in icon should always resolve");
        assert_eq!(pixmap.width(), 64);
        // Opaque in the middle: an SVG that failed to parse would render empty.
        assert_eq!(pixmap.pixel(32, 32).unwrap().alpha(), 255);
    }

    #[test]
    fn an_override_beats_a_builtin() {
        let dir = std::env::temp_dir().join(format!("tremista-builtin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <rect width="16" height="16" fill="#ff0000"/></svg>"##;
        std::fs::write(dir.join(format!("{}.svg", crate::model::LAUNCHPAD_APP_ID)), svg).unwrap();

        let mut cache = IconCache::new(32, None).with_overrides([dir.clone()]);
        let centre = cache
            .get(crate::model::LAUNCHPAD_APP_ID)
            .unwrap()
            .pixel(16, 16)
            .unwrap();
        assert!(centre.red() > 200 && centre.green() < 50);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_icons_are_cached_as_absent() {
        let mut cache = IconCache::new(64, None);
        let name = "tremista-definitely-not-a-real-icon-name";
        assert!(cache.get(name).is_none());
        // Second call must be served from the cache, not re-resolved.
        assert!(cache.by_name.contains_key(name));
        assert!(cache.get(name).is_none());
    }
}
