use tiny_skia::Color;

/// Visual and geometric parameters for the dock. All lengths are in *logical*
/// pixels; the renderer multiplies by the output scale factor.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Edge length of an un-magnified icon.
    pub icon_size: f32,
    /// Scale applied to the icon directly under the cursor.
    pub max_scale: f32,
    /// How far the magnification reaches, in logical px measured from the
    /// cursor along the dock axis. macOS feels right at roughly 2.5 icons.
    pub influence: f32,
    /// Horizontal gap between adjacent un-magnified icons.
    pub gap: f32,
    /// Padding between the icon row and the dock background edge.
    pub padding: f32,
    /// Gap between the dock background and the bottom of the screen.
    pub margin_bottom: f32,
    /// Corner radius of the dock background.
    pub radius: f32,

    pub background: Color,
    /// Hairline along the top edge; this is what reads as "glass" against a
    /// wallpaper and is the single biggest cue that separates this from a
    /// flat panel.
    pub border: Color,
    pub indicator: Color,

    /// Peak height of the launch bounce.
    pub bounce_height: f32,
    /// Duration of one bounce, in seconds.
    pub bounce_duration: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            icon_size: 52.0,
            max_scale: 1.9,
            // ~3 icons of reach each side. Narrower than this and the icons
            // beyond the wave sit visibly dead while their neighbours balloon;
            // the gradual shoulder is most of what sells the effect.
            influence: 190.0,
            gap: 6.0,
            // Must comfortably clear `radius`, or a magnified end icon crowds
            // the plate's rounded corner.
            padding: 11.0,
            margin_bottom: 8.0,
            radius: 22.0,
            // Deliberately low alpha: the compositor blurs whatever is behind
            // the surface, and a heavy fill would throw that blur away.
            background: Color::from_rgba8(28, 28, 30, 130),
            border: Color::from_rgba8(255, 255, 255, 38),
            indicator: Color::from_rgba8(255, 255, 255, 190),
            bounce_height: 34.0,
            bounce_duration: 0.62,
        }
    }
}

impl Theme {
    /// Height the dock background occupies. Magnified icons deliberately grow
    /// *above* this, the way they do on macOS, so it stays constant.
    pub fn background_height(&self) -> f32 {
        self.icon_size + self.padding * 2.0
    }

    /// Height the Wayland surface must reserve to draw everything without
    /// clipping: a fully magnified icon plus its bounce and the bottom margin.
    pub fn surface_height(&self) -> f32 {
        self.icon_size * self.max_scale + self.padding * 2.0 + self.margin_bottom + self.bounce_height
    }
}
