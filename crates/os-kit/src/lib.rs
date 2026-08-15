//! Windows platform layer: input hooking, synthetic input, screen geometry.
//!
//! Everything Win32 lives behind this crate so `mouseless-core` can stay pure
//! and `mouseless-app` can stay readable. When a second platform arrives, this
//! is the crate that grows a `#[cfg]`.

#![cfg(windows)]

pub mod hook;
pub mod input;
pub mod keycode;
pub mod screen;

pub use hook::{Hook, HookError, HookEvent, Hotkey};
pub use screen::{enable_dpi_awareness, virtual_screen};
