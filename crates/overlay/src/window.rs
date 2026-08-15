//! The layered, click-through overlay window.
//!
//! The window is created on its own thread and never leaves it: GDI objects
//! and the message pump both belong to the creating thread. Other threads talk
//! to it by pushing a command and posting a wake-up message.

use std::cell::RefCell;
use std::ffi::c_void;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use mouseless_core::{LabeledCell, Point, Rect};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage,
    UpdateLayeredWindow, HWND_TOPMOST, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNA, ULW_ALPHA, WM_APP, WM_DESTROY, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::canvas::Canvas;
use crate::render::{RenderOptions, Renderer};

/// Wake-up message telling the window thread to drain its command queue.
const WM_OVERLAY_CMD: u32 = WM_APP + 1;

/// What a single repaint should draw.
enum Frame {
    Cells {
        cells: Vec<LabeledCell>,
        typed: String,
    },
    CursorHint { pos: Point, dragging: bool },
}

enum Cmd {
    Draw(Frame),
    Hide,
    Quit,
}

/// Handle to the overlay, usable from any thread.
pub struct Overlay {
    tx: Sender<Cmd>,
    hwnd: isize,
    bounds: Rect,
}

#[derive(Debug)]
pub enum OverlayError {
    NoScreen,
    Win32(windows::core::Error),
    Font(crate::render::FontError),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverlayError::NoScreen => write!(f, "virtual screen has zero area"),
            OverlayError::Win32(e) => write!(f, "overlay window creation failed: {e}"),
            OverlayError::Font(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OverlayError {}

impl Overlay {
    /// Create the overlay covering `bounds` (normally the virtual screen).
    pub fn start(bounds: Rect, options: RenderOptions) -> Result<Self, OverlayError> {
        if bounds.is_empty() {
            return Err(OverlayError::NoScreen);
        }
        let renderer = Renderer::new(options).map_err(OverlayError::Font)?;

        let (tx, rx) = unbounded::<Cmd>();
        let (ready_tx, ready_rx) = bounded::<Result<isize, windows::core::Error>>(1);

        std::thread::Builder::new()
            .name("mouseless-overlay".into())
            .spawn(move || match unsafe { create_window(bounds) } {
                Ok(raw) => {
                    let hwnd = raw.hwnd.0 as isize;
                    STATE.with(|s| {
                        *s.borrow_mut() = Some(WindowState {
                            hwnd: raw.hwnd,
                            dc_mem: raw.dc_mem,
                            bitmap: raw.bitmap,
                            old_bitmap: raw.old_bitmap,
                            bits: raw.bits,
                            bounds: raw.bounds,
                            renderer,
                            rx,
                        })
                    });
                    let _ = ready_tx.send(Ok(hwnd));
                    unsafe { pump() };
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .expect("failed to spawn overlay thread");

        match ready_rx.recv() {
            Ok(Ok(hwnd)) => Ok(Self { tx, hwnd, bounds }),
            Ok(Err(e)) => Err(OverlayError::Win32(e)),
            Err(_) => Err(OverlayError::NoScreen),
        }
    }

    /// Virtual-screen region the overlay covers.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The overlay's window handle, for diagnostics.
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub fn show(&self, cells: Vec<LabeledCell>, typed: String) {
        self.send(Cmd::Draw(Frame::Cells { cells, typed }));
    }

    /// Replace the grid with the cursor-mode crosshair and key hints.
    pub fn show_cursor_hint(&self, pos: Point, dragging: bool) {
        self.send(Cmd::Draw(Frame::CursorHint { pos, dragging }));
    }

    pub fn hide(&self) {
        self.send(Cmd::Hide);
    }

    pub fn stop(&self) {
        self.send(Cmd::Quit);
    }

    fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_ok() {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.hwnd as *mut c_void)),
                    WM_OVERLAY_CMD,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}

struct WindowState {
    hwnd: HWND,
    dc_mem: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    bits: *mut u8,
    bounds: Rect,
    renderer: Renderer,
    rx: Receiver<Cmd>,
}

/// Partially built state, before the renderer and receiver are attached.
struct RawWindow {
    hwnd: HWND,
    dc_mem: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    bits: *mut u8,
    bounds: Rect,
}

thread_local! {
    static STATE: RefCell<Option<WindowState>> = const { RefCell::new(None) };
}

unsafe fn create_window(bounds: Rect) -> Result<RawWindow, windows::core::Error> {
    let hinstance = GetModuleHandleW(None)?;
    let class_name = w!("MyMouselessOverlay");

    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    // A zero return usually means "already registered", which is fine on a
    // restart of the overlay thread; CreateWindowExW will tell us for real.
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        // LAYERED  : per-pixel alpha via UpdateLayeredWindow
        // TRANSPARENT + NOACTIVATE : clicks and focus pass straight through
        // TOOLWINDOW : keeps it out of Alt+Tab
        WS_EX_LAYERED
            | WS_EX_TRANSPARENT
            | WS_EX_TOOLWINDOW
            | WS_EX_NOACTIVATE
            | WS_EX_TOPMOST,
        class_name,
        w!("my-mouseless overlay"),
        WS_POPUP,
        bounds.x,
        bounds.y,
        bounds.w,
        bounds.h,
        None,
        None,
        Some(hinstance.into()),
        None,
    )?;

    // Top-down DIB (negative height) so row 0 is the top scanline, matching
    // the canvas's indexing.
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bounds.w,
            biHeight: -bounds.h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let screen_dc = GetDC(None);
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(Some(screen_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
    ReleaseDC(None, screen_dc);

    let dc_mem = CreateCompatibleDC(None);
    let old_bitmap = SelectObject(dc_mem, bitmap.into());

    Ok(RawWindow {
        hwnd,
        dc_mem,
        bitmap,
        old_bitmap,
        bits: bits as *mut u8,
        bounds,
    })
}

unsafe fn pump() {
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().take() {
            SelectObject(st.dc_mem, st.old_bitmap);
            let _ = DeleteObject(st.bitmap.into());
            let _ = DeleteDC(st.dc_mem);
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_OVERLAY_CMD => {
            drain_commands();
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn drain_commands() {
    // Collect first so the state borrow is not held across rendering.
    let cmds: Vec<Cmd> = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|st| st.rx.try_iter().collect())
            .unwrap_or_default()
    });

    for cmd in cmds {
        match cmd {
            Cmd::Draw(frame) => STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    paint(st, &frame);
                }
            }),
            Cmd::Hide => STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    let _ = ShowWindow(st.hwnd, SW_HIDE);
                }
            }),
            Cmd::Quit => STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    let _ = DestroyWindow(st.hwnd);
                }
            }),
        }
    }
}

unsafe fn paint(st: &mut WindowState, frame: &Frame) {
    // Required before touching DIB bits: GDI may still have queued drawing
    // that would otherwise land on top of ours.
    let _ = GdiFlush();

    let bounds = st.bounds;
    let origin = (bounds.x, bounds.y);
    // Split the borrow: the renderer and the pixel buffer are disjoint fields.
    let renderer = &st.renderer;
    let len = (bounds.w as usize) * (bounds.h as usize) * 4;
    let data = std::slice::from_raw_parts_mut(st.bits, len);
    let mut canvas = Canvas::new(bounds.w, bounds.h, data);
    match frame {
        Frame::Cells { cells, typed } => renderer.draw(&mut canvas, cells, typed, origin),
        Frame::CursorHint { pos, dragging } => {
            renderer.draw_cursor_hint(&mut canvas, *pos, *dragging, origin)
        }
    }

    let screen_dc = GetDC(None);
    let dst = POINT {
        x: bounds.x,
        y: bounds.y,
    };
    let size = SIZE {
        cx: bounds.w,
        cy: bounds.h,
    };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };

    // A failure here paints nothing at all, with no other symptom. Staying
    // silent about it costs hours, so say so.
    if let Err(e) = UpdateLayeredWindow(
        st.hwnd,
        Some(screen_dc),
        Some(&dst),
        Some(&size),
        Some(st.dc_mem),
        Some(&src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    ) {
        eprintln!("overlay: UpdateLayeredWindow failed: {e}");
    }
    ReleaseDC(None, screen_dc);

    let _ = ShowWindow(st.hwnd, SW_SHOWNA);
    // Re-assert topmost: other always-on-top windows can leapfrog us.
    let _ = SetWindowPos(
        st.hwnd,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}
