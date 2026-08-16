//! Wayland platform layer: overlay surfaces, keyboard grab, cursor control.
//!
//! This is the Linux counterpart to `mouseless-os-kit` plus the Windows
//! layered window, and it is one crate rather than two because on Wayland they
//! are one connection. The compositor will not talk to a client about
//! keystrokes it has no surface for, so the keyboard grab *is* the overlay:
//! the layer surface asks for exclusive keyboard interactivity, and the grid
//! being on screen is exactly the period during which we see keys.
//!
//! What this means in practice, compared to the Windows build:
//!
//!   * the activation hotkey belongs to the compositor, not to us — see
//!     [`trigger`] for how a keybinding reaches the running daemon;
//!   * nothing needs elevated privileges, `uinput`, or membership of the
//!     `input` group;
//!   * keystrokes are never hidden from an application that was going to get
//!     them, because we only ever see our own.

#![cfg(unix)]

pub mod keymap;
mod pointer;
mod state;
pub mod trigger;

use crossbeam_channel::Receiver;
use mouseless_core::{Button, KeyPress, LabeledCell, Point, Rect};
use mouseless_overlay::RenderOptions;
use smithay_client_toolkit::reexports::calloop::channel::Sender as CmdSender;

use state::{Cmd, Frame};

/// Something the engine thread needs to know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The compositor keybinding fired, via the trigger socket.
    Trigger,
    Key(KeyPress),
    /// The overlay lost its keyboard grab while it was up.
    FocusLost,
    /// Monitors were added, removed or rearranged; the new layout bounds.
    LayoutChanged(Rect),
    /// Shut down cleanly: a signal, or `my-mouseless quit`.
    Quit,
}

#[derive(Debug)]
pub enum Error {
    Connect(String),
    MissingGlobal(&'static str),
    NoOutputs,
    Font(String),
    Shm(String),
    Loop(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connect(e) => write!(f, "could not connect to the Wayland compositor: {e}"),
            Error::MissingGlobal(name) => write!(
                f,
                "this compositor does not support {name}, which my-mouseless needs"
            ),
            Error::NoOutputs => write!(f, "no usable outputs were reported"),
            Error::Font(e) => write!(f, "{e}"),
            Error::Shm(e) => write!(f, "could not allocate shared memory: {e}"),
            Error::Loop(e) => write!(f, "could not start the event loop: {e}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Handle to the Wayland thread. Commands go one way, [`Event`]s the other.
pub struct Platform {
    cmd: CmdSender<Cmd>,
    bounds: Rect,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Held so the socket file is removed when we exit.
    _trigger: trigger::Listener,
}

/// Connect to the compositor and start listening for triggers.
pub fn start(options: RenderOptions) -> Result<(Platform, Receiver<Event>), Error> {
    let (events_tx, events_rx) = crossbeam_channel::unbounded();
    let (cmd_tx, cmd_rx) = smithay_client_toolkit::reexports::calloop::channel::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);

    let thread_events = events_tx.clone();
    let thread = std::thread::Builder::new()
        .name("mouseless-wayland".into())
        .spawn(move || state::run(options, thread_events, cmd_rx, ready_tx))?;

    let bounds = match ready_rx.recv() {
        Ok(Ok(bounds)) => bounds,
        Ok(Err(e)) => return Err(e),
        // The thread died before reporting; its panic message is already out.
        Err(_) => return Err(Error::Connect("the Wayland thread stopped".into())),
    };

    let listener = trigger::listen(events_tx.clone())?;
    install_signal_handler(events_tx);

    Ok((
        Platform {
            cmd: cmd_tx,
            bounds,
            thread: Some(thread),
            _trigger: listener,
        },
        events_rx,
    ))
}

/// Turn Ctrl+C and `kill` into a [`Event::Quit`] rather than a sudden death.
///
/// It matters because of drags: a synthetic button-down whose release never
/// arrives leaves the button held for the rest of the session, and the user
/// cannot release a button they never pressed.
fn install_signal_handler(events: crossbeam_channel::Sender<Event>) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) {
        Ok(signals) => signals,
        Err(e) => {
            eprintln!("warning: no signal handler installed, a drag could survive exit: {e}");
            return;
        }
    };
    std::thread::Builder::new()
        .name("mouseless-signals".into())
        .spawn(move || {
            let mut signals = signals;
            for _ in signals.forever() {
                if events.send(Event::Quit).is_err() {
                    return;
                }
            }
        })
        .ok();
}

impl Platform {
    /// Bounding box of every monitor, in logical pixels.
    pub fn screen(&self) -> Rect {
        self.bounds
    }

    /// Track a layout change reported by [`Event::LayoutChanged`].
    pub fn set_screen(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    pub fn show(&self, cells: Vec<LabeledCell>, typed: String) {
        self.send(Cmd::Show(Frame::Cells { cells, typed }));
    }

    pub fn show_cursor_hint(&self, pos: Point, dragging: bool) {
        self.send(Cmd::Show(Frame::CursorHint { pos, dragging }));
    }

    pub fn hide(&self) {
        self.send(Cmd::Hide);
    }

    pub fn move_cursor(&self, p: Point) {
        self.send(Cmd::MoveCursor(p));
    }

    pub fn click(&self, button: Button) {
        self.send(Cmd::Click(button));
    }

    pub fn mouse_down(&self, button: Button) {
        self.send(Cmd::MouseDown(button));
    }

    pub fn mouse_up(&self, button: Button) {
        self.send(Cmd::MouseUp(button));
    }

    /// Ask the Wayland thread to shut down, and wait for it.
    ///
    /// Waiting is the point: the thread releases any held mouse button on its
    /// way out, and the process exiting first would strand it.
    pub fn stop(mut self) {
        self.send(Cmd::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn send(&self, cmd: Cmd) {
        // A closed channel means the Wayland thread is already gone, which is
        // only reachable while shutting down.
        let _ = self.cmd.send(cmd);
    }
}
