//! Integer screen geometry, in virtual-screen pixel coordinates.
//!
//! The virtual screen origin can be negative (a monitor left of or above the
//! primary one), so every coordinate here is signed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn center(&self) -> Point {
        Point::new(self.x + self.w / 2, self.y + self.h / 2)
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Nearest point inside the rect. Keeps a nudged cursor on screen.
    pub fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.x, self.right() - 1),
            p.y.clamp(self.y, self.bottom() - 1),
        )
    }

    /// Split into `cols` x `rows` cells in row-major order.
    ///
    /// Boundaries are computed from the edges rather than by accumulating a
    /// cell width, so the cells tile the rect exactly: no seams, no overhang,
    /// even when the size does not divide evenly.
    pub fn subdivide(&self, cols: u32, rows: u32) -> Vec<Rect> {
        if cols == 0 || rows == 0 || self.is_empty() {
            return Vec::new();
        }
        let (cols, rows) = (cols as i64, rows as i64);
        let (w, h) = (self.w as i64, self.h as i64);

        let edge = |i: i64, n: i64, span: i64| -> i32 { (i * span / n) as i32 };

        let mut out = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            let top = self.y + edge(r, rows, h);
            let bottom = self.y + edge(r + 1, rows, h);
            for c in 0..cols {
                let left = self.x + edge(c, cols, w);
                let right = self.x + edge(c + 1, cols, w);
                out.push(Rect::new(left, top, right - left, bottom - top));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdivide_tiles_exactly() {
        let r = Rect::new(0, 0, 1920, 1080);
        let cells = r.subdivide(24, 14);
        assert_eq!(cells.len(), 24 * 14);
        // First cell starts at the origin, last cell ends at the far corner.
        assert_eq!((cells[0].x, cells[0].y), (0, 0));
        let last = cells.last().unwrap();
        assert_eq!((last.right(), last.bottom()), (1920, 1080));
    }

    #[test]
    fn subdivide_leaves_no_seams_on_uneven_sizes() {
        // 1001 does not divide by 7; adjacent cells must still touch.
        let cells = Rect::new(-13, 5, 1001, 999).subdivide(7, 11);
        for row in 0..11 {
            for col in 0..6 {
                let a = cells[row * 7 + col];
                let b = cells[row * 7 + col + 1];
                assert_eq!(a.right(), b.x, "horizontal seam at {row},{col}");
            }
        }
        for row in 0..10 {
            let a = cells[row * 7];
            let b = cells[(row + 1) * 7];
            assert_eq!(a.bottom(), b.y, "vertical seam at row {row}");
        }
    }

    #[test]
    fn subdivide_handles_degenerate_input() {
        assert!(Rect::new(0, 0, 100, 100).subdivide(0, 4).is_empty());
        assert!(Rect::new(0, 0, 0, 100).subdivide(4, 4).is_empty());
    }

    #[test]
    fn negative_origin_is_preserved() {
        let cells = Rect::new(-1920, -200, 1920, 1080).subdivide(2, 2);
        assert_eq!((cells[0].x, cells[0].y), (-1920, -200));
    }
}
