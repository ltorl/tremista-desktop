//! The Launchpad grid: every installed app as a page of large icons.
//!
//! Geometry and drawing only, like the rest of this crate -- the full-screen
//! surface it lands on is the shell's problem. Coordinates are logical pixels
//! relative to that surface, which spans the whole output.

use crate::render::squircle;
use crate::text::Font;
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Paint, Pattern, Pixmap, PixmapMut,
    Point, Rect, SpreadMode, Transform,
};

/// Visual and geometric parameters for the grid, in logical pixels.
#[derive(Debug, Clone)]
pub struct LaunchpadTheme {
    pub icon_size: f32,
    /// Cell footprint. Wider than the icon so labels have room and the hover
    /// highlight reads as a target rather than a box around the artwork.
    pub cell_width: f32,
    pub cell_height: f32,
    /// Upper bounds on the grid. macOS settles at 7x5 and grows the gaps
    /// instead of the icon count, which is what keeps a 4K screen from
    /// becoming a wall of tiny icons.
    pub max_columns: usize,
    pub max_rows: usize,
    /// How far a cell may stretch beyond its base size to fill the screen once
    /// the column or row cap is reached. Without this a 7-column grid at its
    /// base width leaves a wide empty gutter on a 1920px screen; with it the
    /// grid spreads, which is what macOS does.
    pub max_spread: f32,

    pub label_size: f32,
    /// Baseline distance below the bottom of the icon.
    pub label_gap: f32,

    pub margin_x: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    /// Height at the bottom of the surface left untouched, so the dock beneath
    /// stays visible and usable while Launchpad is open, as it does on macOS.
    pub reserved_bottom: f32,
    /// Distance over which the backdrop fades out above `reserved_bottom`. A
    /// hard edge there would read as a rendering bug.
    pub fade: f32,

    pub backdrop: Color,
    pub label: Color,
    /// Drawn one pixel below the label. App icons are bright and the backdrop
    /// is translucent, so white-on-anything needs the separation.
    pub label_shadow: Color,
    pub hover: Color,
    pub hover_radius: f32,

    pub dot_radius: f32,
    pub dot_gap: f32,
    pub dot: Color,
    pub dot_active: Color,

    /// Seconds the open animation runs for.
    pub open_duration: f32,
}

impl Default for LaunchpadTheme {
    fn default() -> Self {
        Self {
            icon_size: 96.0,
            cell_width: 150.0,
            cell_height: 132.0,
            max_columns: 7,
            max_rows: 5,
            max_spread: 1.5,
            label_size: 13.0,
            label_gap: 20.0,
            margin_x: 60.0,
            margin_top: 60.0,
            margin_bottom: 46.0,
            reserved_bottom: 0.0,
            fade: 56.0,
            // Dark enough to push the desktop back, light enough that the
            // compositor's blur behind the surface still shows through.
            backdrop: Color::from_rgba8(18, 18, 22, 190),
            label: Color::from_rgba8(255, 255, 255, 240),
            label_shadow: Color::from_rgba8(0, 0, 0, 130),
            hover: Color::from_rgba8(255, 255, 255, 28),
            hover_radius: 18.0,
            dot_radius: 4.0,
            dot_gap: 16.0,
            dot: Color::from_rgba8(255, 255, 255, 70),
            dot_active: Color::from_rgba8(255, 255, 255, 225),
            open_duration: 0.2,
        }
    }
}

/// One app's place on the current page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// Index into the full app list, not into the page.
    pub index: usize,
    /// Clickable box: the whole cell, so the label and the gap around the icon
    /// are targets too.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// The icon box, centred in the upper part of the cell.
    pub icon_x: f32,
    pub icon_y: f32,
    pub icon_size: f32,
}

impl Cell {
    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// The laid-out page.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    pub per_page: usize,
    /// Page actually laid out, already clamped to `pages`.
    pub page: usize,
    pub pages: usize,
    pub cells: Vec<Cell>,
    /// Centres of the page indicator dots; empty when there is one page.
    pub dots: Vec<(f32, f32)>,
}

/// Lay out `count` apps on `page` within a surface of `width` x `height`.
pub fn compute(
    count: usize,
    page: usize,
    width: f32,
    height: f32,
    theme: &LaunchpadTheme,
) -> Grid {
    let usable_width = (width - theme.margin_x * 2.0).max(theme.cell_width);
    let usable_height = (height - theme.margin_top - theme.margin_bottom - theme.reserved_bottom)
        .max(theme.cell_height);

    let columns = ((usable_width / theme.cell_width).floor() as usize)
        .clamp(1, theme.max_columns.max(1));
    let rows =
        ((usable_height / theme.cell_height).floor() as usize).clamp(1, theme.max_rows.max(1));

    let per_page = columns * rows;
    let pages = count.div_ceil(per_page).max(1);
    let page = page.min(pages - 1);

    // Spread the capped grid over the space available instead of leaving it
    // huddled in the middle, but never past `max_spread`, or a wide screen
    // would strand each icon on its own.
    let spread = |base: f32, span: f32, n: usize| {
        (span / n as f32).clamp(base, base * theme.max_spread.max(1.0))
    };
    let cell_width = spread(theme.cell_width, usable_width, columns);
    let cell_height = spread(theme.cell_height, usable_height, rows);

    // Centre the block rather than left-aligning it: with fewer apps than a
    // full page the grid should still sit in the middle of the screen.
    let block_width = columns as f32 * cell_width;
    let origin_x = (width - block_width) / 2.0;
    let block_height = rows as f32 * cell_height;
    let field_top = theme.margin_top;
    let field_height = height - theme.margin_top - theme.margin_bottom - theme.reserved_bottom;
    let origin_y = field_top + (field_height - block_height).max(0.0) / 2.0;

    let start = page * per_page;
    let end = (start + per_page).min(count);

    let cells = (start..end)
        .enumerate()
        .map(|(slot, index)| {
            let column = slot % columns;
            let row = slot / columns;
            let x = origin_x + column as f32 * cell_width;
            let y = origin_y + row as f32 * cell_height;
            Cell {
                index,
                x,
                y,
                width: cell_width,
                height: cell_height,
                icon_x: x + (cell_width - theme.icon_size) / 2.0,
                // Sits high in the cell; the space below is the label's.
                icon_y: y + cell_height * 0.14,
                icon_size: theme.icon_size,
            }
        })
        .collect();

    let dots = if pages > 1 {
        let spacing = theme.dot_radius * 2.0 + theme.dot_gap;
        let total = (pages - 1) as f32 * spacing;
        let first = width / 2.0 - total / 2.0;
        let y = height - theme.reserved_bottom - theme.margin_bottom / 2.0;
        (0..pages).map(|i| (first + i as f32 * spacing, y)).collect()
    } else {
        Vec::new()
    };

    Grid {
        columns,
        rows,
        per_page,
        page,
        pages,
        cells,
        dots,
    }
}

/// Which app is under the cursor, as an index into the full app list.
pub fn hit_test(grid: &Grid, x: f32, y: f32) -> Option<usize> {
    grid.cells
        .iter()
        .find(|cell| cell.contains(x, y))
        .map(|cell| cell.index)
}

/// What the renderer needs per app.
pub struct GridItem<'a> {
    pub name: &'a str,
    pub icon: Option<&'a Pixmap>,
}

/// Composite one Launchpad frame into `target`.
///
/// `progress` runs 0..1 over the open animation; `hovered` is an index into
/// the full app list, matching [`hit_test`].
pub fn draw(
    target: &mut PixmapMut,
    grid: &Grid,
    items: &[GridItem],
    theme: &LaunchpadTheme,
    font: Option<&Font>,
    hovered: Option<usize>,
    progress: f32,
    scale: f32,
) {
    target.fill(Color::TRANSPARENT);

    let width = target.width() as f32 / scale;
    let height = target.height() as f32 / scale;
    // Ease out: the backdrop should arrive fast and settle, not ramp linearly.
    let t = progress.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    let scale_tf = Transform::from_scale(scale, scale);

    draw_backdrop(target, width, height, theme, eased, scale_tf);

    // Icons grow into place from slightly small. Subtle on purpose: at more
    // than a few percent it reads as a zoom effect rather than as the grid
    // simply appearing.
    let icon_scale = 0.94 + 0.06 * eased;

    for cell in &grid.cells {
        let Some(item) = items.get(cell.index) else {
            continue;
        };

        if hovered == Some(cell.index) {
            // Sized from the icon rather than the cell: cells stretch to fill
            // the screen, and a highlight that stretched with them would be a
            // wide slab around a small icon.
            let plate_width = cell.icon_size + 44.0;
            let plate_height = cell.icon_size + theme.label_gap + theme.label_size + 26.0;
            if let Some(path) = squircle(
                cell.center_x() - plate_width / 2.0,
                cell.icon_y - 14.0,
                plate_width,
                plate_height,
                theme.hover_radius,
            ) {
                let mut paint = Paint {
                    anti_alias: true,
                    ..Default::default()
                };
                paint.set_color(fade(theme.hover, eased));
                target.fill_path(&path, &paint, FillRule::Winding, scale_tf, None);
            }
        }

        let size = cell.icon_size * icon_scale;
        // Grow about the icon's own centre, so the grid does not appear to
        // drift as it settles.
        let x = cell.icon_x + (cell.icon_size - size) / 2.0;
        let y = cell.icon_y + (cell.icon_size - size) / 2.0;

        if let Some(icon) = item.icon {
            draw_icon(target, icon, x, y, size, eased, scale);
        }

        let Some(font) = font else { continue };
        let baseline = cell.icon_y + cell.icon_size + theme.label_gap;
        // Bounded by the icon, not the cell, for the same reason as the
        // highlight: on a wide screen an unbounded label would run most of the
        // way to its neighbour before being cut.
        let label_width = (cell.width - 16.0).min(cell.icon_size + 76.0);
        let label = font.ellipsize(item.name, theme.label_size, label_width);
        font.draw_centered(
            target,
            &label,
            cell.center_x(),
            baseline + 1.0,
            theme.label_size,
            fade(theme.label_shadow, eased),
            scale,
        );
        font.draw_centered(
            target,
            &label,
            cell.center_x(),
            baseline,
            theme.label_size,
            fade(theme.label, eased),
            scale,
        );
    }

    // --- Page dots --------------------------------------------------------
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    for (i, (x, y)) in grid.dots.iter().enumerate() {
        let color = if i == grid.page {
            theme.dot_active
        } else {
            theme.dot
        };
        paint.set_color(fade(color, eased));
        if let Some(dot) = squircle(
            x - theme.dot_radius,
            y - theme.dot_radius,
            theme.dot_radius * 2.0,
            theme.dot_radius * 2.0,
            theme.dot_radius,
        ) {
            target.fill_path(&dot, &paint, FillRule::Winding, scale_tf, None);
        }
    }
}

/// The dimmed sheet behind the grid, ending in a fade so the strip left for the
/// dock is untouched.
fn draw_backdrop(
    target: &mut PixmapMut,
    width: f32,
    height: f32,
    theme: &LaunchpadTheme,
    eased: f32,
    scale_tf: Transform,
) {
    let solid_bottom = (height - theme.reserved_bottom - theme.fade).max(0.0);
    let color = fade(theme.backdrop, eased);

    let mut paint = Paint::default();
    paint.set_color(color);
    if let Some(rect) = Rect::from_xywh(0.0, 0.0, width, solid_bottom) {
        target.fill_rect(rect, &paint, scale_tf, None);
    }

    if theme.fade <= 0.0 {
        return;
    }
    let transparent = Color::from_rgba(color.red(), color.green(), color.blue(), 0.0)
        .unwrap_or(Color::TRANSPARENT);
    let shader = LinearGradient::new(
        Point::from_xy(0.0, solid_bottom),
        Point::from_xy(0.0, solid_bottom + theme.fade),
        vec![
            GradientStop::new(0.0, color),
            GradientStop::new(1.0, transparent),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    );
    let Some(shader) = shader else { return };
    let paint = Paint {
        shader,
        anti_alias: false,
        ..Default::default()
    };
    if let Some(rect) = Rect::from_xywh(0.0, solid_bottom, width, theme.fade) {
        target.fill_rect(rect, &paint, scale_tf, None);
    }
}

/// Blit one icon, scaled to `size` and faded by `alpha`.
fn draw_icon(
    target: &mut PixmapMut,
    icon: &Pixmap,
    x: f32,
    y: f32,
    size: f32,
    alpha: f32,
    scale: f32,
) {
    if icon.width() == 0 {
        return;
    }
    // A Pattern rather than draw_pixmap, for the same reason as the dock: it
    // places on subpixel boundaries, so the open animation is smooth instead of
    // snapping between integer positions.
    let icon_scale = size / icon.width() as f32;
    let pattern_tf = Transform::from_scale(icon_scale, icon_scale)
        .post_translate(x, y)
        .post_scale(scale, scale);

    let paint = Paint {
        shader: Pattern::new(
            icon.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            alpha.clamp(0.0, 1.0),
            pattern_tf,
        ),
        anti_alias: true,
        ..Default::default()
    };
    if let Some(rect) = Rect::from_xywh(x * scale, y * scale, size * scale, size * scale) {
        target.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn fade(color: Color, alpha: f32) -> Color {
    let mut faded = color;
    faded.set_alpha(color.alpha() * alpha.clamp(0.0, 1.0));
    faded
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: f32 = 1920.0;
    const HEIGHT: f32 = 1080.0;

    fn theme() -> LaunchpadTheme {
        LaunchpadTheme::default()
    }

    fn icon() -> Pixmap {
        let mut pixmap = Pixmap::new(96, 96).unwrap();
        pixmap.fill(Color::from_rgba8(255, 0, 0, 255));
        pixmap
    }

    #[test]
    fn a_full_page_is_laid_out_within_the_surface() {
        let grid = compute(40, 0, WIDTH, HEIGHT, &theme());
        assert_eq!((grid.columns, grid.rows), (7, 5));
        assert_eq!(grid.cells.len(), 35);
        for cell in &grid.cells {
            assert!(cell.x >= 0.0 && cell.x + cell.width <= WIDTH);
            assert!(cell.y >= 0.0 && cell.y + cell.height <= HEIGHT);
        }
    }

    #[test]
    fn the_last_page_holds_the_remainder() {
        let grid = compute(40, 1, WIDTH, HEIGHT, &theme());
        assert_eq!(grid.pages, 2);
        assert_eq!(grid.cells.len(), 5);
        assert_eq!(grid.cells[0].index, 35);
        assert_eq!(grid.cells[4].index, 39);
    }

    #[test]
    fn a_page_past_the_end_clamps_instead_of_emptying() {
        // Reachable for real: closing apps can shrink the list while the last
        // page is open, and an empty screen would look like a crash.
        let grid = compute(10, 9, WIDTH, HEIGHT, &theme());
        assert_eq!(grid.page, 0);
        assert_eq!(grid.cells.len(), 10);
    }

    #[test]
    fn one_page_has_no_dots() {
        assert!(compute(12, 0, WIDTH, HEIGHT, &theme()).dots.is_empty());
        assert_eq!(compute(80, 0, WIDTH, HEIGHT, &theme()).dots.len(), 3);
    }

    #[test]
    fn no_apps_still_yields_one_page() {
        let grid = compute(0, 0, WIDTH, HEIGHT, &theme());
        assert_eq!(grid.pages, 1);
        assert!(grid.cells.is_empty());
    }

    #[test]
    fn a_tiny_screen_still_gets_one_cell() {
        let grid = compute(9, 0, 320.0, 240.0, &theme());
        assert!(grid.columns >= 1 && grid.rows >= 1);
        assert!(!grid.cells.is_empty());
    }

    #[test]
    fn the_reserved_strip_is_left_clear_for_the_dock() {
        let mut theme = theme();
        theme.reserved_bottom = 90.0;
        let grid = compute(35, 0, WIDTH, HEIGHT, &theme);
        for cell in &grid.cells {
            assert!(
                cell.y + cell.height <= HEIGHT - theme.reserved_bottom,
                "cell overlaps the dock: {cell:?}"
            );
        }
    }

    #[test]
    fn hit_testing_finds_the_cell_under_the_cursor() {
        let grid = compute(40, 1, WIDTH, HEIGHT, &theme());
        let cell = grid.cells[2];
        assert_eq!(
            hit_test(&grid, cell.center_x(), cell.y + cell.height / 2.0),
            Some(cell.index)
        );
        // Page 1 starts at 35, so hit testing must report absolute indices.
        assert_eq!(grid.cells[2].index, 37);
        assert_eq!(hit_test(&grid, 0.0, 0.0), None);
    }

    #[test]
    fn drawing_paints_icons_over_the_backdrop() {
        let apps: Vec<String> = (0..12).map(|i| format!("App {i}")).collect();
        let pixmap_icon = icon();
        let items: Vec<GridItem> = apps
            .iter()
            .map(|name| GridItem {
                name,
                icon: Some(&pixmap_icon),
            })
            .collect();

        let theme = theme();
        let grid = compute(items.len(), 0, WIDTH, HEIGHT, &theme);
        let mut target = Pixmap::new(WIDTH as u32, HEIGHT as u32).unwrap();
        draw(
            &mut target.as_mut(),
            &grid,
            &items,
            &theme,
            None,
            Some(0),
            1.0,
            1.0,
        );

        let cell = grid.cells[0];
        let px = target
            .pixel(
                cell.center_x() as u32,
                (cell.icon_y + cell.icon_size / 2.0) as u32,
            )
            .unwrap();
        assert!(px.red() > 150, "icon not drawn: {px:?}");

        // The backdrop covers the screen but stays translucent, so the
        // compositor's blur of the desktop behind still comes through.
        let backdrop = target.pixel(4, 4).unwrap();
        assert!(backdrop.alpha() > 100 && backdrop.alpha() < 255);
    }

    #[test]
    fn the_reserved_strip_is_left_transparent() {
        let theme = LaunchpadTheme {
            reserved_bottom: 80.0,
            ..LaunchpadTheme::default()
        };
        let grid = compute(6, 0, WIDTH, HEIGHT, &theme);
        let mut target = Pixmap::new(WIDTH as u32, HEIGHT as u32).unwrap();
        draw(
            &mut target.as_mut(),
            &grid,
            &[],
            &theme,
            None,
            None,
            1.0,
            1.0,
        );
        // Where the dock sits, nothing may be painted at all.
        assert_eq!(target.pixel(960, HEIGHT as u32 - 10).unwrap().alpha(), 0);
    }

    #[test]
    fn the_opening_frame_is_faint_and_the_settled_one_is_not() {
        let theme = theme();
        let grid = compute(6, 0, WIDTH, HEIGHT, &theme);
        let mut early = Pixmap::new(400, 400).unwrap();
        let mut late = Pixmap::new(400, 400).unwrap();
        draw(&mut early.as_mut(), &grid, &[], &theme, None, None, 0.05, 1.0);
        draw(&mut late.as_mut(), &grid, &[], &theme, None, None, 1.0, 1.0);
        assert!(early.pixel(10, 10).unwrap().alpha() < late.pixel(10, 10).unwrap().alpha());
    }

    #[test]
    fn drawing_without_icons_or_a_font_does_not_panic() {
        let theme = theme();
        let items: Vec<GridItem> = (0..5)
            .map(|_| GridItem {
                name: "No icon",
                icon: None,
            })
            .collect();
        let grid = compute(items.len(), 0, 800.0, 600.0, &theme);
        let mut target = Pixmap::new(800, 600).unwrap();
        draw(
            &mut target.as_mut(),
            &grid,
            &items,
            &theme,
            None,
            None,
            1.0,
            2.0,
        );
    }
}
