//! Image loading and rendering: ASCII art, colour half-blocks, or real pixels
//! through the kitty / iTerm2 / sixel terminal graphics protocols.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::config::{GraphicsProtocol, ImageMode};

pub fn avatar_url(name: &str) -> String {
    format!("{}/images/avatar/{}.png", crate::config::DEFAULT_STATIC, urlencode(&name.to_lowercase()))
}

pub fn eicon_url(name: &str) -> String {
    format!("{}/images/eicon/{}.gif", crate::config::DEFAULT_STATIC, urlencode(&name.to_lowercase()))
}

pub fn charimage_url(id: &str, ext: &str) -> String {
    format!("{}/images/charimage/{}.{}", crate::config::DEFAULT_STATIC, id, ext)
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

/// Decoded RGBA image.
#[derive(Debug)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// A decoded image: one frame, or the frames of an animated GIF with their
/// display times in milliseconds.
#[derive(Debug)]
pub struct Frames {
    pub frames: Vec<(Arc<Pixels>, u32)>,
    pub total_ms: u32,
}

impl Frames {
    fn still(p: Pixels) -> Frames {
        Frames { frames: vec![(Arc::new(p), 0)], total_ms: 0 }
    }

    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1 && self.total_ms > 0
    }

    /// The frame to show at `ms` milliseconds into the animation.
    pub fn at(&self, ms: u64) -> (usize, &Arc<Pixels>) {
        if !self.is_animated() {
            return (0, &self.frames[0].0);
        }
        let mut t = (ms % self.total_ms as u64) as u32;
        for (i, (p, d)) in self.frames.iter().enumerate() {
            if t < *d {
                return (i, p);
            }
            t -= d;
        }
        (0, &self.frames[0].0)
    }
}

pub enum ImgState {
    Loading,
    Failed(String),
    Ready(Arc<Frames>),
}

#[derive(Debug)]
pub struct Loaded {
    pub url: String,
    pub result: Result<Arc<Frames>, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Kitty,
    Iterm2,
    Sixel,
}

/// Guess which graphics protocol the terminal supports from environment variables.
pub fn detect_protocol() -> Option<Proto> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let term = env("TERM").to_lowercase();
    let prog = env("TERM_PROGRAM").to_lowercase();
    if !env("KITTY_WINDOW_ID").is_empty() || term.contains("kitty") || prog == "ghostty" || term.contains("ghostty") {
        return Some(Proto::Kitty);
    }
    if prog == "iterm.app" || prog == "wezterm" || !env("WEZTERM_EXECUTABLE").is_empty() || prog == "vscode" || prog == "tabby" {
        return Some(Proto::Iterm2);
    }
    if !env("KONSOLE_VERSION").is_empty() {
        return Some(Proto::Kitty);
    }
    if term.contains("foot")
        || term.contains("mlterm")
        || term.contains("contour")
        || term.contains("sixel")
        || !env("WT_SESSION").is_empty()
        || prog == "mintty"
        || !env("XTERM_VERSION").is_empty()
    {
        return Some(Proto::Sixel);
    }
    None
}

// ---- real pixels in the GUI window ----------------------------------------------------

/// Marks the cells covered by an image in the GUI; the window paints the
/// image's pixels there (and nothing where something else covers it).
pub const IMAGE_CELL: &str = "\u{E000}";

/// An image placed on screen for the GUI window to paint.
#[derive(Clone, Debug)]
pub struct Placement {
    /// The cells it covers.
    pub area: Rect,
    /// Its size in pixels (it fits into the cells).
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Pixels>,
}

impl PartialEq for Placement {
    fn eq(&self, o: &Placement) -> bool {
        self.area == o.area && self.width == o.width && self.height == o.height && Arc::ptr_eq(&self.pixels, &o.pixels)
    }
}

/// Cell size in pixels while a GUI window shows images (0 = no window).
static PIXEL_CELL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PLACED: std::sync::Mutex<Vec<Placement>> = std::sync::Mutex::new(Vec::new());

/// Called by the GUI window: images are painted as pixels, with cells of
/// this size. `None` when the window closes.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn set_pixel_output(cell: Option<(u16, u16)>) {
    let v = cell.map(|(w, h)| (w as u32) << 16 | h as u32).unwrap_or(0);
    PIXEL_CELL.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The cell size in pixels if a GUI window paints images.
pub fn pixel_output() -> Option<(u16, u16)> {
    let v = PIXEL_CELL.load(std::sync::atomic::Ordering::Relaxed);
    (v != 0).then_some(((v >> 16) as u16, v as u16))
}

/// The images placed in the frame just drawn.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn placements() -> Vec<Placement> {
    PLACED.lock().map(|p| p.clone()).unwrap_or_default()
}

pub struct ImageStore {
    entries: HashMap<String, ImgState>,
    text_cache: HashMap<(String, u16, u16, u8), Vec<Line<'static>>>,
    native_cache: HashMap<(String, u16, u16), (String, u16, u16)>,
    /// Native images placed during the current frame.
    frame: Vec<(String, Rect)>,
    last_frame: Vec<(String, Rect)>,
    pub cell_px: (u16, u16),
    agent: ureq::Agent,
    /// Play animated GIFs (the `animate_images` setting).
    pub animate: bool,
    /// Animations start counting from here.
    epoch: std::time::Instant,
    /// An animation was drawn in the current / last frame.
    animating: bool,
    pub animating_last_frame: bool,
}

impl ImageStore {
    pub fn new() -> ImageStore {
        ImageStore {
            entries: HashMap::new(),
            text_cache: HashMap::new(),
            native_cache: HashMap::new(),
            frame: Vec::new(),
            last_frame: Vec::new(),
            cell_px: cell_pixels(),
            agent: crate::api::agent(),
            animate: true,
            epoch: std::time::Instant::now(),
            animating: false,
            animating_last_frame: false,
        }
    }

    /// Start loading an image if it isn't known yet. Returns the current state.
    pub fn request<E: Send + 'static>(&mut self, url: &str, tx: &Sender<E>, wrap: fn(Loaded) -> E) -> &ImgState {
        if !self.entries.contains_key(url) {
            self.entries.insert(url.to_string(), ImgState::Loading);
            let url = url.to_string();
            let tx = tx.clone();
            let agent = self.agent.clone();
            std::thread::spawn(move || {
                let result = load(&agent, &url);
                let _ = tx.send(wrap(Loaded { url, result }));
            });
        }
        &self.entries[url]
    }

    pub fn finish(&mut self, loaded: Loaded) {
        let st = match loaded.result {
            Ok(p) => ImgState::Ready(p),
            Err(e) => ImgState::Failed(e),
        };
        self.entries.insert(loaded.url, st);
    }

    pub fn clear_render_cache(&mut self) {
        self.text_cache.clear();
        self.native_cache.clear();
        self.cell_px = cell_pixels();
    }

    /// Draw an image into `area` using the chosen mode. Returns the used size.
    pub fn draw(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        url: &str,
        mode: ImageMode,
        proto: GraphicsProtocol,
        truecolor: bool,
    ) -> (u16, u16) {
        if area.width == 0 || area.height == 0 {
            return (0, 0);
        }
        let frames = match self.entries.get(url) {
            Some(ImgState::Ready(p)) => p.clone(),
            Some(ImgState::Failed(e)) => {
                buf.set_stringn(area.x, area.y, format!("[image: {e}]"), area.width as usize, Style::default().fg(Color::DarkGray));
                return (area.width.min(20), 1);
            }
            _ => {
                buf.set_stringn(area.x, area.y, "[loading image…]", area.width as usize, Style::default().fg(Color::DarkGray));
                return (area.width.min(17), 1);
            }
        };
        let native = match (mode, proto) {
            (ImageMode::Native, GraphicsProtocol::Kitty) => Some(Proto::Kitty),
            (ImageMode::Native, GraphicsProtocol::Iterm2) => Some(Proto::Iterm2),
            (ImageMode::Native, GraphicsProtocol::Sixel) => Some(Proto::Sixel),
            (ImageMode::Native, GraphicsProtocol::Auto) => detect_protocol(),
            _ => None,
        };
        let gui = pixel_output().filter(|_| mode != ImageMode::Off);
        // Native terminal images stay still: replacing them forces full redraws.
        let (frame, px) = if self.animate && frames.is_animated() && (gui.is_some() || native.is_none()) {
            self.animating = true;
            let (i, p) = frames.at(self.epoch.elapsed().as_millis() as u64);
            (i, p.clone())
        } else {
            (0, frames.frames[0].0.clone())
        };
        if let Some(cell) = gui {
            return place_pixels(buf, area, px, cell);
        }
        if let Some(p) = native {
            let key = (url.to_string(), area.width, area.height);
            if !self.native_cache.contains_key(&key) {
                let enc = encode_native(&px, area.width, area.height, self.cell_px, p);
                self.native_cache.insert(key.clone(), enc);
            }
            let (seq, cols, rows) = self.native_cache[&key].clone();
            if cols == 0 || rows == 0 {
                return (0, 0);
            }
            let used = Rect::new(area.x, area.y, cols.min(area.width), rows.min(area.height));
            for y in used.top()..used.bottom() {
                for x in used.left()..used.right() {
                    buf[(x, y)].set_skip(true);
                }
            }
            let cell = &mut buf[(used.x, used.y)];
            cell.set_skip(false);
            cell.set_symbol(&seq);
            self.frame.push((url.to_string(), used));
            return (used.width, used.height);
        }
        let text_mode = match mode {
            ImageMode::Ascii => 0u8,
            ImageMode::ColorAscii => 1,
            _ => 2,
        };
        let key = (format!("{url}#{frame}"), area.width, area.height, text_mode + if truecolor { 0 } else { 10 });
        if !self.text_cache.contains_key(&key) {
            if self.text_cache.len() > 4000 {
                self.text_cache.clear();
            }
            let lines = match text_mode {
                0 => render_ascii(&px, area.width, area.height, false, truecolor),
                1 => render_ascii(&px, area.width, area.height, true, truecolor),
                _ => render_blocks(&px, area.width, area.height, truecolor),
            };
            self.text_cache.insert(key.clone(), lines);
        }
        let lines = &self.text_cache[&key];
        let mut w = 0;
        for (i, line) in lines.iter().enumerate().take(area.height as usize) {
            let (x, _) = buf.set_line(area.x, area.y + i as u16, line, area.width);
            w = w.max(x - area.x);
        }
        (w, lines.len().min(area.height as usize) as u16)
    }

    /// Call before drawing a frame.
    pub fn begin_frame(&mut self) {
        self.frame.clear();
        self.animating = false;
        if let Ok(mut p) = PLACED.lock() {
            p.clear();
        }
    }

    /// Call after drawing. Returns true when native images moved or disappeared,
    /// in which case the screen must be fully redrawn.
    pub fn end_frame(&mut self) -> bool {
        self.animating_last_frame = self.animating;
        let changed = self.frame != self.last_frame;
        let had_any = !self.last_frame.is_empty();
        self.last_frame = std::mem::take(&mut self.frame);
        changed && (had_any || !self.last_frame.is_empty())
    }
}

/// Put an image into `area` for the GUI window: scaled down to fit (never
/// up), the covered cells marked with `IMAGE_CELL`. Returns the used size.
fn place_pixels(buf: &mut Buffer, area: Rect, px: Arc<Pixels>, (cw, ch): (u16, u16)) -> (u16, u16) {
    let (cw, ch) = (cw.max(1) as f64, ch.max(1) as f64);
    let scale = (area.width as f64 * cw / px.width.max(1) as f64).min(area.height as f64 * ch / px.height.max(1) as f64).min(1.0);
    let width = ((px.width as f64 * scale).round() as u32).max(1);
    let height = ((px.height as f64 * scale).round() as u32).max(1);
    let cols = ((width as f64 / cw).ceil() as u16).clamp(1, area.width);
    let rows = ((height as f64 / ch).ceil() as u16).clamp(1, area.height);
    let used = Rect::new(area.x, area.y, cols, rows).intersection(buf.area);
    for y in used.top()..used.bottom() {
        for x in used.left()..used.right() {
            buf[(x, y)].reset();
            buf[(x, y)].set_symbol(IMAGE_CELL);
        }
    }
    if let Ok(mut p) = PLACED.lock() {
        p.push(Placement { area: used, width, height, pixels: px });
    }
    (used.width, used.height)
}

/// Size in pixels of one terminal cell.
fn cell_pixels() -> (u16, u16) {
    if let Ok(ws) = crossterm::terminal::window_size() {
        if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 {
            return ((ws.width / ws.columns).max(1), (ws.height / ws.rows).max(1));
        }
    }
    (8, 16)
}

fn cache_path(url: &str) -> std::path::PathBuf {
    // FNV-1a hash of the URL as the file name.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    crate::paths::cache_dir().join("images").join(format!("{h:016x}"))
}

fn load(agent: &ureq::Agent, url: &str) -> Result<Arc<Frames>, String> {
    let path = cache_path(url);
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|e| e.as_secs() < 3 * 24 * 3600).unwrap_or(false))
        .unwrap_or(false);
    let bytes = match (fresh, std::fs::read(&path)) {
        (true, Ok(b)) => b,
        _ => {
            let b = crate::api::get_bytes(agent, url, 16 * 1024 * 1024)?;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, &b);
            b
        }
    };
    decode(&bytes).map(Arc::new)
}

/// Animations bigger than this (all frames, decoded) only show their first frame.
#[cfg(feature = "images")]
const MAX_ANIMATION_BYTES: usize = 48 * 1024 * 1024;

#[cfg(feature = "images")]
fn decode(bytes: &[u8]) -> Result<Frames, String> {
    if bytes.starts_with(b"GIF8") {
        if let Some(f) = decode_gif(bytes) {
            return Ok(f);
        }
    }
    let img = image::load_from_memory(bytes).map_err(|e| format!("cannot decode: {e}"))?;
    // Keep memory bounded for huge profile pictures.
    let img = if img.width() > 1024 || img.height() > 1024 { img.thumbnail(1024, 1024) } else { img };
    let rgba = img.to_rgba8();
    Ok(Frames::still(Pixels { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }))
}

/// All frames of an animated GIF, or None to decode it as a still image.
#[cfg(feature = "images")]
fn decode_gif(bytes: &[u8]) -> Option<Frames> {
    use image::AnimationDecoder;
    let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let mut frames = Vec::new();
    let mut size = 0usize;
    for f in dec.into_frames() {
        let f = f.ok()?;
        let (num, den) = f.delay().numer_denom_ms();
        // Like browsers: very short delays play at 100 ms.
        let ms = num.checked_div(den).unwrap_or(100);
        let ms = if ms < 20 { 100 } else { ms.min(10_000) };
        let buf = f.into_buffer();
        size += buf.as_raw().len();
        if size > MAX_ANIMATION_BYTES || buf.width() > 1024 || buf.height() > 1024 {
            return None;
        }
        frames.push((Arc::new(Pixels { width: buf.width(), height: buf.height(), rgba: buf.into_raw() }), ms));
    }
    if frames.len() < 2 {
        return None;
    }
    let total_ms = frames.iter().map(|f| f.1).sum();
    Some(Frames { frames, total_ms })
}

#[cfg(not(feature = "images"))]
fn decode(_bytes: &[u8]) -> Result<Frames, String> {
    Err("built without image support".into())
}

/// Nearest-neighbour/box resample to exactly w x h.
pub fn resample(p: &Pixels, w: u32, h: u32) -> Pixels {
    let w = w.max(1);
    let h = h.max(1);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let y0 = (y as u64 * p.height as u64 / h as u64) as u32;
        let y1 = (((y + 1) as u64 * p.height as u64 / h as u64) as u32).max(y0 + 1).min(p.height);
        for x in 0..w {
            let x0 = (x as u64 * p.width as u64 / w as u64) as u32;
            let x1 = (((x + 1) as u64 * p.width as u64 / w as u64) as u32).max(x0 + 1).min(p.width);
            // Average a few samples of the source box (max 4x4) for smoother results.
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            let sy = ((y1 - y0) / 4).max(1);
            let sx = ((x1 - x0) / 4).max(1);
            let mut yy = y0;
            while yy < y1 {
                let mut xx = x0;
                while xx < x1 {
                    let c = p.get(xx, yy);
                    let alpha = c[3] as u32;
                    r += c[0] as u32 * alpha;
                    g += c[1] as u32 * alpha;
                    b += c[2] as u32 * alpha;
                    a += alpha;
                    n += 1;
                    xx += sx;
                }
                yy += sy;
            }
            match r.checked_div(a) {
                None => out.extend_from_slice(&[0, 0, 0, 0]),
                Some(r) => out.extend_from_slice(&[r as u8, (g / a) as u8, (b / a) as u8, (a / n.max(1)) as u8]),
            }
        }
    }
    Pixels { width: w, height: h, rgba: out }
}

/// Fit an image into cols x rows cells where a cell is `cw` x `ch` "pixels".
fn fit(p: &Pixels, cols: u16, rows: u16, cw: f64, ch: f64) -> (u16, u16) {
    let scale = (cols as f64 * cw / p.width as f64).min(rows as f64 * ch / p.height as f64);
    let c = ((p.width as f64 * scale / cw).round() as u16).clamp(1, cols);
    let r = ((p.height as f64 * scale / ch).round() as u16).clamp(1, rows);
    (c, r)
}

pub fn rgb_color(r: u8, g: u8, b: u8, truecolor: bool) -> Color {
    if truecolor {
        return Color::Rgb(r, g, b);
    }
    // xterm 256-colour approximation.
    let q = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let (qr, qg, qb) = (q(r), q(g), q(b));
    let cube = 16 + 36 * qr + 6 * qg + qb;
    let lv = [0u8, 95, 135, 175, 215, 255];
    let cube_err = (lv[qr as usize] as i32 - r as i32).pow(2)
        + (lv[qg as usize] as i32 - g as i32).pow(2)
        + (lv[qb as usize] as i32 - b as i32).pow(2);
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gi = if avg > 238 { 23 } else { (avg.saturating_sub(3)) / 10 } as u8;
    let gv = 8 + 10 * gi as i32;
    let gray_err = (gv - r as i32).pow(2) + (gv - g as i32).pow(2) + (gv - b as i32).pow(2);
    Color::Indexed(if gray_err < cube_err { 232 + gi } else { cube })
}

pub fn render_blocks(p: &Pixels, max_cols: u16, max_rows: u16, truecolor: bool) -> Vec<Line<'static>> {
    let (cols, rows) = fit(p, max_cols, max_rows, 1.0, 2.0);
    let img = resample(p, cols as u32, rows as u32 * 2);
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows as u32 {
        let mut spans = Vec::with_capacity(cols as usize);
        for x in 0..cols as u32 {
            let top = img.get(x, row * 2);
            let bot = img.get(x, row * 2 + 1);
            let (t, b) = (top[3] >= 128, bot[3] >= 128);
            let span = match (t, b) {
                (true, true) => Span::styled(
                    "▀",
                    Style::default().fg(rgb_color(top[0], top[1], top[2], truecolor)).bg(rgb_color(bot[0], bot[1], bot[2], truecolor)),
                ),
                (true, false) => Span::styled("▀", Style::default().fg(rgb_color(top[0], top[1], top[2], truecolor))),
                (false, true) => Span::styled("▄", Style::default().fg(rgb_color(bot[0], bot[1], bot[2], truecolor))),
                (false, false) => Span::raw(" "),
            };
            spans.push(span);
        }
        lines.push(Line::from(spans));
    }
    lines
}

const RAMP: &[u8] = b" .:-=+*#%@";

pub fn render_ascii(p: &Pixels, max_cols: u16, max_rows: u16, color: bool, truecolor: bool) -> Vec<Line<'static>> {
    let (cols, rows) = fit(p, max_cols, max_rows, 1.0, 2.0);
    let img = resample(p, cols as u32, rows as u32);
    let mut lines = Vec::with_capacity(rows as usize);
    for y in 0..rows as u32 {
        if color {
            let mut spans = Vec::new();
            for x in 0..cols as u32 {
                let c = img.get(x, y);
                if c[3] < 128 {
                    spans.push(Span::raw(" "));
                    continue;
                }
                let lum = (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0;
                let ch = RAMP[((lum * (RAMP.len() - 1) as f32).round() as usize).clamp(1, RAMP.len() - 1)] as char;
                spans.push(Span::styled(ch.to_string(), Style::default().fg(rgb_color(c[0], c[1], c[2], truecolor))));
            }
            lines.push(Line::from(spans));
        } else {
            let mut s = String::with_capacity(cols as usize);
            for x in 0..cols as u32 {
                let c = img.get(x, y);
                let a = c[3] as f32 / 255.0;
                let lum = a * (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0;
                s.push(RAMP[(lum * (RAMP.len() - 1) as f32).round() as usize] as char);
            }
            lines.push(Line::from(s));
        }
    }
    lines
}

/// Encode for a native protocol. Returns (escape sequence, cols, rows).
fn encode_native(p: &Pixels, max_cols: u16, max_rows: u16, cell: (u16, u16), proto: Proto) -> (String, u16, u16) {
    let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
    let (cols, rows) = fit(p, max_cols, max_rows, cw, ch);
    let wpx = (cols as f64 * cw) as u32;
    let hpx = (rows as f64 * ch) as u32;
    let img = resample(p, wpx.min(p.width.max(1) * 4), hpx.min(p.height.max(1) * 4));
    let seq = match proto {
        Proto::Kitty => kitty(&img, cols, rows),
        Proto::Iterm2 => iterm2(&img, cols, rows),
        Proto::Sixel => sixel(&resample(p, wpx, hpx)),
    };
    (seq, cols, rows)
}

fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn kitty(img: &Pixels, cols: u16, rows: u16) -> String {
    let data = b64(&img.rgba);
    let mut out = String::new();
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < chunks.len() { 1 } else { 0 };
        let chunk = std::str::from_utf8(chunk).unwrap_or("");
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Ga=T,f=32,s={},v={},c={},r={},C=1,q=2,m={};{}\x1b\\",
                img.width, img.height, cols, rows, more, chunk
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={};{}\x1b\\", more, chunk));
        }
    }
    out
}

/// Minimal uncompressed (stored) PNG encoder for iTerm2.
fn png(img: &Pixels) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn adler(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &d in data {
            a = (a + d as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((img.width * img.height * 4 + img.height) as usize);
    for y in 0..img.height as usize {
        raw.push(0);
        let w = img.width as usize * 4;
        raw.extend_from_slice(&img.rgba[y * w..(y + 1) * w]);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, blk) in blocks.iter().enumerate() {
        z.push(if i + 1 == blocks.len() { 1 } else { 0 });
        z.extend_from_slice(&(blk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(blk.len() as u16)).to_le_bytes());
        z.extend_from_slice(blk);
    }
    z.extend_from_slice(&adler(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn iterm2(img: &Pixels, cols: u16, rows: u16) -> String {
    let data = png(img);
    format!("\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0:{}\x07", data.len(), cols, rows, b64(&data))
}

fn sixel(img: &Pixels) -> String {
    // 6x6x6 colour cube palette; index 0..216.
    let lv = |v: u8| ((v as u32 * 5 + 127) / 255) as usize;
    let idx: Vec<Option<usize>> = (0..img.width * img.height)
        .map(|i| {
            let c = &img.rgba[(i * 4) as usize..(i * 4 + 4) as usize];
            if c[3] < 128 {
                None
            } else {
                Some(lv(c[0]) * 36 + lv(c[1]) * 6 + lv(c[2]))
            }
        })
        .collect();
    let mut used = [false; 216];
    for i in idx.iter().flatten() {
        used[*i] = true;
    }
    let mut out = format!("\x1bP0;1;0q\"1;1;{};{}", img.width, img.height);
    for (i, u) in used.iter().enumerate() {
        if *u {
            let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
            out.push_str(&format!("#{};2;{};{};{}", i, r * 20, g * 20, b * 20));
        }
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let mut band = 0;
    while band < h {
        let mut colors_in_band: Vec<usize> = Vec::new();
        for y in band..(band + 6).min(h) {
            for x in 0..w {
                if let Some(c) = idx[y * w + x] {
                    if !colors_in_band.contains(&c) {
                        colors_in_band.push(c);
                    }
                }
            }
        }
        for (ci, &c) in colors_in_band.iter().enumerate() {
            out.push_str(&format!("#{c}"));
            let mut run_char = 0u8;
            let mut run_len = 0usize;
            let flush = |out: &mut String, ch: u8, n: usize| {
                if n == 0 {
                    return;
                }
                if n > 3 {
                    out.push_str(&format!("!{}{}", n, ch as char));
                } else {
                    for _ in 0..n {
                        out.push(ch as char);
                    }
                }
            };
            for x in 0..w {
                let mut bits = 0u8;
                for dy in 0..6 {
                    let y = band + dy;
                    if y < h && idx[y * w + x] == Some(c) {
                        bits |= 1 << dy;
                    }
                }
                let ch = 63 + bits;
                if ch == run_char {
                    run_len += 1;
                } else {
                    flush(&mut out, run_char, run_len);
                    run_char = ch;
                    run_len = 1;
                }
            }
            flush(&mut out, run_char, run_len);
            if ci + 1 < colors_in_band.len() {
                out.push('$');
            }
        }
        out.push('-');
        band += 6;
    }
    out.push_str("\x1b\\");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_pixels_for_gui() {
        // 100x50 pixels into 20x10 cells of 8x16: fits unscaled in 13x4 cells.
        let px = Arc::new(Pixels { width: 100, height: 50, rgba: vec![0; 100 * 50 * 4] });
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 12));
        let used = place_pixels(&mut buf, Rect::new(2, 1, 20, 10), px.clone(), (8, 16));
        assert_eq!(used, (13, 4));
        assert_eq!(buf[(2, 1)].symbol(), IMAGE_CELL);
        assert_eq!(buf[(14, 4)].symbol(), IMAGE_CELL);
        assert_eq!(buf[(15, 1)].symbol(), " ");
        // Too big: scaled down to the area, keeping the aspect ratio.
        let big = Arc::new(Pixels { width: 1000, height: 1000, rgba: vec![0; 1000 * 1000 * 4] });
        let used = place_pixels(&mut buf, Rect::new(0, 0, 10, 4), big, (8, 16));
        assert_eq!(used, (8, 4));
    }
    #[cfg(feature = "images")]
    #[test]
    fn animated_gif() {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba, RgbaImage};
        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut bytes);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for (c, ms) in [([255, 0, 0, 255], 200), ([0, 0, 255, 255], 0)] {
                let img = RgbaImage::from_pixel(4, 4, Rgba(c));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(ms, 1))).unwrap();
            }
        }
        let f = decode(&bytes).unwrap();
        assert!(f.is_animated());
        // A delay of 0 plays at 100 ms, like in browsers.
        assert_eq!(f.frames.iter().map(|x| x.1).collect::<Vec<_>>(), vec![200, 100]);
        assert_eq!(f.total_ms, 300);
        assert_eq!(f.at(0).0, 0);
        assert_eq!(f.at(199).0, 0);
        assert_eq!(f.at(250).0, 1);
        assert_eq!(f.at(310).0, 0);
        assert_eq!(f.at(250).1.get(0, 0), [0, 0, 255, 255]);

        // Drawn in blocks mode, the frame changes with time.
        let mut store = ImageStore::new();
        store.entries.insert("x".into(), ImgState::Ready(Arc::new(f)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        store.begin_frame();
        store.draw(&mut buf, Rect::new(0, 0, 4, 2), "x", ImageMode::Blocks, GraphicsProtocol::Auto, true);
        store.end_frame();
        assert!(store.animating_last_frame);
        store.epoch = std::time::Instant::now();
        store.draw(&mut buf, Rect::new(0, 0, 4, 2), "x", ImageMode::Blocks, GraphicsProtocol::Auto, true);
        let first = buf[(0, 0)].fg;
        // 250 ms into the animation: the second frame.
        store.epoch = std::time::Instant::now() - std::time::Duration::from_millis(250);
        store.draw(&mut buf, Rect::new(0, 0, 4, 2), "x", ImageMode::Blocks, GraphicsProtocol::Auto, true);
        assert_ne!(buf[(0, 0)].fg, first);
    }

    fn checker() -> Pixels {
        let mut rgba = Vec::new();
        for y in 0..8u32 {
            for x in 0..8u32 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Pixels { width: 8, height: 8, rgba }
    }
    #[test]
    fn renders() {
        let p = checker();
        assert_eq!(render_blocks(&p, 8, 4, true).len(), 4);
        assert_eq!(render_ascii(&p, 8, 4, false, true).len(), 4);
        let s = sixel(&p);
        assert!(s.starts_with("\x1bP") && s.ends_with("\x1b\\"));
        let png = png(&p);
        assert_eq!(&png[1..4], b"PNG");
        assert!(kitty(&p, 2, 1).starts_with("\x1b_G"));
        assert!(matches!(rgb_color(255, 0, 0, false), Color::Indexed(196)));
    }
}
