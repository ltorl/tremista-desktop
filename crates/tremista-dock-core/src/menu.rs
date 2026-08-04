//! The dock's context menu.
//!
//! Small enough to be worth having rather than pulling in a toolkit: a column
//! of labels on a rounded plate, laid out and drawn the same way as everything
//! else here. Geometry only -- the surface it lands on is the shell's problem.

use crate::render::squircle;
use crate::text::Font;
use tiny_skia::{Color, FillRule, Paint, PixmapMut, Stroke, Transform};

/// Visual parameters, in logical pixels.
#[derive(Debug, Clone)]
pub struct MenuTheme {
    pub item_height: f32,
    /// Horizontal padding either side of a label.
    pub padding_x: f32,
    /// Vertical padding above the first item and below the last.
    pub padding_y: f32,
    pub font_size: f32,
    pub radius: f32,
    pub min_width: f32,
    /// Gap kept between the menu and the edges of the screen.
    pub screen_margin: f32,
    /// Inset of the highlight from the plate edge, so the selected row reads as
    /// a pill inside the menu rather than a full-width band.
    pub highlight_inset: f32,
    pub highlight_radius: f32,

    pub background: Color,
    pub border: Color,
    pub label: Color,
    pub highlight: Color,
    pub highlight_label: Color,
}

impl Default for MenuTheme {
    fn default() -> Self {
        Self {
            item_height: 30.0,
            padding_x: 16.0,
            padding_y: 6.0,
            font_size: 13.0,
            radius: 12.0,
            min_width: 180.0,
            screen_margin: 8.0,
            highlight_inset: 5.0,
            highlight_radius: 7.0,
            // Much more opaque than the dock: a menu has to be readable over
            // whatever it lands on, and it is only on screen for a moment.
            background: Color::from_rgba8(38, 38, 42, 240),
            border: Color::from_rgba8(255, 255, 255, 30),
            label: Color::from_rgba8(255, 255, 255, 238),
            // macOS selection blue.
            highlight: Color::from_rgba8(0, 122, 255, 235),
            highlight_label: Color::from_rgba8(255, 255, 255, 255),
        }
    }
}

/// A placed menu. Coordinates are relative to the surface it will be drawn on.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub items: usize,
    theme_item_height: f32,
    theme_padding_y: f32,
}

impl Menu {
    /// Top edge of row `index`.
    pub fn item_y(&self, index: usize) -> f32 {
        self.y + self.theme_padding_y + index as f32 * self.theme_item_height
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Place a menu whose *bottom* edge sits at `anchor_bottom`, opening upward
/// from a click at `anchor_x` -- which is how a menu over a dock at the bottom
/// of the screen has to behave.
///
/// The result is kept inside `bounds`, so a right-click at the far end of the
/// dock does not put half the menu off screen.
pub fn compute(
    labels: &[String],
    anchor_x: f32,
    anchor_bottom: f32,
    bounds: (f32, f32),
    font: Option<&Font>,
    theme: &MenuTheme,
) -> Menu {
    let text_width = font
        .map(|f| {
            labels
                .iter()
                .map(|label| f.measure(label, theme.font_size))
                .fold(0.0_f32, f32::max)
        })
        .unwrap_or(0.0);
    let width = (text_width + theme.padding_x * 2.0).max(theme.min_width);
    let height = labels.len() as f32 * theme.item_height + theme.padding_y * 2.0;

    let (bounds_width, _bounds_height) = bounds;
    let max_x = (bounds_width - width - theme.screen_margin).max(theme.screen_margin);
    let x = (anchor_x - width / 2.0).clamp(theme.screen_margin, max_x);
    let y = (anchor_bottom - height).max(theme.screen_margin);

    Menu {
        x,
        y,
        width,
        height,
        items: labels.len(),
        theme_item_height: theme.item_height,
        theme_padding_y: theme.padding_y,
    }
}

/// Which row is under a point, if any.
pub fn hit_test(menu: &Menu, x: f32, y: f32) -> Option<usize> {
    if !menu.contains(x, y) {
        return None;
    }
    let row = (y - menu.y - menu.theme_padding_y) / menu.theme_item_height;
    if row < 0.0 {
        return None;
    }
    let row = row as usize;
    (row < menu.items).then_some(row)
}

/// Composite the menu into `target`, which it does not clear -- the caller owns
/// the rest of the surface.
pub fn draw(
    target: &mut PixmapMut,
    menu: &Menu,
    labels: &[String],
    theme: &MenuTheme,
    font: Option<&Font>,
    hovered: Option<usize>,
    scale: f32,
) {
    let transform = Transform::from_scale(scale, scale);
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };

    let Some(plate) = squircle(menu.x, menu.y, menu.width, menu.height, theme.radius) else {
        return;
    };
    paint.set_color(theme.background);
    target.fill_path(&plate, &paint, FillRule::Winding, transform, None);
    paint.set_color(theme.border);
    target.stroke_path(
        &plate,
        &paint,
        &Stroke {
            width: 1.0,
            ..Default::default()
        },
        transform,
        None,
    );

    for (index, label) in labels.iter().enumerate() {
        let top = menu.item_y(index);
        let selected = hovered == Some(index);

        if selected {
            if let Some(pill) = squircle(
                menu.x + theme.highlight_inset,
                top,
                menu.width - theme.highlight_inset * 2.0,
                theme.item_height,
                theme.highlight_radius,
            ) {
                paint.set_color(theme.highlight);
                target.fill_path(&pill, &paint, FillRule::Winding, transform, None);
            }
        }

        let Some(font) = font else {
            continue;
        };
        // Optical centring: text sits slightly above the geometric middle of a
        // row because most glyphs have no descender.
        let baseline = top + theme.item_height / 2.0 + theme.font_size * 0.35;
        let color = if selected {
            theme.highlight_label
        } else {
            theme.label
        };
        font.draw(
            target,
            label,
            menu.x + theme.padding_x,
            baseline,
            theme.font_size,
            color,
            scale,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Pixmap;

    fn labels() -> Vec<String> {
        vec![
            "Turn Magnification Off".to_owned(),
            "Turn Hiding On".to_owned(),
        ]
    }

    fn theme() -> MenuTheme {
        MenuTheme::default()
    }

    #[test]
    fn opens_upward_from_the_anchor() {
        let t = theme();
        let m = compute(&labels(), 500.0, 900.0, (1920.0, 1080.0), None, &t);
        // The bottom edge is the anchor, so the menu sits above the dock.
        assert!((m.y + m.height - 900.0).abs() < 0.01);
        assert!((m.x + m.width / 2.0 - 500.0).abs() < 0.01);
        assert_eq!(m.items, 2);
    }

    #[test]
    fn stays_on_screen_at_either_end() {
        let t = theme();
        let left = compute(&labels(), 2.0, 900.0, (1920.0, 1080.0), None, &t);
        assert!(left.x >= t.screen_margin - 0.01);
        let right = compute(&labels(), 1918.0, 900.0, (1920.0, 1080.0), None, &t);
        assert!(right.x + right.width <= 1920.0 - t.screen_margin + 0.01);
    }

    #[test]
    fn a_narrow_screen_does_not_produce_a_negative_position() {
        let m = compute(&labels(), 100.0, 300.0, (120.0, 300.0), None, &theme());
        assert!(m.x >= 0.0 && m.y >= 0.0);
    }

    #[test]
    fn hit_test_maps_rows_and_rejects_the_padding() {
        let t = theme();
        let m = compute(&labels(), 500.0, 900.0, (1920.0, 1080.0), None, &t);
        for index in 0..2 {
            let y = m.item_y(index) + t.item_height / 2.0;
            assert_eq!(hit_test(&m, m.x + 10.0, y), Some(index));
        }
        // Above the first row and below the last are padding, not items.
        assert_eq!(hit_test(&m, m.x + 10.0, m.y + 1.0), None);
        assert_eq!(hit_test(&m, m.x + 10.0, m.y + m.height - 1.0), None);
        // Outside entirely.
        assert_eq!(hit_test(&m, m.x - 5.0, m.y + 20.0), None);
    }

    #[test]
    fn drawing_touches_only_the_menu() {
        let t = theme();
        let m = compute(&labels(), 200.0, 300.0, (400.0, 400.0), None, &t);
        let mut pixmap = Pixmap::new(400, 400).unwrap();
        draw(
            &mut pixmap.as_mut(),
            &m,
            &labels(),
            &t,
            None,
            Some(0),
            1.0,
        );
        // A corner well away from the plate is untouched.
        assert_eq!(pixmap.pixel(2, 2).unwrap().alpha(), 0);
        let inside = pixmap
            .pixel((m.x + m.width / 2.0) as u32, (m.y + m.height / 2.0) as u32)
            .unwrap();
        assert!(inside.alpha() > 0);
    }

    #[test]
    fn drawing_with_a_font_and_hidpi_does_not_panic() {
        let t = theme();
        let m = compute(&labels(), 200.0, 300.0, (400.0, 400.0), None, &t);
        let mut pixmap = Pixmap::new(800, 800).unwrap();
        draw(
            &mut pixmap.as_mut(),
            &m,
            &labels(),
            &t,
            Font::load().as_ref(),
            None,
            2.0,
        );
    }
}
