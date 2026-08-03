//! Tracking open windows via `zwlr_foreign_toplevel_management_v1`.
//!
//! This is what turns the dock from a launcher into a dock: it is how we learn
//! which apps are running, which one is focused, and how we focus or minimise
//! them on click.

use crate::Dock;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

/// One window the compositor has told us about.
pub struct Toplevel {
    pub handle: ZwlrForeignToplevelHandleV1,
    pub app_id: String,
    pub title: String,
    pub minimized: bool,
    pub activated: bool,
}

impl Toplevel {
    fn new(handle: ZwlrForeignToplevelHandleV1) -> Self {
        Self {
            handle,
            app_id: String::new(),
            title: String::new(),
            minimized: false,
            activated: false,
        }
    }
}

/// Decode the `state` event's array of `u32`s.
///
/// The event carries the toplevel's *complete* state each time, so the flags
/// are replaced wholesale rather than merged.
fn decode_state(bytes: &[u8]) -> (bool, bool) {
    use zwlr_foreign_toplevel_handle_v1::State;
    let (mut minimized, mut activated) = (false, false);
    for chunk in bytes.chunks_exact(4) {
        let value = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if value == State::Minimized as u32 {
            minimized = true;
        } else if value == State::Activated as u32 {
            activated = true;
        }
    }
    (minimized, activated)
}

impl Dock {
    fn toplevel_mut(&mut self, handle: &ZwlrForeignToplevelHandleV1) -> Option<&mut Toplevel> {
        self.toplevels.iter_mut().find(|t| &t.handle == handle)
    }

    /// Focus an app, or minimise it if it is already focused -- the way
    /// clicking a running app's dock icon behaves on macOS.
    ///
    /// Windows of the same app are cycled rather than raised all at once,
    /// because a dock icon maps to an app while the protocol only talks about
    /// individual windows.
    pub fn activate_or_minimize(&mut self, app_id: &str) {
        use tremista_dock_core::matches_app_id;

        let Some(seat) = self.seat.clone() else {
            log::warn!("no seat; cannot focus {app_id}");
            return;
        };

        let matching: Vec<usize> = self
            .toplevels
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                // Reuse the dock item matcher so focusing agrees with the
                // running-indicator logic exactly.
                let probe = tremista_dock_core::DockItem {
                    app_id: t.app_id.clone(),
                    name: String::new(),
                    exec: String::new(),
                    icon_name: String::new(),
                    running: true,
                    pinned: false,
                };
                matches_app_id(&probe, app_id)
            })
            .map(|(i, _)| i)
            .collect();

        let Some(&first) = matching.first() else {
            return;
        };

        // If one of this app's windows already has focus, the click means
        // "get out of the way"; otherwise it means "bring me forward".
        let focused = matching.iter().find(|&&i| self.toplevels[i].activated);
        match focused {
            Some(&i) if matching.len() == 1 => self.toplevels[i].handle.set_minimized(),
            Some(&i) => {
                // Cycle to this app's next window.
                let pos = matching.iter().position(|&m| m == i).unwrap_or(0);
                let next = matching[(pos + 1) % matching.len()];
                self.toplevels[next].handle.unset_minimized();
                self.toplevels[next].handle.activate(&seat);
            }
            None => {
                self.toplevels[first].handle.unset_minimized();
                self.toplevels[first].handle.activate(&seat);
            }
        }
    }
}

/// User data for the foreign-toplevel objects.
///
/// A marker type rather than `()`: sctk's blanket `delegate_dispatch2!` impl
/// covers `Dispatch<I, U>` for every `U` that implements its `Dispatch2`, and a
/// hand-written impl on a std type like `()` would overlap it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ToplevelData;

impl Dispatch<ZwlrForeignToplevelManagerV1, ToplevelData> for Dock {
    fn event(
        state: &mut Self,
        _manager: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &ToplevelData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                state.toplevels.push(Toplevel::new(toplevel));
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                // The compositor has stopped sending updates; anything we still
                // hold is now stale.
                state.toplevels.clear();
                state.rebuild_items();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(Dock, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ToplevelData),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ToplevelData> for Dock {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &ToplevelData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;

        match event {
            Event::AppId { app_id } => {
                if let Some(t) = state.toplevel_mut(handle) {
                    t.app_id = app_id;
                }
            }
            Event::Title { title } => {
                if let Some(t) = state.toplevel_mut(handle) {
                    t.title = title;
                }
            }
            Event::State { state: flags } => {
                let (minimized, activated) = decode_state(&flags);
                if let Some(t) = state.toplevel_mut(handle) {
                    t.minimized = minimized;
                    t.activated = activated;
                }
            }
            // Every burst of the events above is terminated by `done`, which is
            // the only point at which the toplevel's properties are coherent.
            Event::Done => state.rebuild_items(),
            Event::Closed => {
                handle.destroy();
                state.toplevels.retain(|t| &t.handle != handle);
                state.rebuild_items();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zwlr_foreign_toplevel_handle_v1::State;

    fn encode(states: &[State]) -> Vec<u8> {
        states
            .iter()
            .flat_map(|s| (*s as u32).to_ne_bytes())
            .collect()
    }

    #[test]
    fn state_flags_decode() {
        assert_eq!(decode_state(&encode(&[])), (false, false));
        assert_eq!(decode_state(&encode(&[State::Minimized])), (true, false));
        assert_eq!(decode_state(&encode(&[State::Activated])), (false, true));
        assert_eq!(
            decode_state(&encode(&[State::Maximized, State::Activated])),
            (false, true)
        );
    }

    #[test]
    fn unknown_and_truncated_states_are_ignored() {
        // A compositor may send states from a newer protocol version, and a
        // trailing partial word must not panic.
        let mut bytes = encode(&[State::Activated]);
        bytes.extend_from_slice(&99u32.to_ne_bytes());
        bytes.push(0);
        assert_eq!(decode_state(&bytes), (false, true));
    }
}
