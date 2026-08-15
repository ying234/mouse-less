//! Screen geometry and DPI.

use mouseless_core::Rect;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Opt into per-monitor DPI awareness v2.
///
/// Must run before any window is created. Without it Windows lies to us about
/// screen coordinates on mixed-DPI setups: the overlay would be drawn scaled
/// and the cursor would land somewhere other than the cell the user picked.
///
/// Returns `false` if the process already had an awareness context set (which
/// an application manifest would do), and that is not an error.
pub fn enable_dpi_awareness() -> bool {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() }
}

/// The bounding box of every monitor, in virtual-screen coordinates.
///
/// The origin is the top-left of the *primary* monitor, so `x`/`y` are
/// negative when a monitor sits left of or above it.
pub fn virtual_screen() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}
