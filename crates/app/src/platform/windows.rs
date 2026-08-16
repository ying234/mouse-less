//! Windows backend: the low-level keyboard hook plus the layered overlay.

use mouseless_core::{Button, LabeledCell, Point, Rect};
use mouseless_os_kit::{hook, input, screen, Hook, HookEvent, Hotkey};
use mouseless_overlay::{Overlay, RenderOptions};

use crate::config::FileConfig;
use crate::platform::Event;

pub struct Platform {
    overlay: Overlay,
    hook: Hook,
    bounds: Rect,
    hotkey: Hotkey,
}

pub struct Events {
    inner: crossbeam_channel::IntoIter<HookEvent>,
}

impl Iterator for Events {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        self.inner.next().map(|event| match event {
            HookEvent::Hotkey => Event::Trigger,
            HookEvent::Key(press) => Event::Key(press),
        })
    }
}

pub fn start(
    cfg: &FileConfig,
    options: RenderOptions,
) -> Result<(Platform, Events), Box<dyn std::error::Error>> {
    let hotkey = cfg.resolve_hotkey()?;

    // Before any window exists, or Windows reports scaled coordinates and the
    // cursor lands somewhere other than the cell that was picked.
    screen::enable_dpi_awareness();

    // Ctrl+C terminates without unwinding, so a drag in progress would leave
    // the left button held down system-wide. This releases it on the way out.
    input::install_exit_guard();

    let bounds = screen::virtual_screen();
    let overlay = Overlay::start(bounds, options)?;
    let (hook_handle, events) = hook::start(hotkey)?;

    Ok((
        Platform {
            overlay,
            hook: hook_handle,
            bounds,
            hotkey,
        },
        Events {
            inner: events.into_iter(),
        },
    ))
}

impl Platform {
    pub fn screen(&self) -> Rect {
        self.bounds
    }

    /// Windows reports no layout changes to us, so this is never called.
    pub fn set_screen(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    /// Mirror the engine's mode into the hook, so the next keystroke is
    /// swallowed or passed through correctly even if the user is typing fast.
    pub fn set_capturing(&self, on: bool) {
        hook::set_capturing(on);
    }

    pub fn show(&self, cells: Vec<LabeledCell>, typed: String) {
        self.overlay.show(cells, typed);
    }

    pub fn show_cursor_hint(&self, pos: Point, dragging: bool) {
        self.overlay.show_cursor_hint(pos, dragging);
    }

    pub fn hide(&self) {
        self.overlay.hide();
    }

    pub fn move_cursor(&self, p: Point) {
        input::move_cursor(p);
    }

    pub fn click(&self, button: Button) {
        input::click(button);
    }

    pub fn mouse_down(&self, button: Button) {
        input::mouse_down(button);
    }

    pub fn mouse_up(&self, button: Button) {
        input::mouse_up(button);
    }

    pub fn stop(self) {
        self.hook.stop();
        self.overlay.stop();
    }

    /// Spell out what the trigger actually resolved to. A config that parses
    /// but does not do what the user expected is otherwise invisible.
    pub fn trigger_lines(&self, cfg: &FileConfig) -> Vec<String> {
        let described = match self.hotkey {
            Hotkey::Chord { .. } => format!("{} (chord)", cfg.hotkey),
            Hotkey::Tap {
                count,
                tap_ms,
                gap_ms,
                ..
            } => {
                let how = if count >= 2 {
                    format!("tap twice within {gap_ms} ms")
                } else {
                    "tap once".to_string()
                };
                format!("{} ({how}, press under {tap_ms} ms)", cfg.hotkey)
            }
        };
        vec![format!("  hotkey   {described}")]
    }
}
