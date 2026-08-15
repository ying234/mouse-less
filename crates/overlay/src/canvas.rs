//! A BGRA pixel buffer matching the layout `UpdateLayeredWindow` expects.
//!
//! Windows composites layered windows from *premultiplied* BGRA. Getting that
//! wrong shows up as bright fringing around text, so premultiplication happens
//! in exactly one place: [`Canvas::blend`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(r, g, b, a)
    }

    /// This color at a fraction of its opacity, used to dim consumed label
    /// characters without defining a second constant for each shade.
    pub const fn scale_alpha(self, num: u32) -> Self {
        Self(self.0, self.1, self.2, ((self.3 as u32 * num) / 255) as u8)
    }
}

/// Borrowed pixel storage — the buffer belongs to a GDI DIB section.
pub struct Canvas<'a> {
    pub width: i32,
    pub height: i32,
    data: &'a mut [u8],
}

#[inline]
fn mul255(a: u32, b: u32) -> u32 {
    // Rounded fixed-point multiply; the +127 avoids the systematic darkening
    // that a plain integer divide introduces over many blended layers.
    (a * b + 127) / 255
}

impl<'a> Canvas<'a> {
    /// # Safety contract (upheld by the caller)
    /// `data` must be exactly `width * height * 4` bytes.
    pub fn new(width: i32, height: i32, data: &'a mut [u8]) -> Self {
        debug_assert_eq!(data.len(), (width as usize) * (height as usize) * 4);
        Self {
            width,
            height,
            data,
        }
    }

    /// Overwrite every pixel, ignoring what was there.
    ///
    /// The backdrop covers the whole virtual screen, so this runs over several
    /// million pixels per activation; skipping the read-modify-write of a
    /// blend is what keeps the overlay feeling instant.
    pub fn fill(&mut self, color: Rgba) {
        let a = color.3 as u32;
        let px = [
            mul255(color.2 as u32, a) as u8,
            mul255(color.1 as u32, a) as u8,
            mul255(color.0 as u32, a) as u8,
            color.3,
        ];
        for chunk in self.data.chunks_exact_mut(4) {
            chunk.copy_from_slice(&px);
        }
    }

    /// Source-over blend of one pixel. `coverage` scales the color's alpha,
    /// which is how glyph antialiasing enters.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, color: Rgba, coverage: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let a = mul255(color.3 as u32, coverage as u32);
        if a == 0 {
            return;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let inv = 255 - a;
        let dst = &mut self.data[idx..idx + 4];
        // Premultiplied source over premultiplied destination.
        dst[0] = (mul255(color.2 as u32, a) + mul255(dst[0] as u32, inv)) as u8;
        dst[1] = (mul255(color.1 as u32, a) + mul255(dst[1] as u32, inv)) as u8;
        dst[2] = (mul255(color.0 as u32, a) + mul255(dst[2] as u32, inv)) as u8;
        dst[3] = (a + mul255(dst[3] as u32, inv)) as u8;
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
        for row in y.max(0)..(y + h).min(self.height) {
            for col in x.max(0)..(x + w).min(self.width) {
                self.blend(col, row, color, 255);
            }
        }
    }

    /// A one-pixel outline drawn inside the given bounds.
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: i32, h: i32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    #[test]
    fn fill_writes_premultiplied_bgra() {
        let mut buf = canvas(2, 1);
        let mut c = Canvas::new(2, 1, &mut buf);
        // Pure red at half alpha => premultiplied red channel is halved.
        c.fill(Rgba::new(255, 0, 0, 128));
        assert_eq!(buf[0], 0, "blue stays zero");
        assert_eq!(buf[1], 0, "green stays zero");
        assert_eq!(buf[2], 128, "red is premultiplied");
        assert_eq!(buf[3], 128, "alpha preserved");
    }

    #[test]
    fn opaque_blend_replaces_destination() {
        let mut buf = canvas(1, 1);
        let mut c = Canvas::new(1, 1, &mut buf);
        c.fill(Rgba::new(0, 0, 0, 255));
        c.blend(0, 0, Rgba::new(255, 255, 255, 255), 255);
        assert_eq!(&buf[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn zero_coverage_is_a_no_op() {
        let mut buf = canvas(1, 1);
        let mut c = Canvas::new(1, 1, &mut buf);
        c.fill(Rgba::new(10, 20, 30, 255));
        let before = buf.clone();
        let mut c = Canvas::new(1, 1, &mut buf);
        c.blend(0, 0, Rgba::new(255, 255, 255, 255), 0);
        assert_eq!(buf, before);
    }

    #[test]
    fn drawing_outside_bounds_is_clipped_not_panicking() {
        let mut buf = canvas(4, 4);
        let mut c = Canvas::new(4, 4, &mut buf);
        c.blend(-1, 0, Rgba::new(255, 0, 0, 255), 255);
        c.blend(0, 99, Rgba::new(255, 0, 0, 255), 255);
        c.fill_rect(-10, -10, 100, 100, Rgba::new(0, 255, 0, 255));
        c.stroke_rect(2, 2, 10, 10, Rgba::new(0, 0, 255, 255));
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn stroke_rect_touches_only_the_border() {
        let mut buf = canvas(4, 4);
        let mut c = Canvas::new(4, 4, &mut buf);
        c.fill(Rgba::new(0, 0, 0, 255));
        c.stroke_rect(0, 0, 4, 4, Rgba::new(255, 255, 255, 255));
        let px = |x: usize, y: usize| buf[(y * 4 + x) * 4];
        assert_eq!(px(0, 0), 255, "corner is stroked");
        assert_eq!(px(1, 1), 0, "interior is untouched");
        assert_eq!(px(3, 3), 255, "far corner is stroked");
    }

    #[test]
    fn scale_alpha_dims_without_shifting_hue() {
        let c = Rgba::new(200, 100, 50, 255).scale_alpha(128);
        assert_eq!((c.0, c.1, c.2), (200, 100, 50));
        assert_eq!(c.3, 128);
    }
}
