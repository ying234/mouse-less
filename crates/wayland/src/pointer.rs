//! Synthetic cursor movement and clicks, via `zwlr_virtual_pointer_v1`.
//!
//! Wayland gives a client no way to warp the real cursor, so we register a
//! virtual input device and drive it instead. Two things follow from that:
//!
//!   * it needs no elevated privileges and no `uinput` access — unlike the
//!     `/dev/input` route, which needs the user in the `input` group;
//!   * events go through the compositor's normal input path, so focus follows
//!     the click exactly as it would for a real mouse.

use std::time::Instant;

use mouseless_core::{Button, Point, Rect};
use smithay_client_toolkit::reexports::client::protocol::wl_pointer;
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

/// Button codes from `linux/input-event-codes.h`, which is what the protocol
/// specifies here.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

const fn code(button: Button) -> u32 {
    match button {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
    }
}

pub struct VirtualPointer {
    pointer: ZwlrVirtualPointerV1,
    epoch: Instant,
    /// Buttons we are holding down, so they can be released on the way out.
    ///
    /// A synthetic press whose release never arrives leaves the button stuck
    /// for the whole session, and the user cannot let go of a button they
    /// never pressed.
    held: Vec<Button>,
}

impl VirtualPointer {
    pub fn new(pointer: ZwlrVirtualPointerV1) -> Self {
        Self {
            pointer,
            epoch: Instant::now(),
            held: Vec::new(),
        }
    }

    /// Milliseconds since this pointer was created.
    ///
    /// The protocol wants a timestamp with millisecond granularity that only
    /// moves forward; it is not required to share the compositor's clock.
    fn time(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }

    /// Warp the cursor to a point in layout coordinates.
    ///
    /// `motion_absolute` addresses the whole output layout as a fraction of
    /// `extent`, so the layout's own origin has to be subtracted first: on a
    /// multi-monitor setup that origin is not necessarily (0, 0).
    pub fn move_cursor(&self, bounds: Rect, p: Point) {
        if bounds.is_empty() {
            return;
        }
        let p = bounds.clamp(p);
        self.pointer.motion_absolute(
            self.time(),
            (p.x - bounds.x) as u32,
            (p.y - bounds.y) as u32,
            bounds.w as u32,
            bounds.h as u32,
        );
        self.pointer.frame();
    }

    pub fn click(&mut self, button: Button) {
        self.press(button, wl_pointer::ButtonState::Pressed);
        self.press(button, wl_pointer::ButtonState::Released);
    }

    pub fn down(&mut self, button: Button) {
        if !self.held.contains(&button) {
            self.held.push(button);
        }
        self.press(button, wl_pointer::ButtonState::Pressed);
    }

    pub fn up(&mut self, button: Button) {
        self.held.retain(|b| *b != button);
        self.press(button, wl_pointer::ButtonState::Released);
    }

    /// Release every button we are holding. Safe to call when holding none.
    pub fn release_all_held(&mut self) {
        for button in std::mem::take(&mut self.held) {
            self.press(button, wl_pointer::ButtonState::Released);
        }
    }

    fn press(&self, button: Button, state: wl_pointer::ButtonState) {
        self.pointer.button(self.time(), code(button), state);
        self.pointer.frame();
    }
}

impl Drop for VirtualPointer {
    fn drop(&mut self) {
        self.release_all_held();
        self.pointer.destroy();
    }
}
