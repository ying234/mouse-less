//! Show the grid overlay for a few seconds, with no hook installed.
//!
//! `cargo run --example preview` — useful for checking rendering and colors
//! without wiring up the keyboard.

use mouseless_core::{label, GridConfig, LabeledCell};
use mouseless_os_kit::screen;
use mouseless_overlay::{Overlay, RenderOptions};

fn main() {
    screen::enable_dpi_awareness();
    let bounds = screen::virtual_screen();
    let overlay = Overlay::start(bounds, RenderOptions::default()).expect("overlay");
    println!("hwnd={} bounds={},{} {}x{}", overlay.hwnd(), bounds.x, bounds.y, bounds.w, bounds.h);

    let cfg = GridConfig::default();
    let rects = bounds.subdivide(cfg.coarse_cols, cfg.coarse_rows);
    let labels = label::generate(&cfg.alphabet, rects.len());
    let cells: Vec<LabeledCell> = labels
        .into_iter()
        .zip(rects)
        .map(|(label, rect)| LabeledCell { label, rect })
        .collect();

    // A typed prefix of "a" so the consumed-character dimming is visible too.
    let visible: Vec<LabeledCell> = cells
        .iter()
        .filter(|c| c.label.starts_with('a'))
        .cloned()
        .collect();

    println!("showing full grid ({} cells)", cells.len());
    overlay.show(cells, String::new());
    std::thread::sleep(std::time::Duration::from_secs(6));

    println!("showing filtered grid ({} cells)", visible.len());
    overlay.show(visible, "a".to_string());
    std::thread::sleep(std::time::Duration::from_secs(6));

    println!("showing cursor hint");
    overlay.show_cursor_hint(mouseless_core::Point::new(700, 400), false);
    std::thread::sleep(std::time::Duration::from_secs(6));

    println!("showing cursor hint (dragging)");
    overlay.show_cursor_hint(mouseless_core::Point::new(700, 400), true);
    std::thread::sleep(std::time::Duration::from_secs(6));

    overlay.stop();
}
