//! Software renderer for the Linux/macOS window: draws the grid into a pixel
//! buffer (0x00RRGGBB) with an embedded font (DejaVu Sans Mono, see
//! assets/fonts/DejaVu-LICENSE.txt).

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use super::{draw_shape, image_runs, GCell, ImageCache, PxRect, Screen, BOLD, CROSSED, DEFAULT_BG, UNDERLINE};

static REGULAR: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
static BOLD_FONT: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono-Bold.ttf");

struct Glyph {
    /// Offset from the cell's top-left corner.
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
}

pub struct Renderer {
    fonts: [Font; 2],
    px: f32,
    pub cell_w: i32,
    pub cell_h: i32,
    ascent: i32,
    cache: HashMap<(char, bool), Option<Glyph>>,
    images: ImageCache,
}

/// A pixel buffer to draw into.
pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Canvas<'_> {
    pub fn fill(&mut self, r: PxRect, color: u32) {
        let x0 = r.left.clamp(0, self.width as i32) as usize;
        let x1 = r.right.clamp(0, self.width as i32) as usize;
        let y0 = r.top.clamp(0, self.height as i32) as usize;
        let y1 = r.bottom.clamp(0, self.height as i32) as usize;
        if x0 >= x1 {
            return;
        }
        for y in y0..y1 {
            self.pixels[y * self.width + x0..y * self.width + x1].fill(color);
        }
    }

    /// Blend `color` onto the pixel with coverage `a` (0-255).
    fn blend(&mut self, x: i32, y: i32, color: u32, a: u8) {
        if a == 0 || x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let p = &mut self.pixels[y as usize * self.width + x as usize];
        if a == 255 {
            *p = color;
            return;
        }
        let (a, bg) = (a as u32, *p);
        let mix = |s: u32| ((((color >> s) & 0xff) * a + ((bg >> s) & 0xff) * (255 - a)) / 255) << s;
        *p = mix(0) | mix(8) | mix(16);
    }
}

impl Renderer {
    /// A renderer for a font height of `px` pixels.
    pub fn new(px: f32) -> Renderer {
        let load =
            |data: &'static [u8]| Font::from_bytes(data, FontSettings { scale: px, ..FontSettings::default() }).expect("embedded font");
        let fonts = [load(REGULAR), load(BOLD_FONT)];
        let (ascent, descent) = match fonts[0].horizontal_line_metrics(px) {
            Some(m) => (m.ascent.ceil() as i32, (-m.descent).ceil() as i32),
            None => ((px * 0.8) as i32, (px * 0.25) as i32),
        };
        let cell_w = fonts[0].metrics('M', px).advance_width.round().max(1.0) as i32;
        Renderer { fonts, px, cell_w, cell_h: (ascent + descent).max(1), ascent, cache: HashMap::new(), images: ImageCache::default() }
    }

    fn glyph(&mut self, c: char, bold: bool) -> Option<&Glyph> {
        let (fonts, px, ascent) = (&self.fonts, self.px, self.ascent);
        self.cache
            .entry((c, bold))
            .or_insert_with(|| {
                // Fall back to the other weight if a glyph is missing in one.
                let font = [&fonts[bold as usize], &fonts[!bold as usize]].into_iter().find(|f| f.lookup_glyph_index(c) != 0)?;
                let (m, coverage) = font.rasterize(c, px);
                Some(Glyph { left: m.xmin, top: ascent - m.ymin - m.height as i32, width: m.width, height: m.height, coverage })
            })
            .as_ref()
    }

    fn draw_text(&mut self, canvas: &mut Canvas, x: i32, y: i32, cell: &GCell, fg: u32) {
        let bold = cell.flags & BOLD != 0;
        for c in cell.sym.chars() {
            let Some(g) = self.glyph(c, bold) else { continue };
            for gy in 0..g.height {
                for gx in 0..g.width {
                    let a = g.coverage[gy * g.width + gx];
                    canvas.blend(x + g.left + gx as i32, y + g.top + gy as i32, fg, a);
                }
            }
        }
    }

    /// Draw one row of the grid.
    pub fn render_row(&mut self, canvas: &mut Canvas, s: &Screen, row: u16) {
        let (cw, ch) = (self.cell_w, self.cell_h);
        let cells = s.row(row);
        let y = row as i32 * ch;
        canvas.fill(PxRect { left: 0, top: y, right: s.cols as i32 * cw, bottom: y + ch }, DEFAULT_BG);
        let mut x = 0;
        while x < cells.len() {
            let c = &cells[x];
            let w = if x + c.width() > cells.len() { 1 } else { c.width() };
            let r = PxRect { left: x as i32 * cw, top: y, right: (x + w) as i32 * cw, bottom: y + ch };
            let (fg, bg) = s.colors(x as u16, row, c);
            canvas.fill(r, bg);
            if let Some(shape) = c.shape() {
                draw_shape(shape, fg, bg, r, &mut |r, color| canvas.fill(r, color));
            } else if c.sym != " " && c.sym != crate::images::IMAGE_CELL {
                self.draw_text(canvas, r.left, y, c, fg);
            }
            let line = (ch / 16).max(1);
            if c.flags & UNDERLINE != 0 {
                canvas.fill(PxRect { top: y + ch - line - 1, bottom: y + ch - 1, ..r }, fg);
            }
            if c.flags & CROSSED != 0 {
                canvas.fill(PxRect { top: y + ch / 2, bottom: y + ch / 2 + line, ..r }, fg);
            }
            x += w;
        }
        // Images in the cells marked for them.
        for (i, x0, x1) in image_runs(s, row) {
            let bg = cells[x0 as usize].bg;
            let block = self.images.block(&s.images[i], row, x0..x1, (cw, ch), bg);
            let w = (x1 - x0) as usize * cw as usize;
            let left = x0 as usize * cw as usize;
            for yy in 0..ch as usize {
                let py = y as usize + yy;
                if py >= canvas.height || left >= canvas.width {
                    continue;
                }
                let n = w.min(canvas.width - left);
                canvas.pixels[py * canvas.width + left..py * canvas.width + left + n].copy_from_slice(&block[yy * w..yy * w + n]);
            }
        }
        // Cursor: a bar at the left of its cell.
        let (cx, cy) = s.cursor;
        if s.cursor_visible && cy == row && cx < s.cols {
            let x = cx as i32 * cw;
            canvas.fill(PxRect { left: x, top: y, right: x + (cw / 4).max(2), bottom: y + ch }, cells[cx as usize].fg);
        }
    }

    /// Draw the dirty rows and the space around the grid.
    pub fn render(&mut self, canvas: &mut Canvas, s: &mut Screen) {
        for row in 0..s.rows {
            if s.dirty[row as usize] {
                self.render_row(canvas, s, row);
                s.dirty[row as usize] = false;
            }
        }
        let (gw, gh) = (s.cols as i32 * self.cell_w, s.rows as i32 * self.cell_h);
        let (w, h) = (canvas.width as i32, canvas.height as i32);
        canvas.fill(PxRect { left: gw, top: 0, right: w, bottom: h }, DEFAULT_BG);
        canvas.fill(PxRect { left: 0, top: gh, right: w, bottom: h }, DEFAULT_BG);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::Backend;
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, BorderType, Paragraph};
    use std::sync::{Arc, Mutex};

    /// Draw a small screen through the GUI backend and check the pixels.
    /// SHOW_GUI=<file.ppm> saves the result as an image.
    #[test]
    fn renders_text_and_borders() {
        let screen = Arc::new(Mutex::new(Screen::new()));
        screen.lock().unwrap().resize(30, 5);
        let backend = super::super::backend(screen.clone(), Arc::new(|| {}));
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| {
            let b = Block::bordered().border_type(BorderType::Rounded).title(" F-Term ");
            let p = Paragraph::new("Hällo ▀▄ → ✓").style(Style::default().fg(Color::Rgb(255, 200, 0)).bg(Color::Blue)).block(b);
            f.render_widget(p, f.area());
        })
        .unwrap();
        term.backend_mut().flush().unwrap();

        let mut r = Renderer::new(16.0);
        let mut s = screen.lock().unwrap();
        let (w, h) = (s.cols as usize * r.cell_w as usize + 3, s.rows as usize * r.cell_h as usize + 2);
        let mut pixels = vec![0x123456; w * h];
        let mut canvas = Canvas { pixels: &mut pixels, width: w, height: h };
        r.render(&mut canvas, &mut s);
        if let Ok(path) = std::env::var("SHOW_GUI") {
            let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
            for p in &pixels {
                ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
            }
            std::fs::write(path, ppm).unwrap();
        }
        assert!(s.dirty.iter().all(|d| !d));
        // Margin cleared, background and text (and border) in their colours.
        assert_eq!(pixels[w - 1], DEFAULT_BG);
        assert!(pixels.contains(&0x0037da));
        assert!(pixels.contains(&0xffc800));
    }

    #[test]
    fn renders_images() {
        // A 4x2 cell area with a 2x1-cell image: left half red, right half
        // half-transparent red.
        let mut r = Renderer::new(16.0);
        let (cw, ch) = (r.cell_w as u32, r.cell_h as u32);
        let (w, h) = (2 * cw, ch);
        let mut rgba = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[255, 0, 0, if x < cw { 255 } else { 128 }]);
            }
        }
        let pixels = std::sync::Arc::new(crate::images::Pixels { width: w, height: h, rgba });
        let mut s = Screen::new();
        s.resize(4, 2);
        for x in 0..2 {
            s.cells[x].sym = crate::images::IMAGE_CELL.into();
        }
        s.images.push(crate::images::Placement { area: ratatui::layout::Rect::new(0, 0, 2, 1), width: w, height: h, pixels });
        let (pw, ph) = (4 * cw as usize, 2 * ch as usize);
        let mut buf = vec![0x123456; pw * ph];
        let mut canvas = Canvas { pixels: &mut buf, width: pw, height: ph };
        r.render(&mut canvas, &mut s);
        assert_eq!(buf[1], 0xff0000, "opaque part");
        assert_eq!(buf[cw as usize + 1], 0x800000, "blended onto the black background");
        assert_eq!(buf[3 * cw as usize], DEFAULT_BG, "cells without the image");
    }
}
