//! Launchpad: the full-screen grid of every installed app.
//!
//! It is not an application and never gets a window. It is a second layer-shell
//! surface owned by the dock, created when the "All Apps" icon is clicked and
//! destroyed when it is dismissed -- so nothing about it can show up in the dock
//! as a running app, or in an alt-tab list, or anywhere else a window would.
//!
//! The surface covers the output but deliberately leaves a strip at the bottom
//! out of its input region, so the dock underneath stays hoverable: clicking
//! "All Apps" again is how you close Launchpad, exactly as on macOS.

use crate::{copy_pixels, Dock};
use anyhow::{anyhow, Context, Result};
use smithay_client_toolkit::{
    compositor::{FrameCallbackData, Region},
    seat::pointer::{PointerEvent, PointerEventKind, BTN_LEFT, BTN_RIGHT},
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface},
        WaylandSurface,
    },
    shm::slot::SlotPool,
};
use std::time::Instant;
use tiny_skia::Pixmap;
use tremista_dock_core::{launchpad as grid, GridItem};
use wayland_client::{
    protocol::{wl_keyboard, wl_shm},
    Connection, Dispatch, QueueHandle,
};

/// Raw evdev keycodes, which is what `wl_keyboard` reports.
///
/// Used directly rather than translated through a keymap: xkbcommon is a C
/// library, and these three keys are in the same place on every layout. Nothing
/// here needs to know what letter a key produces -- that would only matter for
/// type-to-search, which Launchpad does not have.
const KEY_ESC: u32 = 1;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;

/// Scroll distance that turns one page. Roughly one notch of a mouse wheel, or
/// a short two-finger flick.
const PAGE_SCROLL: f32 = 60.0;

/// The open Launchpad surface. Dropping it destroys the surface, which is the
/// whole of "closing" Launchpad.
pub struct Launchpad {
    pub layer: LayerSurface,
    pool: SlotPool,
    scratch: Pixmap,

    /// Surface size in logical pixels, from the compositor.
    width: u32,
    height: u32,

    page: usize,
    hovered: Option<usize>,
    /// Where the left button went down: `Some(None)` means it went down on the
    /// background, which is a dismiss when the release lands there too.
    press: Option<Option<usize>>,
    scroll: f32,
    opened_at: Instant,
    grid: grid::Grid,

    configured: bool,
    frame_pending: bool,
    dirty: bool,
    region_dirty: bool,
}

impl Launchpad {
    fn new(dock: &Dock, qh: &QueueHandle<Dock>) -> Result<Self> {
        let surface = dock.compositor.create_surface(qh);
        let layer = dock.layer_shell.create_layer_surface(
            qh,
            surface,
            // Above the dock's own Top layer, so the grid covers it and the
            // fade at the bottom is what makes the dock reappear.
            Layer::Overlay,
            Some("tremista-launchpad"),
            None,
        );
        layer.set_anchor(Anchor::all());
        // Zero means "whatever the output is", which is what anchoring to all
        // four edges asks for.
        layer.set_size(0, 0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        // -1, not 0: this is not a panel, and it must not push windows around
        // for the moment it is on screen.
        layer.set_exclusive_zone(-1);
        layer.commit();

        // One 1080p frame to start with; the pool grows if the output is bigger.
        let pool = SlotPool::new(1920 * 1080 * 4, &dock.shm)
            .context("creating the Launchpad shm pool")?;

        Ok(Self {
            layer,
            pool,
            scratch: Pixmap::new(1, 1).ok_or_else(|| anyhow!("allocating a pixmap"))?,
            width: 0,
            height: 0,
            page: 0,
            hovered: None,
            press: None,
            scroll: 0.0,
            opened_at: Instant::now(),
            grid: grid::compute(0, 0, 1.0, 1.0, &tremista_dock_core::LaunchpadTheme::default()),
            configured: false,
            frame_pending: false,
            dirty: true,
            region_dirty: true,
        })
    }

    /// Does `surface` belong to this Launchpad?
    pub fn owns(&self, surface: &wayland_client::protocol::wl_surface::WlSurface) -> bool {
        self.layer.wl_surface() == surface
    }

    pub fn configure(&mut self, width: u32, height: u32) {
        if width != 0 && width != self.width {
            self.width = width;
            self.region_dirty = true;
        }
        if height != 0 && height != self.height {
            self.height = height;
            self.region_dirty = true;
        }
        self.configured = true;
        self.dirty = true;
    }

    pub fn frame_done(&mut self) {
        self.frame_pending = false;
    }

    /// The grid's height in surface-local pixels, which is what a context menu
    /// opened over it needs in order to anchor itself above the click.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Take or give back the keyboard.
    ///
    /// Two surfaces cannot both hold it exclusively, so a menu opened over the
    /// grid borrows it and hands it back on close. The grid stays on screen
    /// throughout: "Pin to Dock" would be a strange thing to do to an app whose
    /// icon vanished as the menu appeared.
    pub fn set_keyboard_focus(&mut self, focused: bool) {
        self.layer.set_keyboard_interactivity(if focused {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        });
        self.layer.commit();
    }
}

impl Dock {
    /// Open Launchpad, or close it if it is already open. This is the only
    /// thing the "All Apps" icon does.
    pub fn toggle_launchpad(&mut self, qh: &QueueHandle<Self>) {
        if self.launchpad_view.is_some() {
            self.close_launchpad();
        } else {
            self.open_launchpad(qh);
        }
    }

    fn open_launchpad(&mut self, qh: &QueueHandle<Self>) {
        // Scanning every `.desktop` file takes long enough to be worth doing
        // once, and the set of installed apps does not change under us often
        // enough to justify watching for it.
        if self.all_apps.is_empty() {
            self.all_apps = tremista_dock_core::model::all_apps();
            log::info!("Launchpad: {} app(s)", self.all_apps.len());
        }

        match Launchpad::new(self, qh) {
            Ok(view) => self.launchpad_view = Some(view),
            Err(e) => log::error!("opening Launchpad: {e:#}"),
        }
    }

    pub fn close_launchpad(&mut self) {
        // Dropping the `LayerSurface` destroys it, which is what removes the
        // grid from the screen and hands keyboard focus back.
        self.launchpad_view = None;
    }

    /// Move `delta` pages, clamped rather than wrapped: a wheel that kept
    /// looping around would make it impossible to tell where the end is.
    pub fn page_launchpad(&mut self, delta: isize) {
        let Some(view) = self.launchpad_view.as_mut() else {
            return;
        };
        let last = view.grid.pages.saturating_sub(1);
        let next = (view.page as isize + delta).clamp(0, last as isize) as usize;
        if next != view.page {
            view.page = next;
            view.dirty = true;
        }
    }

    /// A pointer event that landed on the Launchpad surface.
    pub fn launchpad_pointer(&mut self, event: &PointerEvent, qh: &QueueHandle<Self>) {
        let Some(view) = self.launchpad_view.as_mut() else {
            return;
        };
        let (x, y) = (event.position.0 as f32, event.position.1 as f32);

        match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                let hovered = grid::hit_test(&view.grid, x, y);
                if hovered != view.hovered {
                    view.hovered = hovered;
                    view.dirty = true;
                }
            }
            PointerEventKind::Leave { .. } => {
                view.hovered = None;
                view.press = None;
                view.dirty = true;
            }
            PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                view.press = Some(grid::hit_test(&view.grid, x, y));
            }
            PointerEventKind::Press { button, .. } if button == BTN_RIGHT => {
                // Only over an app: right-clicking the empty grid has nothing
                // to offer, and the dock's own settings menu is a right-click
                // on the dock away.
                if let Some(index) = grid::hit_test(&view.grid, x, y) {
                    self.open_launchpad_menu(index, x, y, qh);
                }
            }
            PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                let released = grid::hit_test(&view.grid, x, y);
                let Some(pressed) = view.press.take() else {
                    return;
                };
                // Press and release must agree, so a drag that started on one
                // icon does not launch another -- or the background dismiss the
                // grid out from under a click that began on an app.
                if pressed != released {
                    return;
                }
                match released {
                    Some(index) => self.launch_from_launchpad(index),
                    None => self.close_launchpad(),
                }
            }
            PointerEventKind::Axis {
                horizontal,
                vertical,
                ..
            } => {
                // Horizontal wins when there is any: a trackpad swipe sideways
                // is the gesture that means "next page".
                let delta = if horizontal.absolute != 0.0 {
                    horizontal.absolute
                } else {
                    vertical.absolute
                };
                view.scroll += delta as f32;
                let steps = (view.scroll / PAGE_SCROLL) as isize;
                if steps != 0 {
                    view.scroll -= steps as f32 * PAGE_SCROLL;
                    self.page_launchpad(steps);
                }
            }
            _ => {}
        }
    }

    fn launch_from_launchpad(&mut self, index: usize) {
        let Some(app) = self.all_apps.get(index).cloned() else {
            return;
        };
        // Close first: the app takes a moment to appear, and leaving the grid
        // up in the meantime makes the click feel like it missed.
        self.close_launchpad();

        if app.exec.is_empty() {
            return;
        }
        match self.launcher.spawn(&app.exec) {
            Ok(()) => {
                // Bounces are keyed by app id, so this only shows if the app
                // also happens to be in the dock -- which is the case we want.
                self.bounces.retain(|(id, _)| *id != app.app_id);
                self.bounces.push((app.app_id, Instant::now()));
                self.dirty = true;
            }
            Err(e) => log::error!("{e:#}"),
        }
    }

    pub fn draw_launchpad_if_needed(&mut self, qh: &QueueHandle<Self>) {
        let ready = self.launchpad_view.as_ref().is_some_and(|v| {
            v.configured && !v.frame_pending && v.dirty && v.width != 0 && v.height != 0
        });
        if !ready {
            return;
        }
        if let Err(e) = self.draw_launchpad(qh) {
            log::error!("drawing a Launchpad frame: {e:#}");
            if let Some(view) = self.launchpad_view.as_mut() {
                // Don't spin: a failure repeating every frame would peg a core.
                view.dirty = false;
            }
        }
    }

    fn draw_launchpad(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        let (width, height, page, hovered, elapsed) = {
            let view = self
                .launchpad_view
                .as_ref()
                .ok_or_else(|| anyhow!("Launchpad closed"))?;
            (
                view.width,
                view.height,
                view.page,
                view.hovered,
                view.opened_at.elapsed().as_secs_f32(),
            )
        };

        let duration = self.launchpad_theme.open_duration;
        let progress = if duration <= 0.0 {
            1.0
        } else {
            (elapsed / duration).min(1.0)
        };

        let laid_out = grid::compute(
            self.all_apps.len(),
            page,
            width as f32,
            height as f32,
            &self.launchpad_theme,
        );

        // Only the visible page: rasterising two hundred icons to open a grid
        // that shows thirty-five of them would stall the first frame.
        let names: Vec<String> = laid_out
            .cells
            .iter()
            .filter_map(|cell| self.all_apps.get(cell.index))
            .map(|app| app.icon_name.clone())
            .collect();
        for name in &names {
            self.icons.get(name);
        }

        let scale = self.scale.max(1);
        let width_px = width as i32 * scale;
        let height_px = height as i32 * scale;

        let format = if self.shm.formats().contains(&wl_shm::Format::Abgr8888) {
            wl_shm::Format::Abgr8888
        } else {
            wl_shm::Format::Argb8888
        };

        let view = self
            .launchpad_view
            .as_mut()
            .ok_or_else(|| anyhow!("Launchpad closed"))?;

        if view.scratch.width() != width_px as u32 || view.scratch.height() != height_px as u32 {
            view.scratch = Pixmap::new(width_px as u32, height_px as u32)
                .ok_or_else(|| anyhow!("allocating a {width_px}x{height_px} pixmap"))?;
        }

        {
            // Indexed by position in the full app list, which is what a cell's
            // `index` refers to.
            let items: Vec<GridItem> = self
                .all_apps
                .iter()
                .map(|app| GridItem {
                    name: &app.name,
                    icon: self.icons.peek(&app.icon_name),
                })
                .collect();

            grid::draw(
                &mut view.scratch.as_mut(),
                &laid_out,
                &items,
                &self.launchpad_theme,
                self.font.as_ref(),
                hovered,
                progress,
                scale as f32,
            );
        }

        let (buffer, canvas) = view
            .pool
            .create_buffer(width_px, height_px, width_px * 4, format)
            .context("creating an shm buffer")?;
        copy_pixels(format, view.scratch.data(), canvas);

        if view.region_dirty {
            let region = Region::new(&self.compositor).context("creating an input region")?;
            // Everything except the dock's strip. Leaving that out is what lets
            // the dock stay usable underneath -- including the "All Apps" icon,
            // which is then the natural way to close Launchpad again.
            let reserved = self.launchpad_theme.reserved_bottom.ceil() as i32;
            region.add(0, 0, width as i32, (height as i32 - reserved).max(0));
            view.layer.set_input_region(Some(region.wl_region()));
            view.region_dirty = false;
        }

        let surface = view.layer.wl_surface();
        let _ = view.layer.set_buffer_scale(scale as u32);
        surface.damage_buffer(0, 0, width_px, height_px);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).context("attaching the buffer")?;
        view.layer.commit();

        view.frame_pending = true;
        // Keep drawing while the open animation is still running.
        view.dirty = progress < 1.0;
        view.page = laid_out.page;
        view.grid = laid_out;
        Ok(())
    }
}

/// User data for our own `wl_keyboard`.
///
/// A marker type for the same reason as [`crate::toplevels::ToplevelData`]: the
/// blanket `delegate_dispatch2!` impl already covers `()`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardData;

impl Dispatch<wl_keyboard::WlKeyboard, KeyboardData> for Dock {
    fn event(
        state: &mut Self,
        _keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &KeyboardData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The dock takes no keyboard focus of its own; every key we see here
        // arrived because Launchpad or a context menu is open and asked for it.
        let wl_keyboard::Event::Key {
            key, state: pressed, ..
        } = event
        else {
            return;
        };
        if !matches!(pressed.into_result(), Ok(wl_keyboard::KeyState::Pressed)) {
            return;
        }

        // A menu opened over the grid is the innermost thing on screen, so
        // Escape dismisses it first and leaves the grid where it was.
        if state.menu_is_open() {
            if key == KEY_ESC {
                state.close_menu();
            }
            return;
        }

        match key {
            KEY_ESC => state.close_launchpad(),
            KEY_LEFT => state.page_launchpad(-1),
            KEY_RIGHT => state.page_launchpad(1),
            _ => {}
        }
    }
}
