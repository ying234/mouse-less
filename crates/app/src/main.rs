//! my-mouseless — keyboard-driven cursor control for Windows.
//!
//! Three threads:
//!   * the hook thread  — owns the keyboard hook, must never block
//!   * the overlay thread — owns the window and all GDI objects
//!   * this thread      — owns the engine, and is the only place decisions live

mod config;

use mouseless_core::{Action, Engine, Input};
use mouseless_os_kit::{hook, input, screen, HookEvent, Hotkey};
use mouseless_overlay::{Overlay, RenderOptions};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (file_cfg, path) = config::load()?;
    let (hotkey, grid) = file_cfg.resolve()?;

    // Before any window exists, or Windows reports scaled coordinates and the
    // cursor lands somewhere other than the cell that was picked.
    screen::enable_dpi_awareness();

    // Ctrl+C terminates without unwinding, so a drag in progress would leave
    // the left button held down system-wide. This releases it on the way out.
    input::install_exit_guard();

    let bounds = screen::virtual_screen();
    let overlay = Overlay::start(
        bounds,
        RenderOptions {
            label_font_max_px: file_cfg.label_font_max_px,
        },
    )?;
    let (hook_handle, events) = hook::start(hotkey)?;

    let mut engine = Engine::new(grid);

    println!("my-mouseless running");
    println!("  config   {}", path.display());
    // Spell out what the trigger actually resolved to. A config that parses
    // but does not do what the user expected is otherwise invisible.
    match hotkey {
        Hotkey::Chord { .. } => println!("  hotkey   {} (chord)", file_cfg.hotkey),
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
            println!(
                "  hotkey   {} ({how}, press under {tap_ms} ms)",
                file_cfg.hotkey
            );
        }
    }
    println!(
        "  screen   {}x{} at ({}, {})",
        bounds.w, bounds.h, bounds.x, bounds.y
    );
    println!("  grid     type a label to select · backspace undo · esc cancel");
    println!("  cursor   space click · r right · m middle · v drag · g grid · hjkl move");
    println!("  text     v to anchor, g + label to jump to the end, space to release");
    println!("  quit     Ctrl+C in this window");

    // The hook sends; we decide. Everything below runs off the input path.
    for event in events {
        let input = match event {
            HookEvent::Hotkey => {
                if engine.is_capturing() {
                    Input::Cancel
                } else {
                    Input::Activate(bounds)
                }
            }
            HookEvent::Key(press) => Input::Key(press),
        };

        let actions = engine.handle(input);

        // Mirror the engine's mode into the hook before performing actions, so
        // the next keystroke is swallowed or passed through correctly even if
        // the user is typing fast.
        hook::set_capturing(engine.is_capturing());

        for action in actions {
            perform(action, &overlay);
        }
    }

    hook_handle.stop();
    overlay.stop();
    Ok(())
}

fn perform(action: Action, overlay: &Overlay) {
    match action {
        Action::ShowCells { cells, typed } => overlay.show(cells, typed),
        Action::ShowCursorHint { pos, dragging } => overlay.show_cursor_hint(pos, dragging),
        Action::Hide => overlay.hide(),
        Action::MoveCursor(p) => {
            input::move_cursor(p);
        }
        Action::Click(button) => {
            input::click(button);
        }
        Action::MouseDown(button) => {
            input::mouse_down(button);
        }
        Action::MouseUp(button) => {
            input::mouse_up(button);
        }
    }
}
