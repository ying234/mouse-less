//! my-mouseless — keyboard-driven cursor control.
//!
//! Three threads on both platforms, each owning exactly one thing:
//!
//!   * the input thread — the keyboard hook (Windows) or the Wayland
//!     connection (Linux); must never block
//!   * the trigger thread — the hotkey (Windows, inside the hook) or the
//!     command socket (Linux)
//!   * this thread — owns the engine, and is the only place decisions live
//!
//! Everything platform-specific is behind [`platform`], so the loop below is
//! the whole program's behaviour and there is only one copy of it.

mod config;
mod platform;

use mouseless_core::{Action, Engine, Input};
use mouseless_overlay::RenderOptions;

use platform::{Event, Platform};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if handle_cli()? {
        return Ok(());
    }

    let (file_cfg, path) = config::load()?;
    let grid = file_cfg.resolve()?;
    let options = RenderOptions {
        label_font_max_px: file_cfg.label_font_max_px,
        ..Default::default()
    };

    let (mut platform, events): (Platform, platform::Events) =
        platform::start(&file_cfg, options)?;
    let mut engine = Engine::new(grid);

    let bounds = platform.screen();
    println!("my-mouseless running");
    println!("  config   {}", path.display());
    for line in platform.trigger_lines(&file_cfg) {
        println!("{line}");
    }
    println!(
        "  screen   {}x{} at ({}, {})",
        bounds.w, bounds.h, bounds.x, bounds.y
    );
    println!("  grid     type a label to select · backspace undo · esc cancel");
    println!("  cursor   space click · r right · m middle · v drag · g grid · hjkl move");
    println!("  text     v to anchor, g + label to jump to the end, space to release");
    println!("  quit     Ctrl+C in this window");

    // The input thread sends; we decide. Everything below runs off the input
    // path, so nothing here can stall the user's keyboard.
    for event in events {
        let input = match event {
            Event::Trigger => {
                if engine.is_capturing() {
                    Input::Cancel
                } else {
                    Input::Activate(platform.screen())
                }
            }
            Event::Key(press) => Input::Key(press),
            Event::FocusLost => Input::Cancel,
            // The cells describe a screen arrangement that no longer exists,
            // so whatever selection was in progress has to go with it.
            Event::LayoutChanged(bounds) => {
                platform.set_screen(bounds);
                Input::Cancel
            }
            Event::Quit => {
                // Cancelling first is what releases a drag; exiting mid-drag
                // would leave the button held down for the whole session.
                for action in engine.handle(Input::Cancel) {
                    perform(action, &platform);
                }
                break;
            }
        };

        let actions = engine.handle(input);
        platform.set_capturing(engine.is_capturing());

        for action in actions {
            perform(action, &platform);
        }
    }

    platform.stop();
    Ok(())
}

fn perform(action: Action, platform: &Platform) {
    match action {
        Action::ShowCells { cells, typed } => platform.show(cells, typed),
        Action::ShowCursorHint { pos, dragging } => platform.show_cursor_hint(pos, dragging),
        Action::Hide => platform.hide(),
        Action::MoveCursor(p) => platform.move_cursor(p),
        Action::Click(button) => platform.click(button),
        Action::MouseDown(button) => platform.mouse_down(button),
        Action::MouseUp(button) => platform.mouse_up(button),
    }
}

#[cfg(unix)]
const USAGE: &str = "\
my-mouseless — keyboard-driven cursor control

  my-mouseless           run the daemon
  my-mouseless toggle    open or close the grid on the running daemon
  my-mouseless quit      ask the running daemon to exit

Wayland gives no client a global hotkey, so bind `my-mouseless toggle` in your
compositor. For Hyprland, in ~/.config/hypr/hyprland.conf:

  exec-once = my-mouseless
  bind = SUPER, slash, exec, my-mouseless toggle";

#[cfg(windows)]
const USAGE: &str = "\
my-mouseless — keyboard-driven cursor control

  my-mouseless           run it

There are no subcommands here: the hotkey is installed by the program itself,
so nothing needs to reach it from outside. Set `hotkey` in the config file
printed at startup to change it.";

/// Handle a one-shot command, returning whether it was one.
///
/// These talk to an already-running daemon over its socket, which is what a
/// compositor keybinding invokes: spawning a whole second instance per
/// keypress would grab the keyboard twice and fight over the cursor.
#[cfg(unix)]
fn handle_cli() -> Result<bool, Box<dyn std::error::Error>> {
    let Some(arg) = std::env::args().nth(1) else {
        return Ok(false);
    };
    match arg.as_str() {
        "toggle" | "quit" => match mouseless_wayland::trigger::send(&arg) {
            Ok(()) => Ok(true),
            Err(e) => Err(format!(
                "could not reach a running my-mouseless ({e}); start one first"
            )
            .into()),
        },
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(true)
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    }
}

/// Windows owns its hotkey through the hook, so there is nothing to send —
/// only a `--help` worth answering rather than starting up and ignoring.
#[cfg(windows)]
fn handle_cli() -> Result<bool, Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        None => Ok(false),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(true)
        }
        Some(other) => Err(format!("unknown argument {other:?}\n\n{USAGE}").into()),
    }
}
