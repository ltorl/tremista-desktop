//! The dock's right-click menus.
//!
//! Built the same way as Launchpad -- a transient layer-shell surface owned by
//! the dock, created on demand and destroyed on dismiss -- for the same reason:
//! a menu is not an application and must never be mistaken for a window.
//!
//! The surface covers the whole output rather than just the menu's own box, so
//! that a click anywhere outside the menu dismisses it. That is what a menu is
//! expected to do, and a small surface could not see those clicks at all.
//!
//! Three menus share this one implementation, differing only in their rows:
//! right-clicking the dock's background offers the dock's own settings,
//! right-clicking an icon offers that app, and right-clicking an app in
//! Launchpad offers a shorter version of the same.

use crate::{copy_pixels, settings::Settings, Dock};
use anyhow::{anyhow, Context, Result};
use smithay_client_toolkit::{
    compositor::FrameCallbackData,
    seat::pointer::{PointerEvent, PointerEventKind, BTN_LEFT, BTN_RIGHT},
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface},
        WaylandSurface,
    },
    shm::slot::SlotPool,
};
use tiny_skia::Pixmap;
use tremista_dock_core::{menu as widget, DockItem};
use wayland_client::{protocol::wl_shm, QueueHandle};

/// What choosing a row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleMagnification,
    ToggleHiding,
    /// Launch the target's `Exec` again, whether or not it is already running.
    NewWindow,
    Pin,
    Unpin,
    /// Politely ask every window of the target to close. The app decides what
    /// that means -- it may put up a "save your work?" dialog, and should.
    Quit,
}

/// The rows of the dock-background menu, in order.
fn dock_actions() -> Vec<Action> {
    vec![Action::ToggleMagnification, Action::ToggleHiding]
}

/// The rows offered for `item`, which vary with what is true of it: there is no
/// Quit for an app that is not running, and nothing to unpin if it is not
/// pinned. `in_launchpad` drops the entries that only make sense in the dock.
fn app_actions(item: &DockItem, in_launchpad: bool) -> Vec<Action> {
    let mut actions = Vec::new();
    // Unpinned running apps carry no `Exec` -- there is nothing to launch.
    if !item.exec.is_empty() {
        actions.push(Action::NewWindow);
    }
    actions.push(if item.pinned {
        Action::Unpin
    } else {
        Action::Pin
    });
    // Quitting from Launchpad is not offered: the grid is about starting things,
    // and the app's own window is right there behind it to close instead.
    if item.running && !in_launchpad {
        actions.push(Action::Quit);
    }
    actions
}

/// The label for a row. Dock settings name their *effect*, the way macOS words
/// its dock menu -- "Turn Hiding On" when hiding is currently off.
fn label(action: Action, settings: Settings) -> String {
    let word = |enabled: bool| if enabled { "Off" } else { "On" };
    match action {
        Action::ToggleMagnification => {
            format!("Turn Magnification {}", word(settings.magnification))
        }
        Action::ToggleHiding => format!("Turn Hiding {}", word(settings.hiding)),
        Action::NewWindow => "New Window".to_owned(),
        Action::Pin => "Pin to Dock".to_owned(),
        Action::Unpin => "Unpin".to_owned(),
        Action::Quit => "Quit".to_owned(),
    }
}

/// The open menu. Dropping it destroys the surface, which closes the menu.
pub struct MenuView {
    pub layer: LayerSurface,
    pool: SlotPool,
    scratch: Pixmap,

    width: u32,
    height: u32,

    actions: Vec<Action>,
    labels: Vec<String>,
    /// The app the rows act on, if this is an app menu.
    target: Option<DockItem>,
    /// Where the click that opened the menu was, in output coordinates.
    anchor_x: f32,
    /// Distance from the bottom of the output up to the menu's bottom edge.
    anchor_above_bottom: f32,

    menu: widget::Menu,
    hovered: Option<usize>,
    press: Option<Option<usize>>,

    configured: bool,
    frame_pending: bool,
    dirty: bool,
}

impl MenuView {
    fn new(
        dock: &Dock,
        actions: Vec<Action>,
        target: Option<DockItem>,
        anchor_x: f32,
        anchor_above_bottom: f32,
        qh: &QueueHandle<Dock>,
    ) -> Result<Self> {
        let surface = dock.compositor.create_surface(qh);
        let layer = dock.layer_shell.create_layer_surface(
            qh,
            surface,
            // Above the dock so the menu is not clipped by it, and above
            // Launchpad so it can be used over the grid.
            Layer::Overlay,
            Some("tremista-dock-menu"),
            None,
        );
        layer.set_anchor(Anchor::all());
        layer.set_size(0, 0);
        // Exclusive so Escape reaches us rather than whatever is behind.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_exclusive_zone(-1);
        layer.commit();

        let pool =
            SlotPool::new(1920 * 1080 * 4, &dock.shm).context("creating the menu shm pool")?;

        let labels = actions
            .iter()
            .map(|action| label(*action, dock.settings))
            .collect();

        Ok(Self {
            layer,
            pool,
            scratch: Pixmap::new(1, 1).ok_or_else(|| anyhow!("allocating a pixmap"))?,
            width: 0,
            height: 0,
            actions,
            labels,
            target,
            anchor_x,
            anchor_above_bottom,
            menu: widget::compute(&[], 0.0, 0.0, (1.0, 1.0), None, &dock.menu_theme),
            hovered: None,
            press: None,
            configured: false,
            frame_pending: false,
            dirty: true,
        })
    }

    pub fn owns(&self, surface: &wayland_client::protocol::wl_surface::WlSurface) -> bool {
        self.layer.wl_surface() == surface
    }

    pub fn configure(&mut self, width: u32, height: u32) {
        if width != 0 {
            self.width = width;
        }
        if height != 0 {
            self.height = height;
        }
        self.configured = true;
        self.dirty = true;
    }

    pub fn frame_done(&mut self) {
        self.frame_pending = false;
    }
}

impl Dock {
    /// Right-click on the dock, at `x` in the dock's (output-wide) surface.
    /// Which menu opens depends on whether an icon was hit.
    pub fn open_dock_menu(&mut self, x: f32, y: f32, qh: &QueueHandle<Self>) {
        let hit = tremista_dock_core::hit_test(&self.layout, x, y);
        let item = hit.and_then(|index| self.visible.get(index)).cloned();

        // "All Apps" is not an app: it has nothing to launch, pin or quit, so
        // right-clicking it gives the dock's own menu like the background does.
        let item = item.filter(|i| i.app_id != tremista_dock_core::model::LAUNCHPAD_APP_ID);

        let (actions, target) = match item {
            Some(item) => (app_actions(&item, false), Some(item)),
            None => (dock_actions(), None),
        };
        // Both menus sit on top of the dock plate.
        let above = self.theme.background_height() + self.theme.margin_bottom;
        self.open_menu(actions, target, x, above, qh);
    }

    /// Right-click on an app in Launchpad. `y` is where the click landed, so the
    /// menu opens just above the icon rather than down by the dock.
    pub fn open_launchpad_menu(&mut self, index: usize, x: f32, y: f32, qh: &QueueHandle<Self>) {
        let Some(item) = self.all_apps.get(index).cloned() else {
            return;
        };
        // Already in the dock? Then offer to take it out again, not to add it
        // twice -- `app_actions` reads that from the item, so mark it first.
        let mut item = item;
        item.pinned = self.pinned.iter().any(|p| p.app_id == item.app_id);
        let actions = app_actions(&item, true);

        let above = self
            .launchpad_view
            .as_ref()
            .map_or(0.0, |view| view.height() as f32 - y);
        self.open_menu(actions, Some(item), x, above, qh);
    }

    fn open_menu(
        &mut self,
        actions: Vec<Action>,
        target: Option<DockItem>,
        x: f32,
        above_bottom: f32,
        qh: &QueueHandle<Self>,
    ) {
        if actions.is_empty() {
            return;
        }
        // Two surfaces asking for exclusive keyboard focus is a fight nobody
        // wins, so Launchpad gives its up for as long as the menu is there.
        if let Some(view) = self.launchpad_view.as_mut() {
            view.set_keyboard_focus(false);
        }

        match MenuView::new(self, actions, target, x, above_bottom, qh) {
            Ok(view) => self.menu_view = Some(view),
            Err(e) => log::error!("opening the dock menu: {e:#}"),
        }
    }

    pub fn close_menu(&mut self) {
        if self.menu_view.take().is_some() {
            if let Some(view) = self.launchpad_view.as_mut() {
                view.set_keyboard_focus(true);
            }
            // The dock stays revealed while the menu is up; once it goes, the
            // usual hover rules apply again.
            self.dirty = true;
        }
    }

    pub fn menu_is_open(&self) -> bool {
        self.menu_view.is_some()
    }

    fn choose(&mut self, row: usize) {
        let Some(view) = self.menu_view.as_ref() else {
            return;
        };
        let Some(action) = view.actions.get(row).copied() else {
            return;
        };
        let target = view.target.clone();
        self.close_menu();

        match action {
            Action::ToggleMagnification => {
                self.settings.magnification = !self.settings.magnification;
                self.settings.save();
            }
            Action::ToggleHiding => {
                self.settings.hiding = !self.settings.hiding;
                self.apply_hiding();
                self.settings.save();
            }
            Action::NewWindow => {
                if let Some(item) = target {
                    self.launch(&item);
                }
            }
            Action::Pin => {
                if let Some(item) = target {
                    self.pin(item);
                }
            }
            Action::Unpin => {
                if let Some(item) = target {
                    self.unpin(&item.app_id);
                }
            }
            Action::Quit => {
                if let Some(item) = target {
                    self.quit(&item);
                }
            }
        }
        self.dirty = true;
    }

    /// Add an app to the end of the pinned list and save it.
    fn pin(&mut self, item: DockItem) {
        if self.pinned.iter().any(|p| p.app_id == item.app_id) {
            return;
        }
        let mut item = item;
        item.pinned = true;
        item.running = false;
        self.pinned.push(item);
        self.save_pins();
        self.rebuild_items();
    }

    fn unpin(&mut self, app_id: &str) {
        let before = self.pinned.len();
        self.pinned.retain(|p| p.app_id != app_id);
        if self.pinned.len() != before {
            self.save_pins();
            // A running app that was just unpinned does not disappear -- it
            // comes straight back as an unpinned running entry.
            self.rebuild_items();
        }
    }

    fn save_pins(&self) {
        let ids: Vec<String> = self.pinned.iter().map(|p| p.app_id.clone()).collect();
        if let Err(e) = crate::config::write_pinned(&ids) {
            log::error!("saving the pinned apps: {e}");
        }
    }

    /// Ask every window of `item` to close. This is a request, not a kill: an
    /// app with unsaved work is entitled to put up a dialog and stay open.
    fn quit(&mut self, item: &DockItem) {
        for toplevel in &self.toplevels {
            if !toplevel.app_id.is_empty()
                && tremista_dock_core::matches_app_id(item, &toplevel.app_id)
            {
                toplevel.handle.close();
            }
        }
    }

    /// A pointer event that landed on the menu surface.
    pub fn menu_pointer(&mut self, event: &PointerEvent) {
        let Some(view) = self.menu_view.as_mut() else {
            return;
        };
        let (x, y) = (event.position.0 as f32, event.position.1 as f32);

        match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                let hovered = widget::hit_test(&view.menu, x, y);
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
            // The right button counts too: press-drag-release from the same
            // right-click that opened the menu is how menus work on a trackpad.
            PointerEventKind::Press { button, .. } if button == BTN_LEFT || button == BTN_RIGHT => {
                view.press = Some(widget::hit_test(&view.menu, x, y));
            }
            PointerEventKind::Release { button, .. }
                if button == BTN_LEFT || button == BTN_RIGHT =>
            {
                let released = widget::hit_test(&view.menu, x, y);
                match (view.press.take(), released) {
                    (_, Some(row)) => self.choose(row),
                    // A click that began and ended on the background dismisses.
                    (Some(None), None) => self.close_menu(),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn draw_menu_if_needed(&mut self, qh: &QueueHandle<Self>) {
        let ready = self
            .menu_view
            .as_ref()
            .is_some_and(|v| v.configured && !v.frame_pending && v.dirty && v.width != 0);
        if !ready {
            return;
        }
        if let Err(e) = self.draw_menu(qh) {
            log::error!("drawing the dock menu: {e:#}");
            if let Some(view) = self.menu_view.as_mut() {
                // Don't spin: a failure repeating every frame would peg a core.
                view.dirty = false;
            }
        }
    }

    fn draw_menu(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        let scale = self.scale.max(1);
        let format = if self.shm.formats().contains(&wl_shm::Format::Abgr8888) {
            wl_shm::Format::Abgr8888
        } else {
            wl_shm::Format::Argb8888
        };

        let theme = &self.menu_theme;
        let font = self.font.as_ref();
        let view = self
            .menu_view
            .as_mut()
            .ok_or_else(|| anyhow!("the menu closed"))?;

        let (width, height) = (view.width, view.height);
        let width_px = width as i32 * scale;
        let height_px = height as i32 * scale;

        view.menu = widget::compute(
            &view.labels,
            view.anchor_x,
            height as f32 - view.anchor_above_bottom,
            (width as f32, height as f32),
            font,
            theme,
        );

        if view.scratch.width() != width_px as u32 || view.scratch.height() != height_px as u32 {
            view.scratch = Pixmap::new(width_px as u32, height_px as u32)
                .ok_or_else(|| anyhow!("allocating a {width_px}x{height_px} pixmap"))?;
        }
        // The surface is output-sized but almost entirely transparent, and the
        // menu moves, so the previous frame has to be cleared rather than drawn
        // over.
        view.scratch.fill(tiny_skia::Color::TRANSPARENT);

        widget::draw(
            &mut view.scratch.as_mut(),
            &view.menu,
            &view.labels,
            theme,
            font,
            view.hovered,
            scale as f32,
        );

        let (buffer, canvas) = view
            .pool
            .create_buffer(width_px, height_px, width_px * 4, format)
            .context("creating an shm buffer")?;
        copy_pixels(format, view.scratch.data(), canvas);

        let surface = view.layer.wl_surface();
        let _ = view.layer.set_buffer_scale(scale as u32);
        surface.damage_buffer(0, 0, width_px, height_px);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).context("attaching the buffer")?;
        view.layer.commit();

        view.frame_pending = true;
        view.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(pinned: bool, running: bool, exec: &str) -> DockItem {
        DockItem {
            app_id: "org.example.App".to_owned(),
            name: "App".to_owned(),
            exec: exec.to_owned(),
            icon_name: "app".to_owned(),
            running,
            pinned,
        }
    }

    #[test]
    fn dock_settings_name_what_choosing_them_will_do() {
        let on = Settings {
            magnification: true,
            hiding: true,
        };
        assert_eq!(label(Action::ToggleMagnification, on), "Turn Magnification Off");
        assert_eq!(label(Action::ToggleHiding, on), "Turn Hiding Off");

        let off = Settings {
            magnification: false,
            hiding: false,
        };
        assert_eq!(label(Action::ToggleMagnification, off), "Turn Magnification On");
        assert_eq!(label(Action::ToggleHiding, off), "Turn Hiding On");
    }

    #[test]
    fn a_pinned_running_app_offers_the_full_dock_menu() {
        assert_eq!(
            app_actions(&app(true, true, "app"), false),
            [Action::NewWindow, Action::Unpin, Action::Quit]
        );
    }

    #[test]
    fn quit_is_only_offered_for_something_that_is_running() {
        let actions = app_actions(&app(true, false, "app"), false);
        assert!(!actions.contains(&Action::Quit));
    }

    #[test]
    fn an_unpinned_app_is_offered_a_pin_instead_of_an_unpin() {
        let actions = app_actions(&app(false, true, "app"), false);
        assert!(actions.contains(&Action::Pin));
        assert!(!actions.contains(&Action::Unpin));
    }

    #[test]
    fn an_entry_with_no_exec_is_not_offered_a_new_window() {
        // Running-but-unpinned entries are built without an `Exec`.
        let actions = app_actions(&app(false, true, ""), false);
        assert!(!actions.contains(&Action::NewWindow));
    }

    #[test]
    fn launchpad_offers_new_window_and_a_pin_but_never_a_quit() {
        assert_eq!(
            app_actions(&app(false, false, "app"), true),
            [Action::NewWindow, Action::Pin]
        );
        // Already pinned, and running: still no Quit from the grid.
        assert_eq!(
            app_actions(&app(true, true, "app"), true),
            [Action::NewWindow, Action::Unpin]
        );
    }

    #[test]
    fn every_action_has_a_label() {
        for action in [
            Action::ToggleMagnification,
            Action::ToggleHiding,
            Action::NewWindow,
            Action::Pin,
            Action::Unpin,
            Action::Quit,
        ] {
            assert!(!label(action, Settings::default()).is_empty());
        }
    }
}
