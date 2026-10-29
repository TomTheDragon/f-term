//! GUI window on Windows (95 to 11), drawn with GDI.
//!
//! Only ANSI ("A") window functions and ExtTextOutW are used, all of which
//! Windows 95 has. Text is drawn with the system fonts (Lucida Console or
//! Courier New), box-drawing and block characters as shapes.

use std::cell::RefCell;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{
    backend, channel, draw_shape, events, image_runs, lock, set_bell, GuiBackend, GuiConfig, GuiEvents, ImageCache, PxRect, Screen,
    Selector, Shared, BOLD, CROSSED, DEFAULT_BG, ITALIC, UNDERLINE,
};
use crate::config::Settings;

// ---- Win32 declarations ---------------------------------------------------------

type Handle = *mut core::ffi::c_void;
type WndProc = unsafe extern "system" fn(Handle, u32, usize, isize) -> isize;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct SizeW {
    cx: i32,
    cy: i32,
}

#[repr(C)]
struct WndClassA {
    style: u32,
    wnd_proc: WndProc,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u8,
    class_name: *const u8,
}

#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct TextMetricA {
    ints: [i32; 11], // tmHeight first
    bytes: [u8; 9],
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassA(c: *const WndClassA) -> u16;
    fn CreateWindowExA(
        ex: u32,
        class: *const u8,
        title: *const u8,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        inst: Handle,
        param: Handle,
    ) -> Handle;
    fn DefWindowProcA(h: Handle, m: u32, w: usize, l: isize) -> isize;
    fn GetMessageA(m: *mut Msg, h: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(m: *const Msg) -> i32;
    fn DispatchMessageA(m: *const Msg) -> isize;
    fn PostMessageA(h: Handle, m: u32, w: usize, l: isize) -> i32;
    fn PostQuitMessage(code: i32);
    fn ShowWindow(h: Handle, cmd: i32) -> i32;
    fn UpdateWindow(h: Handle) -> i32;
    fn DestroyWindow(h: Handle) -> i32;
    fn BeginPaint(h: Handle, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(h: Handle, ps: *const PaintStruct) -> i32;
    fn InvalidateRect(h: Handle, r: *const Rect, erase: i32) -> i32;
    fn GetClientRect(h: Handle, r: *mut Rect) -> i32;
    fn LoadCursorA(inst: Handle, name: *const u8) -> Handle;
    fn LoadIconA(inst: Handle, name: *const u8) -> Handle;
    fn AdjustWindowRect(r: *mut Rect, style: u32, menu: i32) -> i32;
    fn SystemParametersInfoA(action: u32, p1: u32, p2: *mut Rect, ini: u32) -> i32;
    fn GetKeyState(vk: i32) -> i16;
    fn SetCapture(h: Handle) -> Handle;
    fn ReleaseCapture() -> i32;
    fn ScreenToClient(h: Handle, p: *mut Point) -> i32;
    fn OpenClipboard(h: Handle) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, mem: Handle) -> Handle;
    fn MessageBoxA(h: Handle, text: *const u8, caption: *const u8, flags: u32) -> i32;
    fn MessageBeep(kind: u32) -> i32;
    fn FlashWindow(h: Handle, invert: i32) -> i32;
    fn GetDC(h: Handle) -> Handle;
    fn ReleaseDC(h: Handle, dc: Handle) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateFontA(
        h: i32,
        w: i32,
        esc: i32,
        orient: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out_prec: u32,
        clip_prec: u32,
        quality: u32,
        pitch: u32,
        face: *const u8,
    ) -> Handle;
    fn SelectObject(dc: Handle, obj: Handle) -> Handle;
    fn DeleteObject(obj: Handle) -> i32;
    fn GetTextMetricsA(dc: Handle, tm: *mut TextMetricA) -> i32;
    fn GetTextExtentPoint32A(dc: Handle, s: *const u8, n: i32, size: *mut SizeW) -> i32;
    fn GetTextFaceA(dc: Handle, n: i32, out: *mut u8) -> i32;
    fn ExtTextOutW(dc: Handle, x: i32, y: i32, opts: u32, r: *const Rect, s: *const u16, n: u32, dx: *const i32) -> i32;
    fn ExtTextOutA(dc: Handle, x: i32, y: i32, opts: u32, r: *const Rect, s: *const u8, n: u32, dx: *const i32) -> i32;
    fn SetTextColor(dc: Handle, c: u32) -> u32;
    fn SetBkColor(dc: Handle, c: u32) -> u32;
    fn SetBkMode(dc: Handle, mode: i32) -> i32;
    fn CreateCompatibleDC(dc: Handle) -> Handle;
    fn CreateCompatibleBitmap(dc: Handle, w: i32, h: i32) -> Handle;
    fn DeleteDC(dc: Handle) -> i32;
    fn BitBlt(dst: Handle, x: i32, y: i32, w: i32, h: i32, src: Handle, sx: i32, sy: i32, rop: u32) -> i32;
    fn SetDIBitsToDevice(
        dc: Handle,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        sx: i32,
        sy: i32,
        start: u32,
        lines: u32,
        bits: *const u8,
        info: *const u8,
        usage: u32,
    ) -> i32;
    fn GetDIBits(dc: Handle, bmp: Handle, start: u32, lines: u32, bits: *mut u8, info: *mut u8, usage: u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GlobalLock(h: Handle) -> *mut u8;
    fn GlobalUnlock(h: Handle) -> i32;
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalFree(h: Handle) -> Handle;
    fn WideCharToMultiByte(cp: u32, flags: u32, s: *const u16, n: i32, out: *mut u8, len: i32, def: *const u8, used: *mut i32) -> i32;
    fn GetVersion() -> u32;
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, out: *mut u16, len: i32) -> i32;
    fn IsDBCSLeadByte(c: u8) -> i32;
    fn AllocConsole() -> i32;
    fn lstrlenA(s: *const u8) -> i32;
}

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020A;
/// Sent by the backend: repaint the rows that changed.
const WM_APP_REDRAW: u32 = 0x8000;
/// Sent when the app ends: close the window.
const WM_APP_QUIT: u32 = 0x8001;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const ETO_OPAQUE: u32 = 2;
const ETO_CLIPPED: u32 = 4;
const SRCCOPY: u32 = 0x00CC_0020;
const CF_TEXT: u32 = 1;
const CF_UNICODETEXT: u32 = 13;

/// The open window, for message boxes.
static HWND: AtomicIsize = AtomicIsize::new(0);

/// Show an error in a message box (the GUI build has no console).
pub fn message_box(text: &str) {
    // The A functions take the ANSI code page; keep it to plain ASCII.
    let mut t: Vec<u8> = text.chars().map(|c| if c.is_ascii() && c != '\0' { c as u8 } else { b'?' }).collect();
    t.push(0);
    unsafe {
        MessageBoxA(HWND.load(Ordering::Relaxed) as Handle, t.as_ptr(), c"F-Term".as_ptr().cast(), 0x10 /* MB_ICONERROR */);
    }
}

/// The GUI build has no console; open one for command line output.
/// Dropping it waits for Enter, so the output can be read before it closes.
pub struct CliConsole;

impl CliConsole {
    pub fn open() -> CliConsole {
        unsafe {
            AllocConsole();
        }
        CliConsole
    }
}

impl Drop for CliConsole {
    fn drop(&mut self) {
        println!("\nPress Enter to close this window.");
        let _ = io::stdin().read_line(&mut String::new());
    }
}

/// Open the window and run `app` with the backend to draw with and the
/// window's input events. The window lives on its own thread.
pub fn run(settings: &Settings, app: impl FnOnce(GuiBackend, GuiEvents) -> io::Result<()>) -> io::Result<()> {
    let config = GuiConfig::new(settings);
    let screen: Shared = Arc::new(Mutex::new(Screen::new()));
    let (tx, rx) = channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let shared = screen.clone();
    let thread = std::thread::Builder::new().name("window".into()).spawn(move || window_thread(shared, tx, config, ready_tx))?;
    let hwnd = match ready_rx.recv() {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return Err(io::Error::other(e)),
        Err(_) => return Err(io::Error::other("the window thread ended")),
    };
    HWND.store(hwnd, Ordering::Relaxed);
    set_bell(Some(Box::new(move || unsafe {
        MessageBeep(0);
        FlashWindow(hwnd as Handle, 1);
    })));
    let wake = Arc::new(move || unsafe {
        PostMessageA(hwnd as Handle, WM_APP_REDRAW, 0, 0);
    });
    let close = Box::new(move || {
        HWND.store(0, Ordering::Relaxed);
        unsafe {
            PostMessageA(hwnd as Handle, WM_APP_QUIT, 0, 0);
        }
        let _ = thread.join();
    });
    app(backend(screen, wake), events(rx, close))
}

// ---- window thread ----------------------------------------------------------------

struct WinState {
    screen: Shared,
    tx: Sender<Event>,
    /// Regular, bold, italic, bold italic.
    fonts: [Handle; 4],
    cell_w: i32,
    cell_h: i32,
    /// Off-screen bitmap to paint into, to avoid flicker.
    mem_dc: Handle,
    mem_bmp: Handle,
    mem_size: (i32, i32),
    last_mouse: (u16, u16),
    /// First byte of a double-byte character typed in a DBCS code page.
    dbcs_lead: Option<u8>,
    /// Text selection with the mouse.
    selector: Selector,
    /// Scaled images (painting only has `&self`).
    images: RefCell<ImageCache>,
}

thread_local! {
    static STATE: RefCell<Option<WinState>> = const { RefCell::new(None) };
}

fn create_font(face: &str, size: i32, bold: bool, italic: bool) -> Handle {
    let mut name: Vec<u8> = face.bytes().take(31).collect();
    name.push(0);
    unsafe {
        CreateFontA(
            -size,
            0,
            0,
            0,
            if bold { 700 } else { 400 },
            italic as u32,
            0,
            0,
            1,    // DEFAULT_CHARSET
            0,    // OUT_DEFAULT_PRECIS
            0,    // CLIP_DEFAULT_PRECIS
            0,    // DEFAULT_QUALITY
            0x31, // FIXED_PITCH | FF_MODERN
            name.as_ptr(),
        )
    }
}

/// Pick the first installed font of the candidates and measure its cell size.
fn load_fonts(choice: &GuiConfig) -> ([Handle; 4], i32, i32) {
    let mut candidates: Vec<&str> = Vec::new();
    if !choice.font.is_empty() {
        candidates.push(&choice.font);
    }
    candidates.extend(["Lucida Console", "Courier New"]);
    unsafe {
        let dc = GetDC(null_mut());
        let mut face = candidates[candidates.len() - 1];
        for c in &candidates {
            let f = create_font(c, choice.font_size as i32, false, false);
            let old = SelectObject(dc, f);
            let mut got = [0u8; 64];
            let n = GetTextFaceA(dc, got.len() as i32, got.as_mut_ptr()).max(0) as usize;
            SelectObject(dc, old);
            DeleteObject(f);
            if String::from_utf8_lossy(&got[..n.min(got.len())]).trim_end_matches('\0').eq_ignore_ascii_case(c) {
                face = c;
                break;
            }
        }
        let fonts = [
            create_font(face, choice.font_size as i32, false, false),
            create_font(face, choice.font_size as i32, true, false),
            create_font(face, choice.font_size as i32, false, true),
            create_font(face, choice.font_size as i32, true, true),
        ];
        let old = SelectObject(dc, fonts[0]);
        let mut tm: TextMetricA = std::mem::zeroed();
        GetTextMetricsA(dc, &mut tm);
        let mut sz = SizeW::default();
        GetTextExtentPoint32A(dc, c"MMMMMMMMMM".as_ptr().cast(), 10, &mut sz);
        SelectObject(dc, old);
        ReleaseDC(null_mut(), dc);
        let cell_h = tm.ints[0].max(4);
        let cell_w = ((sz.cx + 5) / 10).max(2);
        (fonts, cell_w, cell_h)
    }
}

fn window_thread(screen: Shared, tx: Sender<Event>, choice: GuiConfig, ready: Sender<Result<isize, String>>) {
    unsafe {
        let inst = GetModuleHandleA(null());
        let class = c"FTermWindow".as_ptr().cast::<u8>();
        let wc = WndClassA {
            style: 0,
            wnd_proc: wnd_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance: inst,
            icon: LoadIconA(null_mut(), 32512 as *const u8),     // IDI_APPLICATION
            cursor: LoadCursorA(null_mut(), 32513 as *const u8), // IDC_IBEAM
            background: null_mut(),
            menu_name: null(),
            class_name: class,
        };
        if RegisterClassA(&wc) == 0 {
            let _ = ready.send(Err("could not register the window class".into()));
            return;
        }
        let (fonts, cell_w, cell_h) = load_fonts(&choice);
        // Images are painted as pixels in this window.
        crate::images::set_pixel_output(Some((cell_w as u16, cell_h as u16)));

        // Window size for the wanted columns and rows, but no larger than the screen.
        let mut r = Rect { left: 0, top: 0, right: choice.cols as i32 * cell_w, bottom: choice.rows as i32 * cell_h };
        AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, 0);
        let (mut w, mut h) = (r.right - r.left, r.bottom - r.top);
        let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
        let mut work = Rect::default();
        if SystemParametersInfoA(0x30 /* SPI_GETWORKAREA */, 0, &mut work, 0) != 0 && work.right > work.left {
            if w > work.right - work.left || h > work.bottom - work.top {
                w = w.min(work.right - work.left);
                h = h.min(work.bottom - work.top);
                x = work.left;
                y = work.top;
            }
        }
        let hwnd =
            CreateWindowExA(0, class, c"F-Term".as_ptr().cast(), WS_OVERLAPPEDWINDOW, x, y, w, h, null_mut(), null_mut(), inst, null_mut());
        if hwnd.is_null() {
            let _ = ready.send(Err("could not create the window".into()));
            return;
        }
        let mut client = Rect::default();
        GetClientRect(hwnd, &mut client);
        {
            let mut s = lock(&screen);
            s.cell_w = cell_w;
            s.cell_h = cell_h;
            s.resize(((client.right / cell_w).max(1)) as u16, ((client.bottom / cell_h).max(1)) as u16);
        }
        STATE.with(|st| {
            *st.borrow_mut() = Some(WinState {
                screen,
                tx,
                fonts,
                cell_w,
                cell_h,
                mem_dc: null_mut(),
                mem_bmp: null_mut(),
                mem_size: (0, 0),
                last_mouse: (u16::MAX, u16::MAX),
                dbcs_lead: None,
                images: RefCell::new(ImageCache::default()),
                selector: Selector::default(),
            })
        });
        let _ = ready.send(Ok(hwnd as isize));
        ShowWindow(hwnd, 1 /* SW_SHOWNORMAL */);
        UpdateWindow(hwnd);

        let mut msg: Msg = std::mem::zeroed();
        while GetMessageA(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageA(&msg);
        }

        // Dropping the state closes the event channel.
        if let Some(st) = STATE.with(|st| st.borrow_mut().take()) {
            for f in st.fonts {
                DeleteObject(f);
            }
            if !st.mem_dc.is_null() {
                DeleteDC(st.mem_dc);
                DeleteObject(st.mem_bmp);
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: Handle, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        // Everything is painted in WM_PAINT; erasing first would flicker.
        WM_ERASEBKGND => return 1,
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        WM_APP_QUIT => {
            DestroyWindow(hwnd);
            return 0;
        }
        _ => {}
    }
    // Handlers can cause nested messages (e.g. SetCapture); those get the default.
    let handled = STATE.with(|st| match st.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(s) => s.handle(hwnd, msg, wp, lp),
            None => None,
        },
        Err(_) => None,
    });
    match handled {
        Some(r) => r,
        None => DefWindowProcA(hwnd, msg, wp, lp),
    }
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

fn modifiers() -> KeyModifiers {
    let mut m = KeyModifiers::NONE;
    if key_down(0x10) {
        m |= KeyModifiers::SHIFT;
    }
    if key_down(0x11) {
        m |= KeyModifiers::CONTROL;
    }
    if key_down(0x12) {
        m |= KeyModifiers::ALT;
    }
    m
}

fn special_key(vk: usize, shift: bool) -> Option<KeyCode> {
    Some(match vk {
        0x08 => KeyCode::Backspace,
        0x09 if shift => KeyCode::BackTab,
        0x09 => KeyCode::Tab,
        0x0D => KeyCode::Enter,
        0x1B => KeyCode::Esc,
        0x21 => KeyCode::PageUp,
        0x22 => KeyCode::PageDown,
        0x23 => KeyCode::End,
        0x24 => KeyCode::Home,
        0x25 => KeyCode::Left,
        0x26 => KeyCode::Up,
        0x27 => KeyCode::Right,
        0x28 => KeyCode::Down,
        0x2D => KeyCode::Insert,
        0x2E => KeyCode::Delete,
        0x70..=0x7B => KeyCode::F((vk - 0x6F) as u8),
        _ => return None,
    })
}

fn is_nt() -> bool {
    unsafe { GetVersion() & 0x8000_0000 == 0 }
}

/// Text from the clipboard, if there is any.
fn clipboard_text(hwnd: Handle) -> Option<String> {
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return None;
        }
        let mut out = None;
        // Windows 9x has no Unicode clipboard; use the ANSI text there.
        if is_nt() && IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
            let h = GetClipboardData(CF_UNICODETEXT);
            let p = if h.is_null() { null_mut() } else { GlobalLock(h) as *const u16 };
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(h);
            }
        } else if IsClipboardFormatAvailable(CF_TEXT) != 0 {
            let h = GetClipboardData(CF_TEXT);
            let p = if h.is_null() { null_mut() } else { GlobalLock(h) };
            if !p.is_null() {
                out = Some(ansi_to_string(std::slice::from_raw_parts(p, lstrlenA(p) as usize)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out.map(|s| s.replace("\r\n", "\n"))
    }
}

/// Put text on the clipboard: as ANSI text, and on NT also as Unicode.
fn set_clipboard(hwnd: Handle, text: &str) {
    let text = text.replace('\n', "\r\n");
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        // GMEM_MOVEABLE | GMEM_DDESHARE
        let put = |format: u32, bytes: &[u8]| {
            let h = GlobalAlloc(0x2002, bytes.len());
            if h.is_null() {
                return;
            }
            let p = GlobalLock(h);
            if p.is_null() {
                GlobalFree(h);
                return;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            GlobalUnlock(h);
            if SetClipboardData(format, h).is_null() {
                GlobalFree(h);
            }
        };
        let n = WideCharToMultiByte(0, 0, wide.as_ptr(), wide.len() as i32, null_mut(), 0, c"?".as_ptr().cast(), null_mut());
        if n > 0 {
            let mut ansi = vec![0u8; n as usize];
            WideCharToMultiByte(0, 0, wide.as_ptr(), wide.len() as i32, ansi.as_mut_ptr(), n, c"?".as_ptr().cast(), null_mut());
            put(CF_TEXT, &ansi);
        }
        if is_nt() {
            let bytes: Vec<u8> = wide.iter().flat_map(|w| w.to_le_bytes()).collect();
            put(CF_UNICODETEXT, &bytes);
        }
        CloseClipboard();
    }
}

/// Text in the ANSI code page to a String.
fn ansi_to_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut w = vec![0u16; bytes.len()];
    let n = unsafe { MultiByteToWideChar(0, 0, bytes.as_ptr(), bytes.len() as i32, w.as_mut_ptr(), w.len() as i32) };
    String::from_utf16_lossy(&w[..n.max(0) as usize])
}

impl WinState {
    fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    fn key(&self, code: KeyCode, mods: KeyModifiers) {
        self.send(Event::Key(KeyEvent::new(code, mods)));
    }

    /// Copy the selected text; false if nothing is selected.
    fn copy_selection(&self, hwnd: Handle) -> bool {
        let text = lock(&self.screen).selected_text();
        if text.is_empty() {
            return false;
        }
        set_clipboard(hwnd, &text);
        true
    }

    fn clear_selection(&self, hwnd: Handle) {
        let had = {
            let mut s = lock(&self.screen);
            let had = s.selection.is_some();
            s.set_selection(None);
            had
        };
        if had {
            self.invalidate_dirty(hwnd);
        }
    }

    fn paste(&self, hwnd: Handle) {
        if let Some(t) = clipboard_text(hwnd) {
            self.send(Event::Paste(t));
        }
    }

    /// A typed character (byte in the ANSI code page) as a key event.
    fn typed(&mut self, byte: u8, mods: KeyModifiers) {
        let s = if let Some(lead) = self.dbcs_lead.take() {
            ansi_to_string(&[lead, byte])
        } else if byte >= 0x80 && unsafe { IsDBCSLeadByte(byte) } != 0 {
            self.dbcs_lead = Some(byte);
            return;
        } else {
            ansi_to_string(&[byte])
        };
        for c in s.chars() {
            self.key(KeyCode::Char(c), mods);
        }
    }

    fn mouse_cell(&self, lp: isize) -> (u16, u16) {
        let x = (lp & 0xffff) as i16 as i32;
        let y = ((lp >> 16) & 0xffff) as i16 as i32;
        let s = lock(&self.screen);
        let col = (x.max(0) / self.cell_w).min(s.cols.saturating_sub(1) as i32);
        let row = (y.max(0) / self.cell_h).min(s.rows.saturating_sub(1) as i32);
        (col as u16, row as u16)
    }

    fn mouse_event(&mut self, kind: MouseEventKind, (column, row): (u16, u16), wp: usize) {
        let mut modifiers = KeyModifiers::NONE;
        if wp & 4 != 0 {
            modifiers |= KeyModifiers::SHIFT;
        }
        if wp & 8 != 0 {
            modifiers |= KeyModifiers::CONTROL;
        }
        self.last_mouse = (column, row);
        self.send(Event::Mouse(MouseEvent { kind, column, row, modifiers }));
    }

    fn handle(&mut self, hwnd: Handle, msg: u32, wp: usize, lp: isize) -> Option<isize> {
        match msg {
            WM_PAINT => unsafe { self.paint(hwnd) },
            WM_APP_REDRAW => self.invalidate_dirty(hwnd),
            WM_SIZE => {
                if wp == 1 {
                    return Some(0); // minimized
                }
                let (w, h) = ((lp & 0xffff) as i32, ((lp >> 16) & 0xffff) as i32);
                let (cols, rows) = (((w / self.cell_w).max(1)) as u16, ((h / self.cell_h).max(1)) as u16);
                let changed = {
                    let mut s = lock(&self.screen);
                    let changed = (s.cols, s.rows) != (cols, rows);
                    if changed {
                        s.resize(cols, rows);
                    }
                    changed
                };
                if changed {
                    self.send(Event::Resize(cols, rows));
                }
                unsafe {
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            WM_SETFOCUS => self.send(Event::FocusGained),
            WM_KILLFOCUS => self.send(Event::FocusLost),
            // Closing the window works like Ctrl+Q (which asks first if set to).
            WM_CLOSE => self.key(KeyCode::Char('q'), KeyModifiers::CONTROL),
            WM_KEYDOWN => {
                let m = modifiers();
                let (shift, ctrl, alt) =
                    (m.contains(KeyModifiers::SHIFT), m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT));
                // Ctrl+C or Ctrl+Insert copies while text is selected (else Ctrl+C goes to the app).
                if ctrl && !alt && (wp == b'C' as usize || wp == 0x2D) && self.copy_selection(hwnd) {
                    self.clear_selection(hwnd);
                    return Some(0);
                }
                if !matches!(wp, 0x10..=0x12) {
                    self.clear_selection(hwnd);
                }
                if (shift && wp == 0x2D) || (ctrl && !alt && wp == b'V' as usize) {
                    self.paste(hwnd);
                } else if let Some(code) = special_key(wp, shift) {
                    self.key(code, m);
                } else if ctrl && !alt && (wp as u8).is_ascii_alphanumeric() && wp < 0x80 {
                    // Ctrl+letter: WM_CHAR would only give a control code.
                    self.key(KeyCode::Char((wp as u8 as char).to_ascii_lowercase()), m);
                } else if ctrl && !alt && wp == 0x20 {
                    self.key(KeyCode::Char(' '), m);
                } else {
                    return None;
                }
            }
            WM_CHAR => {
                let b = wp as u8;
                // Control codes come as WM_KEYDOWN above.
                if b < 0x20 || b == 0x7f {
                    return Some(0);
                }
                let m = modifiers();
                // Ctrl+Alt is AltGr: the character itself is what counts.
                let mods = if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::ALT) {
                    KeyModifiers::NONE
                } else {
                    m & KeyModifiers::SHIFT
                };
                self.typed(b, mods);
            }
            WM_SYSKEYDOWN => {
                // Alt+F4 closes the window as usual.
                if wp == 0x73 {
                    return None;
                }
                let m = modifiers();
                if let Some(code) = special_key(wp, m.contains(KeyModifiers::SHIFT)) {
                    self.key(code, m);
                }
                // Letters arrive as WM_SYSCHAR.
            }
            // Don't let a lone Alt open the system menu.
            WM_SYSKEYUP => {}
            WM_SYSCHAR => {
                let b = wp as u8;
                if b == b' ' {
                    return None; // Alt+Space: system menu
                }
                if b >= 0x20 && b != 0x7f {
                    let m = modifiers();
                    self.typed(b, KeyModifiers::ALT | (m & KeyModifiers::CONTROL));
                }
            }
            WM_MOUSEMOVE => {
                let cell = self.mouse_cell(lp);
                if wp & 1 != 0 && self.selector.drag(&self.screen, cell) {
                    self.invalidate_dirty(hwnd);
                }
                if cell != self.last_mouse {
                    let kind = if wp & 1 != 0 {
                        MouseEventKind::Drag(MouseButton::Left)
                    } else if wp & 2 != 0 {
                        MouseEventKind::Drag(MouseButton::Right)
                    } else if wp & 0x10 != 0 {
                        MouseEventKind::Drag(MouseButton::Middle)
                    } else {
                        MouseEventKind::Moved
                    };
                    self.mouse_event(kind, cell, wp);
                }
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                let button = match msg {
                    WM_LBUTTONDOWN | WM_LBUTTONUP => MouseButton::Left,
                    WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
                    _ => MouseButton::Middle,
                };
                let down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN);
                let cell = self.mouse_cell(lp);
                // Dragging with the left button selects text; letting go copies it.
                if msg == WM_LBUTTONDOWN {
                    self.selector.press(&self.screen, cell);
                    self.invalidate_dirty(hwnd);
                } else if msg == WM_LBUTTONUP {
                    if let Some(text) = self.selector.release(&self.screen) {
                        set_clipboard(hwnd, &text);
                    }
                }
                self.mouse_event(if down { MouseEventKind::Down(button) } else { MouseEventKind::Up(button) }, cell, wp);
                // Capturing sends WM_CAPTURECHANGED to us; that one gets the default handling.
                unsafe {
                    if down {
                        SetCapture(hwnd);
                    } else {
                        ReleaseCapture();
                    }
                }
            }
            WM_MOUSEWHEEL => {
                // The position is in screen coordinates here.
                let mut p = Point { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
                unsafe {
                    ScreenToClient(hwnd, &mut p);
                }
                let cell = self.mouse_cell(((p.y & 0xffff) << 16 | (p.x & 0xffff)) as isize);
                let delta = ((wp >> 16) & 0xffff) as i16 as i32;
                let kind = if delta > 0 { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown };
                for _ in 0..(delta.abs() / 120).max(1) {
                    self.mouse_event(kind, cell, wp & 0xffff);
                }
            }
            _ => return None,
        }
        Some(0)
    }

    fn invalidate_dirty(&self, hwnd: Handle) {
        let mut s = lock(&self.screen);
        let width = s.cols as i32 * self.cell_w;
        let mut row = 0;
        while row < s.dirty.len() {
            if !s.dirty[row] {
                row += 1;
                continue;
            }
            let start = row;
            while row < s.dirty.len() && s.dirty[row] {
                s.dirty[row] = false;
                row += 1;
            }
            let r = Rect { left: 0, top: start as i32 * self.cell_h, right: width, bottom: row as i32 * self.cell_h };
            unsafe {
                InvalidateRect(hwnd, &r, 0);
            }
        }
    }

    unsafe fn paint(&mut self, hwnd: Handle) {
        let mut ps: PaintStruct = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut client = Rect::default();
        GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right.max(1), client.bottom.max(1));
        if self.mem_dc.is_null() || self.mem_size.0 < w || self.mem_size.1 < h {
            if !self.mem_dc.is_null() {
                DeleteDC(self.mem_dc);
                DeleteObject(self.mem_bmp);
            }
            self.mem_dc = CreateCompatibleDC(dc);
            self.mem_bmp = CreateCompatibleBitmap(dc, w, h);
            SelectObject(self.mem_dc, self.mem_bmp);
            SetBkMode(self.mem_dc, 2 /* OPAQUE */);
            self.mem_size = (w, h);
        }
        let mdc = self.mem_dc;
        let p = ps.paint;
        let (cw, ch) = (self.cell_w, self.cell_h);
        {
            let s = lock(&self.screen);
            let first = (p.top / ch).max(0) as u16;
            let last = (((p.bottom + ch - 1) / ch) as u16).min(s.rows);
            for row in first..last {
                self.paint_row(mdc, &s, row);
            }
            // Space right of and below the grid.
            let gw = s.cols as i32 * cw;
            let gh = s.rows as i32 * ch;
            fill(mdc, DEFAULT_BG, Rect { left: gw, top: 0, right: w, bottom: h });
            fill(mdc, DEFAULT_BG, Rect { left: 0, top: gh, right: w, bottom: h });
            // Cursor: a bar at the left of its cell.
            let (cx, cy) = s.cursor;
            if s.cursor_visible && cx < s.cols && cy < s.rows {
                let cell = &s.cells[cy as usize * s.cols as usize + cx as usize];
                let x = cx as i32 * cw;
                let y = cy as i32 * ch;
                fill(mdc, cell.fg, Rect { left: x, top: y, right: x + (cw / 4).max(2), bottom: y + ch });
            }
        }
        BitBlt(dc, p.left, p.top, p.right - p.left, p.bottom - p.top, mdc, p.left, p.top, SRCCOPY);
        EndPaint(hwnd, &ps);
        if let Some(path) = std::env::var_os("F_TERM_GUI_DUMP") {
            self.dump(std::path::Path::new(&path), w, h);
        }
    }

    /// Save the window contents as a BMP file (F_TERM_GUI_DUMP, for testing).
    unsafe fn dump(&self, path: &std::path::Path, w: i32, h: i32) {
        // BITMAPINFOHEADER for 32-bit top-down rows.
        let mut info = [0u8; 40];
        info[0..4].copy_from_slice(&40u32.to_le_bytes());
        info[4..8].copy_from_slice(&w.to_le_bytes());
        info[8..12].copy_from_slice(&(-h).to_le_bytes());
        info[12..14].copy_from_slice(&1u16.to_le_bytes());
        info[14..16].copy_from_slice(&32u16.to_le_bytes());
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        // The bitmap must not be selected into a DC for GetDIBits.
        let tmp = CreateCompatibleBitmap(self.mem_dc, 1, 1);
        let old = SelectObject(self.mem_dc, tmp);
        GetDIBits(self.mem_dc, old, 0, h as u32, pixels.as_mut_ptr(), info.as_mut_ptr(), 0);
        SelectObject(self.mem_dc, old);
        DeleteObject(tmp);
        let mut file = Vec::with_capacity(54 + pixels.len());
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&info);
        file.extend_from_slice(&pixels);
        let _ = std::fs::write(path, file);
    }

    unsafe fn paint_row(&self, dc: Handle, s: &Screen, row: u16) {
        let (cw, ch) = (self.cell_w, self.cell_h);
        let cols = s.cols as usize;
        let cells = s.row(row);
        let y = row as i32 * ch;
        let mut x = 0;
        let mut text: Vec<u16> = Vec::new();
        let mut dx: Vec<i32> = Vec::new();
        while x < cols {
            let c = &cells[x];
            let (fg, bg) = s.colors(x as u16, row, c);
            let w = if x + c.width() > cols { 1 } else { c.width() };
            if let Some(shape) = c.shape() {
                let r = PxRect { left: x as i32 * cw, top: y, right: (x + w) as i32 * cw, bottom: y + ch };
                let mut f = |r: PxRect, color: u32| fill(dc, color, Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom });
                f(r, bg);
                draw_shape(shape, fg, bg, r, &mut f);
                x += w;
                continue;
            }
            // A run of cells with the same style, drawn in one call.
            let start = x;
            text.clear();
            dx.clear();
            while x < cols {
                let d = &cells[x];
                if s.colors(x as u16, row, d) != (fg, bg) || d.flags != c.flags {
                    break;
                }
                if x > start && d.shape().is_some() {
                    break;
                }
                let w = if x + d.width() > cols { 1 } else { d.width() };
                // Image cells are painted over below.
                let sym = if d.sym.is_empty() || d.sym == crate::images::IMAGE_CELL { " " } else { d.sym.as_str() };
                let before = text.len();
                text.extend(sym.encode_utf16());
                dx.push(w as i32 * cw);
                dx.extend(std::iter::repeat(0).take(text.len() - before - 1));
                x += w;
            }
            let r = Rect { left: start as i32 * cw, top: y, right: x as i32 * cw, bottom: y + ch };
            let font = self.fonts[(c.flags & BOLD != 0) as usize | ((c.flags & ITALIC != 0) as usize) << 1];
            let old = SelectObject(dc, font);
            SetTextColor(dc, colorref(fg));
            SetBkColor(dc, colorref(bg));
            ExtTextOutW(dc, r.left, y, ETO_OPAQUE | ETO_CLIPPED, &r, text.as_ptr(), text.len() as u32, dx.as_ptr());
            SelectObject(dc, old);
            let line = (ch / 16).max(1);
            if c.flags & UNDERLINE != 0 {
                fill(dc, fg, Rect { top: y + ch - line - 1, bottom: y + ch - 1, ..r });
            }
            if c.flags & CROSSED != 0 {
                fill(dc, fg, Rect { top: y + ch / 2, bottom: y + ch / 2 + line, ..r });
            }
        }
        // Images in the cells marked for them.
        for (i, x0, x1) in image_runs(s, row) {
            let bg = cells[x0 as usize].bg;
            let block = self.images.borrow_mut().block(&s.images[i], row, x0..x1, (cw, ch), bg);
            let w = (x1 - x0) as i32 * cw;
            // A bottom-up 32-bit DIB; 0x00RRGGBB in memory is the B, G, R, 0 it expects.
            let mut bits: Vec<u32> = Vec::with_capacity(block.len());
            for yy in (0..ch).rev() {
                bits.extend_from_slice(&block[(yy * w) as usize..((yy + 1) * w) as usize]);
            }
            let mut info = [0u8; 40];
            info[0..4].copy_from_slice(&40u32.to_le_bytes());
            info[4..8].copy_from_slice(&w.to_le_bytes());
            info[8..12].copy_from_slice(&ch.to_le_bytes());
            info[12..14].copy_from_slice(&1u16.to_le_bytes());
            info[14..16].copy_from_slice(&32u16.to_le_bytes());
            SetDIBitsToDevice(dc, x0 as i32 * cw, y, w as u32, ch as u32, 0, 0, 0, ch as u32, bits.as_ptr().cast(), info.as_ptr(), 0);
        }
    }
}

/// Fill a rectangle with a colour (ExtTextOut with no text needs no brush).
unsafe fn fill(dc: Handle, color: u32, r: Rect) {
    if r.right > r.left && r.bottom > r.top {
        SetBkColor(dc, colorref(color));
        ExtTextOutA(dc, 0, 0, ETO_OPAQUE, &r, null(), 0, null());
    }
}

/// GDI colours are 0x00BBGGRR; ours are 0x00RRGGBB.
fn colorref(c: u32) -> u32 {
    (c & 0xff) << 16 | (c & 0xff00) | (c >> 16) & 0xff
}
