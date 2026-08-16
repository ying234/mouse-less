//! The activation trigger, as a Unix socket.
//!
//! Wayland has no equivalent of a low-level keyboard hook: a client cannot see
//! keystrokes that are not addressed to it, and that is a deliberate security
//! property rather than a gap to work around. The compositor owns global
//! hotkeys, so the trigger is a compositor binding that runs
//! `my-mouseless toggle`, and this socket is how that one-shot invocation
//! reaches the running daemon.
//!
//! Once the overlay is up the story changes: its layer surface takes an
//! exclusive keyboard grab, so every subsequent keystroke arrives normally and
//! never reaches the application underneath.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::Event;

/// Turns a stream of taps into the double taps that actually mean something.
///
/// A lone modifier makes a good trigger because nothing else claims it, but
/// only if a single press does not fire: people release Ctrl without meaning
/// anything by it constantly. Requiring two within a short window is what
/// separates intent from habit — the same reasoning, and the same
/// `double_tap_ms` setting, as the Windows tap trigger.
#[derive(Debug)]
pub struct DoubleTap {
    window: Duration,
    /// The first tap of a pair, still waiting for its partner.
    pending: Option<Instant>,
}

impl DoubleTap {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: None,
        }
    }

    /// Record a tap, and report whether it completed a double tap.
    ///
    /// Completing one clears the state rather than leaving the tap available
    /// to pair with the next: three taps in a row are one trigger and a fresh
    /// start, not two triggers.
    pub fn tap(&mut self, now: Instant) -> bool {
        match self.pending {
            Some(first) if now.duration_since(first) <= self.window => {
                self.pending = None;
                true
            }
            _ => {
                self.pending = Some(now);
                false
            }
        }
    }
}

/// Where the daemon listens.
///
/// `XDG_RUNTIME_DIR` is the right home for it: per-user, mode 0700, and wiped
/// when the session ends, so a stale socket cannot outlive a reboot.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(dir).join("my-mouseless.sock")
}

/// Send one command to a running daemon.
pub fn send(command: &str) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path())?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// Owns the socket file, and removes it on the way out.
pub struct Listener {
    path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start accepting commands, forwarding them to `tx` on a background thread.
///
/// `tap_window` is how close together two `tap` commands must be to count as a
/// double tap.
///
/// Fails if another daemon already holds the socket, which is worth refusing:
/// two instances would both grab the keyboard and fight over the cursor.
pub fn listen(tx: Sender<Event>, tap_window: Duration) -> std::io::Result<Listener> {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // The file existing does not prove anyone is behind it — a crash
            // or a kill -9 leaves one there. Probing tells the two apart.
            if UnixStream::connect(&path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "another my-mouseless is already running on {}",
                        path.display()
                    ),
                ));
            }
            std::fs::remove_file(&path)?;
            UnixListener::bind(&path)?
        }
        Err(e) => return Err(e),
    };

    std::thread::Builder::new()
        .name("mouseless-trigger".into())
        .spawn(move || {
            let mut double_tap = DoubleTap::new(tap_window);
            for stream in listener.incoming().flatten() {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let event = match line.trim() {
                        "toggle" | "" => Event::Trigger,
                        // Half of a double tap is not an event at all, so
                        // there is nothing to forward until the second one.
                        "tap" => {
                            if double_tap.tap(Instant::now()) {
                                Event::Trigger
                            } else {
                                continue;
                            }
                        }
                        "quit" => Event::Quit,
                        other => {
                            eprintln!("trigger: ignoring unknown command {other:?}");
                            continue;
                        }
                    };
                    // A closed receiver means the engine loop is gone; there
                    // is nothing left to trigger.
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
        })?;

    Ok(Listener { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_millis(350);

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_single_tap_does_nothing() {
        // The whole point: releasing Ctrl without meaning anything by it is
        // something people do all day.
        let start = Instant::now();
        let mut d = DoubleTap::new(WINDOW);
        assert!(!d.tap(start));
    }

    #[test]
    fn two_taps_inside_the_window_trigger() {
        let start = Instant::now();
        let mut d = DoubleTap::new(WINDOW);
        assert!(!d.tap(start));
        assert!(d.tap(at(start, 200)));
    }

    #[test]
    fn the_boundary_counts_as_inside() {
        let start = Instant::now();
        let mut d = DoubleTap::new(WINDOW);
        d.tap(start);
        assert!(d.tap(at(start, 350)));
    }

    #[test]
    fn two_taps_outside_the_window_do_not() {
        let start = Instant::now();
        let mut d = DoubleTap::new(WINDOW);
        assert!(!d.tap(start));
        assert!(!d.tap(at(start, 351)));
    }

    #[test]
    fn a_late_tap_becomes_the_start_of_the_next_pair() {
        // Otherwise a slow tap would be wasted and the user would have to tap
        // three times to get anywhere.
        let start = Instant::now();
        let mut d = DoubleTap::new(WINDOW);
        d.tap(start);
        assert!(!d.tap(at(start, 900)));
        assert!(d.tap(at(start, 1000)));
    }

    #[test]
    fn three_quick_taps_trigger_once() {
        // Not twice: the second tap is consumed by the first trigger, so the
        // third has to wait for a partner of its own.
        let start = Instant::now();
        let mut d = DoubleTap::new(WINDOW);
        assert!(!d.tap(start));
        assert!(d.tap(at(start, 100)));
        assert!(!d.tap(at(start, 200)));
        assert!(d.tap(at(start, 300)));
    }
}
