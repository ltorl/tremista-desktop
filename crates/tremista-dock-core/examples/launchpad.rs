//! Render Launchpad frames to PNG, so the grid can be iterated on without a
//! compositor. Synthesises icons and app names rather than reading the system,
//! so it works on any platform.
//!
//! Usage: cargo run -p tremista-dock-core --example launchpad [out_dir]

use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};
use tremista_dock_core::{
    launchpad::{self, LaunchpadTheme},
    render::squircle,
    text::Font,
    GridItem, Theme,
};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

const NAMES: &[&str] = &[
    "Files",
    "Terminal",
    "Chromium",
    "Settings",
    "Text Editor",
    "Calculator",
    "Calendar",
    "Music",
    "Photos",
    "Maps",
    "Weather",
    "Clocks",
    "Disk Usage Analyzer",
    "Fonts",
    "Software",
    "Videos",
    "Contacts",
    "Document Viewer",
    "Image Viewer",
    "Archive Manager",
    "System Monitor",
    "Passwords and Keys",
    "Characters",
    "Logs",
    "Extensions",
    "Connections",
    "Boxes",
    "Builder",
    "Cheese",
    "Evolution",
    "GIMP",
    "Inkscape",
    "LibreOffice Writer",
    "Remote Desktop Viewer",
    "Transmission",
    "VLC",
    "Blender",
    "Steam",
    "Krita",
    "Thunderbird",
];

const PALETTE: [(u8, u8, u8); 8] = [
    (255, 92, 87),
    (255, 181, 71),
    (94, 200, 120),
    (66, 165, 245),
    (171, 122, 235),
    (255, 128, 171),
    (77, 208, 225),
    (149, 165, 180),
];

fn synthetic_icon(index: usize, resolution: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(resolution, resolution).unwrap();
    let (r, g, b) = PALETTE[index % PALETTE.len()];
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    let inset = resolution as f32 * 0.06;
    let size = resolution as f32 - inset * 2.0;
    let path = squircle(inset, inset, size, size, size * 0.22).unwrap();
    pixmap.fill_path(
        &path,
        &paint,
        tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    pixmap
}

/// Stand-in for the desktop behind the overlay, so the backdrop's translucency
/// is actually visible.
fn desktop(width: u32, height: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).unwrap();
    for y in 0..height {
        let t = y as f32 / height as f32;
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(
            (40.0 + 90.0 * t) as u8,
            (70.0 + 60.0 * t) as u8,
            (130.0 + 70.0 * t) as u8,
            255,
        ));
        pixmap.fill_rect(
            Rect::from_xywh(0.0, y as f32, width as f32, 1.0).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
    }
    pixmap
}

fn main() -> anyhow::Result<()> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| "preview".into());
    std::fs::create_dir_all(&out_dir)?;

    let dock = Theme::default();
    let theme = LaunchpadTheme {
        // What the shell passes: the strip the dock occupies, left clear.
        reserved_bottom: dock.background_height() + dock.margin_bottom,
        ..LaunchpadTheme::default()
    };

    let font = Font::load();
    if font.is_none() {
        eprintln!("note: no font found, so the preview has no labels");
    }

    let icons: Vec<Pixmap> = (0..NAMES.len())
        .map(|i| synthetic_icon(i, 128))
        .collect();
    let items: Vec<GridItem> = NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| GridItem {
            name,
            icon: Some(&icons[i]),
        })
        .collect();

    let frames: [(&str, usize, Option<usize>, f32); 4] = [
        ("00-page-one", 0, None, 1.0),
        ("01-hover", 0, Some(9), 1.0),
        ("02-page-two", 1, None, 1.0),
        // Mid-animation, where the fade and the icon growth are visible.
        ("03-opening", 0, None, 0.25),
    ];

    for (name, page, hovered, progress) in frames {
        let mut canvas = desktop(WIDTH, HEIGHT);
        let grid = launchpad::compute(items.len(), page, WIDTH as f32, HEIGHT as f32, &theme);

        let mut overlay = Pixmap::new(WIDTH, HEIGHT).unwrap();
        launchpad::draw(
            &mut overlay.as_mut(),
            &grid,
            &items,
            &theme,
            font.as_ref(),
            hovered,
            progress,
            1.0,
        );
        canvas.draw_pixmap(
            0,
            0,
            overlay.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );

        let path = format!("{out_dir}/launchpad-{name}.png");
        canvas.save_png(&path)?;
        println!("wrote {path}");
    }

    Ok(())
}
