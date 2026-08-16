//! Wayland backend.

use mouseless_core::{Button, LabeledCell, Point, Rect};
use mouseless_overlay::RenderOptions;
use mouseless_wayland::Event as WaylandEvent;

use crate::config::FileConfig;
use crate::platform::Event;

pub struct Platform {
    inner: mouseless_wayland::Platform,
}

pub struct Events {
    inner: crossbeam_channel::IntoIter<WaylandEvent>,
}

impl Iterator for Events {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        self.inner.next().map(|event| match event {
            WaylandEvent::Trigger => Event::Trigger,
            WaylandEvent::Key(press) => Event::Key(press),
            WaylandEvent::FocusLost => Event::FocusLost,
            WaylandEvent::LayoutChanged(bounds) => Event::LayoutChanged(bounds),
            WaylandEvent::Quit => Event::Quit,
        })
    }
}

pub fn start(
    cfg: &FileConfig,
    options: RenderOptions,
) -> Result<(Platform, Events), Box<dyn std::error::Error>> {
    let tap_window = std::time::Duration::from_millis(cfg.double_tap_ms.into());
    let (inner, events) = mouseless_wayland::start(options, tap_window)?;
    Ok((
        Platform { inner },
        Events {
            inner: events.into_iter(),
        },
    ))
}

impl Platform {
    pub fn screen(&self) -> Rect {
        self.inner.screen()
    }

    pub fn set_screen(&mut self, bounds: Rect) {
        self.inner.set_screen(bounds);
    }

    /// No-op here. The overlay's keyboard grab starts and stops with the
    /// overlay itself, so there is no separate capturing flag to mirror.
    pub fn set_capturing(&self, _on: bool) {}

    pub fn show(&self, cells: Vec<LabeledCell>, typed: String) {
        self.inner.show(cells, typed);
    }

    pub fn show_cursor_hint(&self, pos: Point, dragging: bool) {
        self.inner.show_cursor_hint(pos, dragging);
    }

    pub fn hide(&self) {
        self.inner.hide();
    }

    pub fn move_cursor(&self, p: Point) {
        self.inner.move_cursor(p);
    }

    pub fn click(&self, button: Button) {
        self.inner.click(button);
    }

    pub fn mouse_down(&self, button: Button) {
        self.inner.mouse_down(button);
    }

    pub fn mouse_up(&self, button: Button) {
        self.inner.mouse_up(button);
    }

    pub fn stop(self) {
        self.inner.stop();
    }

    /// How the user activates the grid, for the startup banner.
    ///
    /// Worth spelling out: the daemon cannot bind the key itself, so a user
    /// who has not added a compositor binding will otherwise sit looking at a
    /// running program that never does anything.
    pub fn trigger_lines(&self, _cfg: &FileConfig) -> Vec<String> {
        vec![
            "  trigger  bind a key to `my-mouseless toggle` in your compositor".to_string(),
            format!(
                "           socket {}",
                mouseless_wayland::trigger::socket_path().display()
            ),
        ]
    }
}
