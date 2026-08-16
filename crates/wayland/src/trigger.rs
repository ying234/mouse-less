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

use crossbeam_channel::Sender;

use crate::Event;

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
/// Fails if another daemon already holds the socket, which is worth refusing:
/// two instances would both grab the keyboard and fight over the cursor.
pub fn listen(tx: Sender<Event>) -> std::io::Result<Listener> {
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
            for stream in listener.incoming().flatten() {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let event = match line.trim() {
                        "toggle" | "" => Event::Trigger,
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
