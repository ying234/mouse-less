//! The one place that knows which operating system this is.
//!
//! Both backends expose the same three things — an [`Event`] stream, a
//! `Platform` that performs the engine's actions, and a `start` that wires
//! them up — so `main` holds the decision loop once rather than twice. The
//! loop is where the behaviour lives, and behaviour that exists in two copies
//! drifts into two behaviours.

#[cfg(unix)]
mod linux;
#[cfg(unix)]
pub use linux::{start, Events, Platform};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{start, Events, Platform};

/// Something the engine loop needs to react to.
///
/// Each platform produces the subset it can observe; the rest simply never
/// arrive. The loop handles all of them either way, so that gaining one on a
/// platform later is a change in that backend and nowhere else.
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The trigger fired: the hotkey on Windows, the socket on Wayland.
    Trigger,
    Key(mouseless_core::KeyPress),
    /// The overlay lost the keyboard while it was up. Wayland only.
    FocusLost,
    /// Monitors were rearranged. Wayland only.
    LayoutChanged(mouseless_core::Rect),
    /// Shut down cleanly.
    Quit,
}
