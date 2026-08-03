use crate::layout::Layout;
use crate::model::DockItem;
use crate::theme::Theme;
use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PixmapMut, Rect, SpreadMode,
    Stroke, Transform,
};

/// Control-point ratio for approximating a quarter circle with a cubic Bezier.
const CIRCLE_KAPPA: f32 = 0.552_284_75;

/// How far corners are pushed toward continuous curvature. macOS corners are
/// not circular arcs -- they're closer to a superellipse, and the giveaway is
/// that curvature ramps in gradually instead of starting abruptly at the
/// tangent point. Extending the control points past the circular kappa
/// approximates that, and it is most of why the plate reads as "Apple-ish"
/// rather than "rounded rectangle".
const CORNER_SMOOTHING: f32 = 1.22;

/// Build a rounded rectangle with continuous-ish corners.
pub fn squircle(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    if r == 0.0 {
        return PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?).into();
    }

    let c = r * CIRCLE_KAPPA * CORNER_SMOOTHING;
    let (l, t, rt, b) = (x, y, x + w, y + h);

    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(rt - r, t);
    pb.cubic_to(rt - r + c, t, rt, t + r - c, rt, t + r);
    pb.line_to(rt, b - r);
    pb.cubic_to(rt, b - r + c, rt - r + c, b, rt - r, b);
    pb.line_to(l + r, b);
    pb.cubic_to(l + r - c, b, l, b - r + c, l, b - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - c, l + r - c, t, l + r, t);
    pb.close();
    pb.finish()
}

/// Per-item state the renderer needs that isn't geometry.
pub struct RenderItem<'a> {
    pub item: &'a DockItem,
    pub icon: Option<&'a Pixmap>,
    /// Vertical offset from the launch bounce, negative is upward.
    pub bounce: f32,
}

/// Composite one dock frame into `target`.
///
/// `scale` is the output's fractional scale factor; the layout is computed in
/// logical pixels and everything is scaled up at draw time so the result is
/// crisp on HiDPI without duplicating the layout maths.
pub fn draw(target: &mut PixmapMut, layout: &Layout, items: &[RenderItem], theme: &Theme, scale: f32) {
    target.fill(tiny_skia::Color::TRANSPARENT);
    let scale_tf = Transform::from_scale(scale, scale);

    // --- Dock plate -------------------------------------------------------
    let (bx, by, bw, bh) = layout.background;
    if bw > 0.0 {
        if let Some(plate) = squircle(bx, by, bw, bh, theme.radius) {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(theme.background);
            target.fill_path(&plate, &paint, FillRule::Winding, scale_tf, None);

            // Hairline rim. Drawn at 1 logical px so it stays a hairline at
            // any output scale.
            paint.set_color(theme.border);
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            target.stroke_path(&plate, &paint, &stroke, scale_tf, None);
        }
    }

    // --- Icons ------------------------------------------------------------
    for (geometry, render) in layout.items.iter().zip(items) {
        let Some(icon) = render.icon else { continue };
        let y = geometry.y + render.bounce;

        // Map the cached icon pixmap onto the (possibly magnified) icon box.
        // A Pattern with its own transform gives subpixel-accurate placement;
        // draw_pixmap would snap to integers and make the magnification wave
        // visibly stair-step.
        let icon_scale = geometry.size / icon.width() as f32;
        let pattern_tf = Transform::from_scale(icon_scale, icon_scale)
            .post_translate(geometry.x, y)
            .post_scale(scale, scale);

        let paint = Paint {
            shader: Pattern::new(
                icon.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                1.0,
                pattern_tf,
            ),
            anti_alias: true,
            ..Default::default()
        };

        if let Some(rect) = Rect::from_xywh(
            geometry.x * scale,
            y * scale,
            geometry.size * scale,
            geometry.size * scale,
        ) {
            target.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    // --- Running indicators ----------------------------------------------
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(theme.indicator);
    let dot_radius = 2.5;
    // Centred in the padding band below the icon baseline.
    let dot_y = by + bh - theme.padding / 2.0;

    for (geometry, render) in layout.items.iter().zip(items) {
        if !render.item.running {
            continue;
        }
        if let Some(dot) = squircle(
            geometry.center_x() - dot_radius,
            dot_y - dot_radius,
            dot_radius * 2.0,
            dot_radius * 2.0,
            dot_radius,
        ) {
            target.fill_path(&dot, &paint, FillRule::Winding, scale_tf, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;
    use crate::model::DockItem;

    fn item(running: bool) -> DockItem {
        DockItem {
            app_id: "test".into(),
            name: "Test".into(),
            exec: "true".into(),
            icon_name: "test".into(),
            running,
            pinned: true,
        }
    }

    fn solid_icon() -> Pixmap {
        let mut p = Pixmap::new(64, 64).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        p
    }

    #[test]
    fn squircle_is_inset_within_its_bounds() {
        let path = squircle(10.0, 20.0, 100.0, 50.0, 12.0).unwrap();
        let b = path.bounds();
        assert!(b.left() >= 9.9 && b.top() >= 19.9);
        assert!(b.right() <= 110.1 && b.bottom() <= 70.1);
    }

    #[test]
    fn squircle_clamps_oversized_radius() {
        // A radius larger than half the shorter edge must not invert the path.
        let path = squircle(0.0, 0.0, 40.0, 20.0, 999.0).unwrap();
        let b = path.bounds();
        assert!(b.width() <= 40.1 && b.height() <= 20.1);
    }

    #[test]
    fn squircle_rejects_degenerate_sizes() {
        assert!(squircle(0.0, 0.0, 0.0, 10.0, 4.0).is_none());
        assert!(squircle(0.0, 0.0, 10.0, -1.0, 4.0).is_none());
    }

    #[test]
    fn draw_paints_the_plate_and_icons() {
        let theme = Theme::default();
        let width = 600.0;
        let height = theme.surface_height();
        let mut pixmap = Pixmap::new(width as u32, height as u32).unwrap();

        let l = layout::compute(3, width, None, &theme);
        let icon = solid_icon();
        let items: Vec<DockItem> = (0..3).map(|_| item(true)).collect();
        let renders: Vec<RenderItem> = items
            .iter()
            .map(|it| RenderItem {
                item: it,
                icon: Some(&icon),
                bounce: 0.0,
            })
            .collect();

        draw(&mut pixmap.as_mut(), &l, &renders, &theme, 1.0);

        // Centre of the middle icon must be opaque red.
        let g = l.items[1];
        let px = pixmap
            .pixel(g.center_x() as u32, (g.y + g.size / 2.0) as u32)
            .unwrap();
        assert!(px.red() > 200, "icon not drawn: {px:?}");
        assert_eq!(px.alpha(), 255);

        // The top-left corner is outside the plate and must stay transparent,
        // which is what lets the compositor blur show through.
        assert_eq!(pixmap.pixel(2, 2).unwrap().alpha(), 0);
    }

    #[test]
    fn empty_dock_renders_without_panicking() {
        let theme = Theme::default();
        let mut pixmap = Pixmap::new(400, theme.surface_height() as u32).unwrap();
        let l = layout::compute(0, 400.0, None, &theme);
        draw(&mut pixmap.as_mut(), &l, &[], &theme, 1.0);
    }
}
