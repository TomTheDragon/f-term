//! Notifications: terminal bell, desktop notifications via escape codes,
//! or a user-configured command.

use std::io::Write;

use crate::config::Settings;

pub fn notify(settings: &Settings, title: &str, body: &str) {
    let title = clean(title);
    let body = clean(body);
    let mut out = std::io::stdout();
    if settings.notify_bell {
        #[cfg(feature = "gui")]
        if crate::ui::gui::active() {
            crate::ui::gui::bell();
            // The Linux window has no bell sound of its own: use the desktop's.
            if cfg!(all(unix, not(target_os = "macos"))) && settings.notify_sound.trim().is_empty() {
                spawn_quiet("canberra-gtk-play", &["--id=message-new-instant"]);
            }
        }
        let _ = out.write_all(b"\x07");
    }
    let sound = settings.notify_sound.trim();
    if !sound.is_empty() {
        play_sound(sound);
    }
    if settings.notify_osc {
        // OSC 777 (rxvt/foot/wezterm/ghostty), OSC 9 (iTerm2/kitty/Windows Terminal).
        let _ = write!(out, "\x1b]777;notify;{title};{body}\x1b\\");
        let _ = write!(out, "\x1b]9;{title}: {body}\x1b\\");
    }
    let _ = out.flush();
    let cmd = settings.notify_command.trim();
    if !cmd.is_empty() {
        let parts = split_command(cmd);
        if let Some((prog, args)) = parts.split_first() {
            let args: Vec<String> = args.iter().map(|a| a.replace("%t", &title).replace("%m", &body)).collect();
            let _ = std::process::Command::new(prog)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn spawn_quiet(prog: &str, args: &[&str]) -> bool {
    std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Play a sound file (WAV) without waiting for it.
#[cfg(windows)]
pub fn play_sound(path: &str) {
    // winmm.dll is loaded when needed, so the builds don't have to link it.
    type PlaySoundA = unsafe extern "system" fn(*const u8, isize, u32) -> i32;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(name: *const u8) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *const std::ffi::c_void;
    }
    const SND_ASYNC: u32 = 0x1;
    const SND_NODEFAULT: u32 = 0x2;
    const SND_FILENAME: u32 = 0x2_0000;
    let Ok(file) = std::ffi::CString::new(path) else { return };
    unsafe {
        let lib = LoadLibraryA(b"winmm.dll\0".as_ptr());
        if lib == 0 {
            return;
        }
        let f = GetProcAddress(lib, b"PlaySoundA\0".as_ptr());
        if f.is_null() {
            return;
        }
        let play: PlaySoundA = std::mem::transmute(f);
        play(file.as_ptr() as *const u8, 0, SND_FILENAME | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(not(windows))]
pub fn play_sound(path: &str) {
    let players: &[(&str, &[&str])] =
        if cfg!(target_os = "macos") { &[("afplay", &[])] } else { &[("pw-play", &[]), ("paplay", &[]), ("aplay", &["-q"])] };
    for (prog, args) in players {
        let mut a: Vec<&str> = args.to_vec();
        a.push(path);
        if spawn_quiet(prog, &a) {
            return;
        }
    }
}

fn clean(s: &str) -> String {
    let s = crate::bbcode::strip(s);
    let s: String = s.chars().filter(|c| !c.is_control() && *c != ';').take(200).collect();
    s
}

/// Split a command line on whitespace, honouring double quotes.
pub fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Open a URL in the system browser.
pub fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Only http(s) links can be opened".into());
    }
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open browser: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn split() {
        assert_eq!(super::split_command(r#"notify-send "%t" "%m""#), vec!["notify-send", "%t", "%m"]);
    }
}
