//! Layout, theming and rendering for the Tremista dock.
//!
//! Deliberately free of any Wayland dependency: everything here is pure
//! computation over a `tiny_skia::Pixmap`, so it builds and is testable on any
//! platform, and the Wayland shell in `tremista-dock` stays a thin transport
//! layer over it. `cargo run -p tremista-dock-core --example preview` renders
//! a frame to PNG for iterating on the look without a compositor.

pub mod icons;
pub mod launchpad;
pub mod layout;
pub mod model;
pub mod render;
pub mod text;
pub mod theme;

pub use icons::IconCache;
pub use launchpad::{GridItem, LaunchpadTheme};
pub use layout::{bounce_offset, compute as compute_layout, hit_test, ItemGeometry, Layout};
pub use model::{matches_app_id, DockItem};
pub use render::{draw, RenderItem};
pub use text::Font;
pub use theme::Theme;
