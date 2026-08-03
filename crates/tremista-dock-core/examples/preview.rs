//! Render dock frames to PNG so the look can be iterated on without a
//! compositor. Synthesises icons rather than reading the icon theme, so it
//! works on any platform.
//!
//! Usage: cargo run -p tremista-dock-core --example preview [out_dir]

use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};
use tremista_dock_core::{
    compute_layout, draw, layout, model::DockItem, render::squircle, RenderItem, Theme,
};

const WIDTH: u32 = 1000;
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
    // App icons are squircles too, at roughly 22% of their edge length.
    let path = squircle(inset, inset, size, size, size * 0.22).unwrap();
    pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    pixmap
}

/// Stand-in for a wallpaper, so translucency is actually visible.
fn wallpaper(width: u32, height: u32) -> Pixmap {
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

    let theme = Theme::default();
    let height = theme.surface_height().ceil() as u32;
    let count = 8;

    let icon_resolution = (theme.icon_size * theme.max_scale).ceil() as u32;
    let icons: Vec<Pixmap> = (0..count).map(|i| synthetic_icon(i, icon_resolution)).collect();
    let items: Vec<DockItem> = (0..count)
        .map(|i| DockItem {
            app_id: format!("app{i}"),
            name: format!("App {i}"),
            exec: "true".into(),
            icon_name: "none".into(),
            // Mark a few as running so the indicator dots show up.
            running: i % 3 == 0,
            pinned: true,
        })
        .collect();

    let resting = compute_layout(count, WIDTH as f32, None, &theme);
    let frames: Vec<(&str, Option<f32>, f32)> = vec![
        ("00-resting", None, 0.0),
        ("01-hover-first", Some(resting.items[0].center_x()), 0.0),
        ("02-hover-middle", Some(resting.items[3].center_x()), 0.0),
        ("03-hover-last", Some(resting.items[count - 1].center_x()), 0.0),
        // Cursor between two icons: the wave should straddle them evenly.
        (
            "04-hover-between",
            Some((resting.items[3].center_x() + resting.items[4].center_x()) / 2.0),
            0.0,
        ),
        ("05-bounce", Some(resting.items[2].center_x()), 0.18),
    ];

    for (name, cursor, bounce_at) in frames {
        let mut canvas = wallpaper(WIDTH, height);
        let l = compute_layout(count, WIDTH as f32, cursor, &theme);

        let renders: Vec<RenderItem> = items
            .iter()
            .enumerate()
            .map(|(i, item)| RenderItem {
                item,
                icon: Some(&icons[i]),
                bounce: if i == 2 {
                    layout::bounce_offset(bounce_at, &theme)
                } else {
                    0.0
                },
            })
            .collect();

        // Render the dock into its own transparent layer, then composite it
        // over the wallpaper -- the same order the compositor uses.
        let mut dock = Pixmap::new(WIDTH, height).unwrap();
        draw(&mut dock.as_mut(), &l, &renders, &theme, 1.0);
        canvas.draw_pixmap(
            0,
            0,
            dock.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );

        let path = format!("{out_dir}/{name}.png");
        canvas.save_png(&path)?;
        println!("wrote {path}");
    }

    Ok(())
}
