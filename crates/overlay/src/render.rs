//! Turning engine cells into pixels.

use fontdue::{Font, FontSettings};
use mouseless_core::{LabeledCell, Point};

use crate::canvas::{Canvas, Rgba};

const BACKDROP: Rgba = Rgba::new(8, 10, 16, 110);
const CELL_BORDER: Rgba = Rgba::new(255, 255, 255, 28);
const CHIP_BG: Rgba = Rgba::new(16, 18, 28, 235);
const CHIP_BORDER: Rgba = Rgba::new(120, 200, 255, 90);
const TEXT: Rgba = Rgba::new(240, 246, 255, 255);
/// Characters the user already typed, kept visible but clearly spent.
const TEXT_CONSUMED: Rgba = Rgba::new(120, 130, 150, 255);
const CROSSHAIR: Rgba = Rgba::new(120, 220, 255, 230);
const CROSSHAIR_SHADOW: Rgba = Rgba::new(0, 0, 0, 140);
const HINT_TEXT: &str =
    "space click  ·  r right  ·  m middle  ·  v drag  ·  g grid  ·  hjkl move  ·  esc";
/// Shown while a button is held. Deliberately different in wording and color
/// so a held button is never mistaken for the idle crosshair.
const HINT_TEXT_DRAG: &str =
    "DRAGGING  ·  space drop  ·  g grid  ·  hjkl extend  ·  esc release";
const CROSSHAIR_DRAG: Rgba = Rgba::new(255, 180, 90, 240);
const CHIP_BG_DRAG: Rgba = Rgba::new(48, 26, 8, 240);
const CHIP_BORDER_DRAG: Rgba = Rgba::new(255, 180, 90, 140);

/// Fonts to try, in order, from the system font directory.
const FONT_CANDIDATES: [&str; 4] = ["segoeuib.ttf", "arialbd.ttf", "segoeui.ttf", "arial.ttf"];

/// Below this, labels stop being readable at all.
const MIN_FONT_PX: f32 = 9.0;

/// Largest label text, in pixels.
///
/// Only the coarse grid reaches this: its cells are big enough that the
/// size-from-cell formula saturates, while refined cells compute well under it.
/// So raising this makes level one bigger and leaves the refinement alone.
pub const DEFAULT_LABEL_FONT_MAX_PX: f32 = 22.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    pub label_font_max_px: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            label_font_max_px: DEFAULT_LABEL_FONT_MAX_PX,
        }
    }
}

/// Label size for a cell: proportional to the cell, bounded at both ends.
///
/// Height drives it, with a width term so that a wide-but-short cell cannot
/// produce text taller than its own row.
fn label_font_px(cell_w: i32, cell_h: i32, max_px: f32) -> f32 {
    let h = cell_h.max(1) as f32;
    let w = cell_w.max(1) as f32;
    (h / 3.0)
        .min(w * 0.7)
        .clamp(MIN_FONT_PX, max_px.max(MIN_FONT_PX))
}

pub struct Renderer {
    font: Font,
    options: RenderOptions,
}

#[derive(Debug)]
pub enum FontError {
    NotFound,
    Parse(String),
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::NotFound => write!(
                f,
                "no usable system font found (tried {})",
                FONT_CANDIDATES.join(", ")
            ),
            FontError::Parse(e) => write!(f, "could not parse font: {e}"),
        }
    }
}

impl std::error::Error for FontError {}

impl Renderer {
    /// Load a bold UI font from the Windows font directory.
    ///
    /// Loading at runtime rather than embedding keeps the binary small and
    /// picks up the font the user actually reads system UI in.
    pub fn new(options: RenderOptions) -> Result<Self, FontError> {
        let dir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let bytes = FONT_CANDIDATES
            .iter()
            .find_map(|name| std::fs::read(format!("{dir}\\Fonts\\{name}")).ok())
            .ok_or(FontError::NotFound)?;

        let font = Font::from_bytes(bytes, FontSettings::default())
            .map_err(|e| FontError::Parse(e.to_string()))?;
        Ok(Self { font, options })
    }

    fn text_width(&self, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|c| self.font.metrics(c, px).advance_width)
            .sum()
    }

    /// Draw the backdrop and every visible cell.
    ///
    /// `origin` is the virtual-screen coordinate of the canvas's top-left
    /// pixel; cell rects are in virtual-screen space and get shifted by it.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        cells: &[LabeledCell],
        typed: &str,
        origin: (i32, i32),
    ) {
        canvas.fill(BACKDROP);
        if cells.is_empty() {
            return;
        }

        // Scale text to the cells so refined grids stay legible without their
        // chips overlapping.
        let px = label_font_px(
            cells[0].rect.w,
            cells[0].rect.h,
            self.options.label_font_max_px,
        );

        let typed_len = typed.chars().count();

        for cell in cells {
            let x = cell.rect.x - origin.0;
            let y = cell.rect.y - origin.1;
            canvas.stroke_rect(x, y, cell.rect.w, cell.rect.h, CELL_BORDER);

            let text_w = self.text_width(&cell.label, px);
            let pad_x = 3i32;
            let chip_w = text_w.ceil() as i32 + pad_x * 2;
            let chip_h = px.ceil() as i32 + 5;

            let chip_x = x + (cell.rect.w - chip_w) / 2;
            let chip_y = y + (cell.rect.h - chip_h) / 2;

            canvas.fill_rect(chip_x, chip_y, chip_w, chip_h, CHIP_BG);
            canvas.stroke_rect(chip_x, chip_y, chip_w, chip_h, CHIP_BORDER);

            // Baseline sits a little above the chip's bottom edge.
            let baseline = chip_y + chip_h - 3;
            let mut pen = chip_x as f32 + pad_x as f32;

            for (i, ch) in cell.label.chars().enumerate() {
                let color = if i < typed_len { TEXT_CONSUMED } else { TEXT };
                self.draw_glyph(canvas, ch, px, pen, baseline, color);
                pen += self.font.metrics(ch, px).advance_width;
            }
        }
    }

    /// Draw the cursor-mode crosshair and its key hints.
    ///
    /// The backdrop is cleared to fully transparent here: in cursor mode the
    /// user is aiming at real content, so dimming the screen would fight the
    /// task. The crosshair alone signals that the keyboard is captured.
    pub fn draw_cursor_hint(
        &self,
        canvas: &mut Canvas<'_>,
        pos: Point,
        dragging: bool,
        origin: (i32, i32),
    ) {
        canvas.fill(Rgba::new(0, 0, 0, 0));

        let (crosshair, chip_bg, chip_border, hint) = if dragging {
            (CROSSHAIR_DRAG, CHIP_BG_DRAG, CHIP_BORDER_DRAG, HINT_TEXT_DRAG)
        } else {
            (CROSSHAIR, CHIP_BG, CHIP_BORDER, HINT_TEXT)
        };

        let x = pos.x - origin.0;
        let y = pos.y - origin.1;

        // A gap around the centre keeps the exact target pixel visible.
        const ARM: i32 = 26;
        const GAP: i32 = 4;
        for d in GAP..ARM {
            for (px, py) in [(x - d, y), (x + d, y), (x, y - d), (x, y + d)] {
                canvas.blend(px, py, crosshair, 255);
                // A dark companion pixel keeps the line readable over light
                // backgrounds without needing to sample what is underneath.
                canvas.blend(px, py + 1, CROSSHAIR_SHADOW, 255);
            }
        }

        let px = 13.0;
        let text_w = self.text_width(hint, px).ceil() as i32;
        let chip_w = text_w + 12;
        let chip_h = px.ceil() as i32 + 9;

        // Sit below-right of the cursor, flipping when that would run off the
        // edge of the canvas.
        let mut chip_x = x + ARM;
        let mut chip_y = y + ARM;
        if chip_x + chip_w > canvas.width {
            chip_x = x - ARM - chip_w;
        }
        if chip_y + chip_h > canvas.height {
            chip_y = y - ARM - chip_h;
        }
        chip_x = chip_x.clamp(0, (canvas.width - chip_w).max(0));
        chip_y = chip_y.clamp(0, (canvas.height - chip_h).max(0));

        canvas.fill_rect(chip_x, chip_y, chip_w, chip_h, chip_bg);
        canvas.stroke_rect(chip_x, chip_y, chip_w, chip_h, chip_border);

        let baseline = chip_y + chip_h - 5;
        let mut pen = chip_x as f32 + 6.0;
        for ch in hint.chars() {
            self.draw_glyph(canvas, ch, px, pen, baseline, TEXT);
            pen += self.font.metrics(ch, px).advance_width;
        }
    }

    fn draw_glyph(
        &self,
        canvas: &mut Canvas<'_>,
        ch: char,
        px: f32,
        pen_x: f32,
        baseline_y: i32,
        color: Rgba,
    ) {
        let (metrics, bitmap) = self.font.rasterize(ch, px);
        if metrics.width == 0 || metrics.height == 0 {
            return;
        }
        let gx = (pen_x + metrics.xmin as f32).round() as i32;
        // fontdue's ymin is the offset of the bitmap's bottom from the
        // baseline, positive upward, so the top edge is height + ymin above it.
        let gy = baseline_y - (metrics.height as i32 + metrics.ymin);

        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let coverage = bitmap[row * metrics.width + col];
                if coverage > 0 {
                    canvas.blend(gx + col as i32, gy + row as i32, color, coverage);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_core::Rect;

    fn cells() -> Vec<LabeledCell> {
        Rect::new(0, 0, 200, 100)
            .subdivide(2, 2)
            .into_iter()
            .zip(["aa", "ab", "ba", "bb"])
            .map(|(rect, label)| LabeledCell {
                label: label.to_string(),
                rect,
            })
            .collect()
    }

    // The renderer needs a real system font, so these only run on Windows.
    #[test]
    fn draws_without_panicking_and_marks_pixels() {
        let Ok(r) = Renderer::new(RenderOptions::default()) else {
            eprintln!("skipping: no system font available");
            return;
        };
        let mut buf = vec![0u8; 200 * 100 * 4];
        let mut canvas = Canvas::new(200, 100, &mut buf);
        r.draw(&mut canvas, &cells(), "a", (0, 0));

        assert!(buf.iter().any(|&b| b > 0), "something was drawn");
    }

    #[test]
    fn coarse_cells_saturate_at_the_configured_maximum() {
        // A 24x14 grid on a 3440x2640 desktop: cells are large enough that the
        // formula saturates, so the cap is what the user actually sees.
        assert_eq!(label_font_px(143, 188, 22.0), 22.0);
        assert_eq!(label_font_px(143, 188, 17.0), 17.0);
    }

    #[test]
    fn refined_cells_are_unaffected_by_the_maximum() {
        // 5x5 within one of those cells => about 28x37 px. Well under the cap,
        // so raising the cap must not change the refinement level's text.
        let small = label_font_px(28, 37, 17.0);
        assert!((small - 12.333).abs() < 0.01, "got {small}");
        assert_eq!(
            label_font_px(28, 37, 22.0),
            small,
            "raising the cap must only affect the coarse grid"
        );
    }

    #[test]
    fn tiny_cells_stop_shrinking_at_the_readable_floor() {
        assert_eq!(label_font_px(4, 4, 22.0), MIN_FONT_PX);
        assert_eq!(label_font_px(0, 0, 22.0), MIN_FONT_PX);
    }

    #[test]
    fn a_nonsense_maximum_cannot_produce_invisible_text() {
        assert_eq!(label_font_px(143, 188, 0.0), MIN_FONT_PX);
    }

    #[test]
    fn wide_short_cells_are_limited_by_height() {
        // Text must never be taller than the row it labels.
        assert!(label_font_px(400, 30, 22.0) <= 10.0);
    }

    #[test]
    fn empty_cell_list_still_paints_the_backdrop() {
        let Ok(r) = Renderer::new(RenderOptions::default()) else { return };
        let mut buf = vec![0u8; 40 * 40 * 4];
        let mut canvas = Canvas::new(40, 40, &mut buf);
        r.draw(&mut canvas, &[], "", (0, 0));
        assert!(buf[3] > 0, "backdrop alpha present");
    }

    #[test]
    fn cells_are_offset_by_the_canvas_origin() {
        let Ok(r) = Renderer::new(RenderOptions::default()) else { return };
        // A canvas whose origin matches the cells should be busier at the top
        // left than one offset far away (where cells fall outside and clip).
        let mut near = vec![0u8; 200 * 100 * 4];
        r.draw(
            &mut Canvas::new(200, 100, &mut near),
            &cells(),
            "",
            (0, 0),
        );
        let mut far = vec![0u8; 200 * 100 * 4];
        r.draw(
            &mut Canvas::new(200, 100, &mut far),
            &cells(),
            "",
            (5000, 5000),
        );
        assert_ne!(near, far, "origin must shift the drawing");
    }
}
