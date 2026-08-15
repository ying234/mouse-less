//! The click-through grid overlay.

#![cfg(windows)]

pub mod canvas;
pub mod render;
pub mod window;

pub use canvas::{Canvas, Rgba};
pub use render::{RenderOptions, Renderer, DEFAULT_LABEL_FONT_MAX_PX};
pub use window::{Overlay, OverlayError};
