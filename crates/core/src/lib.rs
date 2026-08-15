//! Platform-independent heart of my-mouseless.
//!
//! Everything here is pure: no Win32 types, no I/O, no threads. The host
//! (`mouseless-app`) feeds it [`Input`] values and performs the [`Action`]
//! values it returns. That split is what keeps the interesting behaviour
//! testable on any machine.

pub mod config;
pub mod engine;
pub mod geom;
pub mod key;
pub mod label;

pub use config::{ConfigError, GridConfig};
pub use engine::{Action, Button, Engine, Input, LabeledCell};
pub use geom::{Point, Rect};
pub use key::{Key, KeyPress, Mods};
