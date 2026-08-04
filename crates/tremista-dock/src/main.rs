//! A macOS-style dock, drawn as a `wlr-layer-shell` surface.
//!
//! The dock is an ordinary Wayland client, not part of the compositor. It
//! anchors itself to the bottom of the screen, learns about open windows through
//! `wlr-foreign-toplevel-management`, and does all its drawing on the CPU with
//! tiny-skia -- a dock is a few hundred small blits per frame, well below the
//! point where handing work to the GPU would pay for the context it costs.

mod config;
mod launcher;
mod launchpad;
mod toplevels;

use anyhow::{anyhow, Context, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::time::Instant;
use tiny_skia::Pixmap;
use tremista_dock_core::{
    bounce_offset, compute_layout, draw, hit_test, matches_app_id, model::resolve_pinned, DockItem,
    Font, IconCache, LaunchpadTheme, Layout, RenderItem, Theme,
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;

use launcher::Launcher;
use launchpad::Launchpad;
use toplevels::Toplevel;

pub struct Dock {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    compositor: CompositorState,

    layer_shell: LayerShell,
    layer: LayerSurface,
    pool: SlotPool,
    pointer: Option<wl_pointer::WlPointer>,
    /// Bound only so Launchpad can see Escape and the arrow keys; the dock
    /// itself never takes keyboard focus.
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// Needed to `activate` a toplevel: the protocol wants a seat as evidence
    /// the focus change came from user input.
    pub seat: Option<wl_seat::WlSeat>,
    /// Held only to keep the global bound; we never send it a request.
    _toplevel_manager: Option<ZwlrForeignToplevelManagerV1>,

    /// Surface size in logical pixels. The width spans the whole output.
    width: u32,
    height: u32,
    scale: i32,

    theme: Theme,
    icons: IconCache,
    /// Apps the user pinned, in order. These stay put whether running or not.
    pinned: Vec<DockItem>,
    /// What is actually shown: the pinned apps, then any running app that is
    /// not pinned -- the way macOS appends unpinned running apps on the right
    /// -- and finally the "All Apps" icon, which is always the rightmost.
    visible: Vec<DockItem>,
    /// The "All Apps" entry, rebuilt into `visible` on every change.
    launchpad: DockItem,
    /// The Launchpad grid, present only while it is open.
    launchpad_view: Option<Launchpad>,
    launchpad_theme: LaunchpadTheme,
    /// Every installed app, filled in the first time Launchpad opens.
    all_apps: Vec<DockItem>,
    /// For Launchpad's labels. `None` if no usable font was found, in which
    /// case the grid simply has no captions.
    font: Option<Font>,
    pub toplevels: Vec<Toplevel>,

    cursor_x: Option<f32>,
    pressed: Option<usize>,
    layout: Layout,
    /// Launch bounces in flight, keyed by app id. An index would be wrong,
    /// because the item list shifts as windows open and close.
    bounces: Vec<(String, Instant)>,

    scratch: Pixmap,
    launcher: Launcher,

    /// The compositor chooses our width, so nothing can be drawn until it has.
    configured: bool,
    dirty: bool,
    /// Set between committing a frame and the compositor asking for the next
    /// one. Drawing while it is set would only queue work that gets discarded.
    frame_pending: bool,
    input_region_dirty: bool,
    exit: bool,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let conn = Connection::connect_to_env().context("connecting to the Wayland display")?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor not available")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context(
        "the compositor does not support wlr-layer-shell, which the dock needs to anchor itself",
    )?;

    let theme = Theme::default();
    let height = theme.surface_height().ceil() as u32;
    // Launchpad leaves the dock's strip alone, so the dock stays visible and
    // clickable underneath the grid, as it does on macOS.
    let launchpad_theme = LaunchpadTheme {
        reserved_bottom: theme.background_height() + theme.margin_bottom,
        ..LaunchpadTheme::default()
    };

    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Top, Some("tremista-dock"), None);

    // Spanning the full width and centring the dock inside it means pointer
    // motion never has to resize the surface, which is what stops magnification
    // lagging a frame behind the cursor.
    layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, height);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Reserve only the resting plate. Magnified icons deliberately spill into
    // window territory rather than shoving windows around as the cursor moves.
    layer.set_exclusive_zone((theme.background_height() + theme.margin_bottom).ceil() as i32);
    layer.commit();

    // Room for one 4K-wide frame at scale 2; the pool grows if it has to.
    let pool = SlotPool::new(4096 * height as usize * 4, &shm).context("creating an shm pool")?;

    let pinned = resolve_pinned(&config::pinned());
    log::info!("{} pinned app(s) resolved", pinned.len());

    // Rasterise at the largest size any icon can reach, so magnifying one never
    // scales a small bitmap up.
    let icon_resolution = (theme.icon_size * theme.max_scale).ceil() as u32;
    let layout = compute_layout(0, 0.0, None, &theme);

    let mut dock = Dock {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        compositor,
        layer_shell,
        layer,
        pool,
        pointer: None,
        keyboard: None,
        seat: None,
        _toplevel_manager: None,
        width: 0,
        height,
        scale: 1,
        icons: IconCache::new(icon_resolution, None).with_overrides(config::icon_dirs()),
        pinned: pinned.clone(),
        visible: pinned,
        launchpad: tremista_dock_core::model::launchpad(),
        launchpad_view: None,
        launchpad_theme,
        all_apps: Vec::new(),
        font: Font::load(),
        toplevels: Vec::new(),
        cursor_x: None,
        pressed: None,
        layout,
        bounces: Vec::new(),
        scratch: Pixmap::new(1, 1).ok_or_else(|| anyhow!("allocating the scratch pixmap"))?,
        launcher: Launcher::new(),
        theme,
        configured: false,
        dirty: true,
        frame_pending: false,
        input_region_dirty: true,
        exit: false,
    };

    // Optional: without it the dock still launches apps, it just cannot show
    // which of them are running or focus their windows.
    match globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, toplevels::ToplevelData) {
        Ok(manager) => dock._toplevel_manager = Some(manager),
        Err(e) => log::warn!(
            "wlr-foreign-toplevel-management unavailable ({e}); \
             running indicators and click-to-focus are disabled"
        ),
    }

    // Appends "All Apps" to the pinned list. Done here rather than in the
    // initialiser so it also happens when there is no toplevel manager to
    // trigger the first rebuild.
    dock.rebuild_items();

    while !dock.exit {
        event_queue.blocking_dispatch(&mut dock)?;
        dock.draw_if_needed(&qh);
        dock.draw_launchpad_if_needed(&qh);
    }

    Ok(())
}

impl Dock {
    /// Recompute the visible item list from the pinned set plus open windows.
    pub fn rebuild_items(&mut self) {
        let mut visible = self.pinned.clone();

        for item in &mut visible {
            item.running = self
                .toplevels
                .iter()
                .any(|t| !t.app_id.is_empty() && matches_app_id(item, &t.app_id));
        }

        // Running apps that are not pinned get appended, de-duplicated by app
        // id so a five-window editor is still one icon.
        for toplevel in &self.toplevels {
            if toplevel.app_id.is_empty() {
                continue;
            }
            if visible.iter().any(|i| matches_app_id(i, &toplevel.app_id)) {
                continue;
            }
            visible.push(DockItem {
                app_id: toplevel.app_id.clone(),
                name: if toplevel.title.is_empty() {
                    toplevel.app_id.clone()
                } else {
                    toplevel.title.clone()
                },
                // Unpinned entries are never launched from -- they always have
                // a window to focus instead.
                exec: String::new(),
                icon_name: toplevel.app_id.clone(),
                running: true,
                pinned: false,
            });
        }

        // Always last, so it stays at the right end however many windows are
        // open -- the one entry whose position never moves.
        visible.push(self.launchpad.clone());

        if visible != self.visible {
            self.visible = visible;
            self.input_region_dirty = true;
            self.dirty = true;
        }
    }

    fn draw_if_needed(&mut self, qh: &QueueHandle<Self>) {
        if !self.configured || self.frame_pending || self.width == 0 {
            return;
        }
        // A bounce in flight means the next frame differs from this one even
        // when nothing else has changed.
        if !self.dirty && !self.bouncing() {
            return;
        }
        if let Err(e) = self.draw(qh) {
            log::error!("drawing a frame: {e:#}");
            // Don't spin: a failure repeating every frame would peg a core.
            self.dirty = false;
        }
    }

    fn bouncing(&self) -> bool {
        self.bounces
            .iter()
            .any(|(_, started)| started.elapsed().as_secs_f32() < self.theme.bounce_duration)
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        self.launcher.reap();
        self.bounces
            .retain(|(_, started)| started.elapsed().as_secs_f32() < self.theme.bounce_duration);

        let scale = self.scale.max(1);
        let width_px = self.width as i32 * scale;
        let height_px = self.height as i32 * scale;

        self.layout = compute_layout(
            self.visible.len(),
            self.width as f32,
            self.cursor_x,
            &self.theme,
        );

        if self.scratch.width() != width_px as u32 || self.scratch.height() != height_px as u32 {
            self.scratch = Pixmap::new(width_px as u32, height_px as u32)
                .ok_or_else(|| anyhow!("allocating a {width_px}x{height_px} pixmap"))?;
        }

        // Warm the icon cache before rendering: `draw` needs several icons
        // borrowed at once, which the cache can only hand out immutably.
        for item in &self.visible {
            self.icons.get(&item.icon_name);
        }

        {
            let (icons, bounces, theme) = (&self.icons, &self.bounces, &self.theme);
            let renders: Vec<RenderItem> = self
                .visible
                .iter()
                .map(|item| RenderItem {
                    item,
                    icon: icons.peek(&item.icon_name),
                    bounce: bounces
                        .iter()
                        .find(|(id, _)| *id == item.app_id)
                        .map(|(_, started)| bounce_offset(started.elapsed().as_secs_f32(), theme))
                        .unwrap_or(0.0),
                })
                .collect();

            draw(
                &mut self.scratch.as_mut(),
                &self.layout,
                &renders,
                theme,
                scale as f32,
            );
        }

        // Prefer a format whose byte order already matches tiny-skia's, so the
        // copy below is a straight memcpy. Argb8888 is the only format wl_shm
        // is required to advertise, so it stays as the fallback.
        let format = if self.shm.formats().contains(&wl_shm::Format::Abgr8888) {
            wl_shm::Format::Abgr8888
        } else {
            wl_shm::Format::Argb8888
        };

        let (buffer, canvas) = self
            .pool
            .create_buffer(width_px, height_px, width_px * 4, format)
            .context("creating an shm buffer")?;

        copy_pixels(format, self.scratch.data(), canvas);

        if self.input_region_dirty {
            self.apply_input_region()?;
            self.input_region_dirty = false;
        }
        self.update_minimize_targets();

        let surface = self.layer.wl_surface();
        let _ = self.layer.set_buffer_scale(scale as u32);
        surface.damage_buffer(0, 0, width_px, height_px);
        // Ask for the next frame *before* committing, so the compositor knows
        // we intend to keep animating.
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).context("attaching the buffer")?;
        self.layer.commit();

        self.frame_pending = true;
        self.dirty = false;
        Ok(())
    }

    /// Restrict input to the dock plate.
    ///
    /// Without this the surface spans the whole width of the screen and would
    /// swallow every click along the bottom edge. The region is sized to the
    /// dock at full magnification rather than at rest: a region that shrank as
    /// icons collapsed would pull itself out from under the cursor, and the
    /// dock would flicker between magnified and resting.
    fn apply_input_region(&self) -> Result<()> {
        let region = Region::new(&self.compositor).context("creating an input region")?;

        if !self.visible.is_empty() {
            // The row is at its widest with the cursor at the centre, where the
            // most icons fall inside the magnification falloff.
            let widest = compute_layout(
                self.visible.len(),
                self.width as f32,
                Some(self.width as f32 / 2.0),
                &self.theme,
            );
            let (x, y, w, _) = widest.background;
            region.add(
                x.floor() as i32,
                y.floor() as i32,
                w.ceil() as i32,
                (self.height as f32 - y).ceil() as i32,
            );
        }

        self.layer.set_input_region(Some(region.wl_region()));
        Ok(())
    }

    /// Tell the compositor which icon each window belongs to, so minimising
    /// animates into that icon instead of into a corner of the screen.
    fn update_minimize_targets(&self) {
        let surface = self.layer.wl_surface();
        for toplevel in &self.toplevels {
            if toplevel.app_id.is_empty() {
                continue;
            }
            let Some(index) = self
                .visible
                .iter()
                .position(|i| matches_app_id(i, &toplevel.app_id))
            else {
                continue;
            };
            let Some(g) = self.layout.items.get(index) else {
                continue;
            };
            toplevel
                .handle
                .set_rectangle(surface, g.x as i32, g.y as i32, g.size as i32, g.size as i32);
        }
    }

    fn on_click(&mut self, index: usize, qh: &QueueHandle<Self>) {
        let Some(item) = self.visible.get(index).cloned() else {
            return;
        };

        // Not an app: it opens the dock's own grid rather than starting
        // anything, so it is handled before the running/exec paths.
        if item.app_id == tremista_dock_core::model::LAUNCHPAD_APP_ID {
            self.toggle_launchpad(qh);
            return;
        }

        if item.running {
            self.activate_or_minimize(&item.app_id);
            return;
        }

        if item.exec.is_empty() {
            return;
        }
        match self.launcher.spawn(&item.exec) {
            Ok(()) => {
                self.bounces.retain(|(id, _)| *id != item.app_id);
                self.bounces.push((item.app_id, Instant::now()));
                self.dirty = true;
            }
            Err(e) => log::error!("{e:#}"),
        }
    }
}

/// Copy a premultiplied RGBA pixmap into a wl_shm buffer.
fn copy_pixels(format: wl_shm::Format, pixels: &[u8], canvas: &mut [u8]) {
    match format {
        // tiny-skia stores premultiplied R,G,B,A bytes, which is exactly what
        // Abgr8888 means on a little-endian machine.
        wl_shm::Format::Abgr8888 => canvas.copy_from_slice(pixels),
        // Argb8888 is a native-endian 0xAARRGGBB word, i.e. B,G,R,A bytes, so
        // red and blue have to be swapped on the way out.
        _ => {
            for (dst, src) in canvas.chunks_exact_mut(4).zip(pixels.chunks_exact(4)) {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
                dst[3] = src[3];
            }
        }
    }
}

impl CompositorHandler for Dock {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if new_factor.max(1) == self.scale {
            return;
        }
        self.scale = new_factor.max(1);
        // Cached icons were rasterised for the old scale and would look soft.
        let resolution = self.theme.icon_size * self.theme.max_scale * self.scale as f32;
        self.icons.set_resolution(resolution.ceil() as u32);
        self.dirty = true;
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // Two surfaces animate independently, so the callback has to say which
        // one is ready for its next frame.
        if let Some(view) = self.launchpad_view.as_mut() {
            if view.owns(surface) {
                view.frame_done();
                return;
            }
        }
        self.frame_pending = false;
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for Dock {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // The compositor closing the grid is just a dismiss; only the dock's
        // own surface going away means the dock is done.
        if self.launchpad_view.as_ref().is_some_and(|v| v.owns(layer.wl_surface())) {
            self.close_launchpad();
            return;
        }
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;

        if let Some(view) = self.launchpad_view.as_mut() {
            if view.owns(layer.wl_surface()) {
                view.configure(width, height);
                return;
            }
        }

        if width != 0 && width != self.width {
            self.width = width;
            self.input_region_dirty = true;
        }
        if height != 0 {
            self.height = height;
        }
        self.configured = true;
        self.dirty = true;
    }
}

impl SeatHandler for Dock {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer if self.pointer.is_none() => {
                match self.seat_state.get_pointer(qh, &seat) {
                    Ok(pointer) => {
                        self.pointer = Some(pointer);
                        self.seat = Some(seat);
                    }
                    Err(e) => log::error!("acquiring the pointer: {e}"),
                }
            }
            // Bound through the raw protocol rather than sctk's keyboard
            // helper: that one wants a keymap, and a keymap wants libxkbcommon.
            // Launchpad only needs Escape and the arrow keys, which are fixed
            // evdev codes on every layout.
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = Some(seat.get_keyboard(qh, launchpad::KeyboardData));
                self.seat.get_or_insert(seat);
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.seat.as_ref() == Some(&seat) {
            self.seat = None;
        }
    }
}

impl PointerHandler for Dock {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if self
                .launchpad_view
                .as_ref()
                .is_some_and(|v| v.owns(&event.surface))
            {
                self.launchpad_pointer(event);
                continue;
            }
            if &event.surface != self.layer.wl_surface() {
                continue;
            }
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);

            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.cursor_x = Some(x);
                    self.dirty = true;
                }
                PointerEventKind::Leave { .. } => {
                    self.cursor_x = None;
                    self.pressed = None;
                    self.dirty = true;
                }
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    self.pressed = hit_test(&self.layout, x, y);
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    // Act only if press and release landed on the same icon, so
                    // a drag that began elsewhere does not launch anything.
                    let released = hit_test(&self.layout, x, y);
                    if let (Some(pressed), Some(released)) = (self.pressed.take(), released) {
                        if pressed == released {
                            self.on_click(released, qh);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl OutputHandler for Dock {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for Dock {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Dock {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(Dock);
// One blanket impl covers every sctk-owned object: since 0.21 the per-protocol
// `delegate_*!` macros are gone, and each `*Data` type carries its own dispatch.
// Our foreign-toplevel objects stay out of its way by using a marker type of
// their own (`toplevels::ToplevelData`) rather than `()`.
delegate_dispatch2!(Dock);
