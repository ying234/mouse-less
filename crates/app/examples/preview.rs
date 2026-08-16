//! Show the grid overlay for a few seconds, with no trigger wired up.
//!
//! `cargo run --example preview` — useful for checking rendering and colors
//! without a hotkey, a compositor binding, or a running daemon.
//!
//! On Wayland the overlay holds an exclusive keyboard grab while it is up, so
//! the keyboard is unusable for the few seconds this runs. It exits on its own.

use std::time::Duration;

use mouseless_core::{label, GridConfig, LabeledCell, Rect};
use mouseless_overlay::RenderOptions;

// Both modules are shared with the binary rather than duplicated. Most of what
// they offer is unused here, which is the point of a preview.
#[path = "../src/config.rs"]
#[allow(dead_code)]
mod config;
#[path = "../src/platform/mod.rs"]
#[allow(dead_code, unused_imports)]
mod platform;

fn main() {
    let cfg = config::FileConfig::default();
    let (platform, _events) = platform::start(&cfg, RenderOptions::default()).expect("platform");

    let bounds = platform.screen();
    println!("bounds={},{} {}x{}", bounds.x, bounds.y, bounds.w, bounds.h);

    let grid = GridConfig::default();
    let cells = build_cells(bounds, &grid);
    // A typed prefix of "a" so the consumed-character dimming is visible too.
    let filtered: Vec<LabeledCell> = cells
        .iter()
        .filter(|c| c.label.starts_with('a'))
        .cloned()
        .collect();

    let hold = Duration::from_secs(3);

    println!("showing full grid ({} cells)", cells.len());
    platform.show(cells, String::new());
    std::thread::sleep(hold);

    println!("showing filtered grid ({} cells)", filtered.len());
    platform.show(filtered, "a".to_string());
    std::thread::sleep(hold);

    println!("showing cursor hint");
    platform.show_cursor_hint(bounds.center(), false);
    std::thread::sleep(hold);

    println!("showing cursor hint (dragging)");
    platform.show_cursor_hint(bounds.center(), true);
    std::thread::sleep(hold);

    platform.hide();
    platform.stop();
}

fn build_cells(bounds: Rect, grid: &GridConfig) -> Vec<LabeledCell> {
    let rects = bounds.subdivide(grid.coarse_cols, grid.coarse_rows);
    label::generate(&grid.alphabet, rects.len())
        .into_iter()
        .zip(rects)
        .map(|(label, rect)| LabeledCell { label, rect })
        .collect()
}
