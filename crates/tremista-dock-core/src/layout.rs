use crate::theme::Theme;
use std::f32::consts::PI;

/// Where a single icon ended up, in logical pixels relative to the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemGeometry {
    /// Left edge of the (scaled) icon box.
    pub x: f32,
    /// Top edge of the (scaled) icon box.
    pub y: f32,
    /// Edge length of the scaled icon box.
    pub size: f32,
    /// Magnification applied, 1.0 when the cursor is away.
    pub scale: f32,
}

impl ItemGeometry {
    pub fn center_x(&self) -> f32 {
        self.x + self.size / 2.0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.size && y >= self.y && y < self.y + self.size
    }
}

/// Resolved geometry for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub items: Vec<ItemGeometry>,
    /// The dock background plate: (x, y, width, height).
    pub background: (f32, f32, f32, f32),
}

impl Layout {
    /// Slide the whole dock by `dy` logical pixels.
    ///
    /// Auto-hide is drawn rather than moved: the surface stays where it is and
    /// the plate slides out of the bottom of it, so hiding costs no round trip
    /// to the compositor and cannot fight with the layer surface's anchoring.
    pub fn offset_y(&mut self, dy: f32) {
        if dy == 0.0 {
            return;
        }
        for item in &mut self.items {
            item.y += dy;
        }
        self.background.1 += dy;
    }
}

/// Magnification factor for an icon whose resting centre sits `distance`
/// logical pixels from the cursor.
///
/// Raised cosine rather than a linear ramp: it is smooth at both ends, so
/// icons ease into and out of magnification instead of visibly cornering as
/// the cursor crosses the influence boundary.
fn magnification(distance: f32, theme: &Theme) -> f32 {
    if theme.influence <= 0.0 {
        return 1.0;
    }
    let t = (distance.abs() / theme.influence).clamp(0.0, 1.0);
    let falloff = 0.5 * (1.0 + (PI * t).cos());
    1.0 + (theme.max_scale - 1.0) * falloff
}

/// Compute the dock layout.
///
/// `surface_width` is the full width of the layer surface (we span the whole
/// output and centre the dock inside it, which avoids resizing the surface on
/// every pointer move). `cursor_x` is `None` when the pointer is not over the
/// dock, which collapses every icon to its resting size.
pub fn compute(count: usize, surface_width: f32, cursor_x: Option<f32>, theme: &Theme) -> Layout {
    let bg_height = theme.background_height();
    let surface_height = theme.surface_height();
    // Icons sit on a common baseline at the bottom of the background plate, so
    // magnified ones grow upward out of the dock.
    let baseline = surface_height - theme.margin_bottom - theme.padding;

    if count == 0 {
        let bg_y = surface_height - theme.margin_bottom - bg_height;
        return Layout {
            items: Vec::new(),
            background: (surface_width / 2.0, bg_y, 0.0, bg_height),
        };
    }

    // Resting layout first: magnification is a function of each icon's
    // *resting* centre, not its magnified one. Using the resting position
    // keeps the mapping stable; deriving it from the moving position would
    // feed back on itself and jitter.
    let resting_width = count as f32 * theme.icon_size + (count - 1) as f32 * theme.gap;
    let resting_start = (surface_width - resting_width) / 2.0;

    let scales: Vec<f32> = (0..count)
        .map(|i| {
            let resting_center =
                resting_start + i as f32 * (theme.icon_size + theme.gap) + theme.icon_size / 2.0;
            match cursor_x {
                Some(cx) => magnification(resting_center - cx, theme),
                None => 1.0,
            }
        })
        .collect();

    // Lay the scaled icons out sequentially. Because widths grew, the row
    // spreads outward from the cursor on its own -- this is what produces the
    // macOS "wave" rather than icons overlapping in place.
    let total_width: f32 = scales.iter().map(|s| s * theme.icon_size).sum::<f32>()
        + (count - 1) as f32 * theme.gap;
    let mut pen = (surface_width - total_width) / 2.0;

    let mut items = Vec::with_capacity(count);
    for &scale in &scales {
        let size = theme.icon_size * scale;
        items.push(ItemGeometry {
            x: pen,
            y: baseline - size,
            size,
            scale,
        });
        pen += size + theme.gap;
    }

    let bg_x = (surface_width - total_width) / 2.0 - theme.padding;
    let bg_y = surface_height - theme.margin_bottom - bg_height;
    Layout {
        items,
        background: (bg_x, bg_y, total_width + theme.padding * 2.0, bg_height),
    }
}

/// Vertical offset of a launch bounce, `elapsed` seconds in. Returns 0 once
/// the animation is over.
///
/// Two decaying hops, each a sine arch, which reads much closer to the macOS
/// bounce than a single parabola.
pub fn bounce_offset(elapsed: f32, theme: &Theme) -> f32 {
    if elapsed < 0.0 || elapsed >= theme.bounce_duration {
        return 0.0;
    }
    let t = elapsed / theme.bounce_duration;
    let arch = (PI * t * 2.0).sin().abs();
    let decay = 1.0 - t;
    -theme.bounce_height * arch * decay * decay
}

/// Index of the icon under a point, if any.
pub fn hit_test(layout: &Layout, x: f32, y: f32) -> Option<usize> {
    layout.items.iter().position(|g| g.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn resting_layout_is_centred_and_unmagnified() {
        let t = theme();
        let l = compute(5, 1000.0, None, &t);
        assert_eq!(l.items.len(), 5);
        for g in &l.items {
            assert_eq!(g.scale, 1.0);
            assert_eq!(g.size, t.icon_size);
        }
        // Symmetric about the surface centre.
        let first = l.items.first().unwrap();
        let last = l.items.last().unwrap();
        let left_margin = first.x;
        let right_margin = 1000.0 - (last.x + last.size);
        assert!((left_margin - right_margin).abs() < 0.01);
    }

    #[test]
    fn cursor_magnifies_nearest_icon_most() {
        let t = theme();
        let resting = compute(7, 1000.0, None, &t);
        let target = resting.items[3].center_x();
        let l = compute(7, 1000.0, Some(target), &t);

        let peak = l.items[3].scale;
        assert!((peak - t.max_scale).abs() < 0.001, "peak was {peak}");
        // Scale decreases monotonically as we walk away from the cursor.
        for i in 0..3 {
            assert!(l.items[i].scale < l.items[i + 1].scale);
        }
        for i in 4..7 {
            assert!(l.items[i].scale < l.items[i - 1].scale);
        }
    }

    #[test]
    fn magnified_icons_never_overlap() {
        let t = theme();
        for cursor in (0..1000).step_by(7) {
            let l = compute(9, 1000.0, Some(cursor as f32), &t);
            for pair in l.items.windows(2) {
                let gap = pair[1].x - (pair[0].x + pair[0].size);
                assert!(
                    gap >= t.gap - 0.001,
                    "icons overlapped at cursor {cursor}: gap {gap}"
                );
            }
        }
    }

    #[test]
    fn icons_share_a_baseline() {
        let t = theme();
        let l = compute(6, 1000.0, Some(500.0), &t);
        let baselines: Vec<f32> = l.items.iter().map(|g| g.y + g.size).collect();
        for b in &baselines {
            assert!((b - baselines[0]).abs() < 0.001, "baseline drift: {baselines:?}");
        }
    }

    #[test]
    fn magnification_is_continuous_across_the_influence_edge() {
        let t = theme();
        // Just inside the influence radius the scale must already be ~1.0,
        // otherwise icons visibly pop as the wave passes over them.
        let just_inside = magnification(t.influence - 0.01, &t);
        assert!((just_inside - 1.0).abs() < 0.001, "got {just_inside}");
        let outside = magnification(t.influence + 50.0, &t);
        assert_eq!(outside, 1.0);
    }

    #[test]
    fn empty_dock_does_not_panic() {
        let l = compute(0, 1000.0, Some(500.0), &theme());
        assert!(l.items.is_empty());
        assert_eq!(l.background.2, 0.0);
    }

    #[test]
    fn bounce_starts_and_ends_at_rest() {
        let t = theme();
        assert_eq!(bounce_offset(0.0, &t), 0.0);
        assert_eq!(bounce_offset(t.bounce_duration, &t), 0.0);
        assert_eq!(bounce_offset(t.bounce_duration + 1.0, &t), 0.0);
        // Peaks upward (negative y) somewhere in the first half.
        let mid = bounce_offset(t.bounce_duration * 0.25, &t);
        assert!(mid < -1.0, "expected an upward hop, got {mid}");
    }

    #[test]
    fn offsetting_moves_icons_and_plate_together() {
        let t = theme();
        let mut l = compute(4, 1000.0, None, &t);
        let before = l.clone();
        l.offset_y(40.0);
        for (a, b) in l.items.iter().zip(&before.items) {
            assert_eq!(a.y - b.y, 40.0);
            assert_eq!(a.x, b.x);
        }
        assert_eq!(l.background.1 - before.background.1, 40.0);
        assert_eq!(l.background.3, before.background.3);
    }

    #[test]
    fn hit_test_finds_the_hovered_icon() {
        let t = theme();
        let l = compute(5, 1000.0, None, &t);
        let g = l.items[2];
        assert_eq!(hit_test(&l, g.center_x(), g.y + g.size / 2.0), Some(2));
        assert_eq!(hit_test(&l, 5.0, 5.0), None);
    }
}
