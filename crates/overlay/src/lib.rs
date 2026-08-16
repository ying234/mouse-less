//! The grid overlay: a software renderer, plus the platform surface it lands on.
//!
//! [`canvas`] and [`render`] are pure pixel-pushing and build on any platform —
//! they are shared by the Windows layered window and the Wayland layer surface.
//! Only [`window`] is Win32.

pub mod canvas;
pub mod render;
#[cfg(windows)]
pub mod window;

pub use canvas::{Canvas, Rgba};
pub use render::{RenderOptions, Renderer, DEFAULT_LABEL_FONT_MAX_PX};
#[cfg(windows)]
pub use window::{Overlay, OverlayError};
