//! Locations of the config, data (logs) and cache directories.
//!
//! No external crate is used here on purpose: the usual helpers call
//! Windows APIs that do not exist on Windows 9x.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static BASE: OnceLock<Dirs> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

/// Default directories for an app folder name (`app`) and portable folder name.
fn detect_named(app: &str, portable_name: &str) -> Dirs {
    // Explicit override, or portable mode (a file named "portable" next to the exe).
    let portable_root = env_path("F_TERM_HOME").or_else(|| env_path("F_TERMINAL_HOME")).or_else(|| {
        let dir = exe_dir();
        if dir.join("portable").exists() || dir.join("portable.txt").exists() {
            Some(dir.join(portable_name))
        } else {
            None
        }
    });
    if let Some(root) = portable_root {
        return Dirs { config: root.clone(), data: root.join("data"), cache: root.join("cache") };
    }

    if cfg!(windows) {
        let root = env_path("APPDATA").map(|p| p.join(app)).unwrap_or_else(|| exe_dir().join(portable_name));
        let cache = env_path("LOCALAPPDATA").map(|p| p.join(app).join("cache")).unwrap_or_else(|| root.join("cache"));
        return Dirs { config: root.clone(), data: root.join("data"), cache };
    }

    let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        let root = home.join("Library/Application Support").join(app);
        return Dirs { config: root.clone(), data: root.join("data"), cache: home.join("Library/Caches").join(app) };
    }

    let config = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join(app);
    let data = env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")).join(app);
    let cache = env_path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")).join(app);
    Dirs { config, data, cache }
}

fn detect() -> Dirs {
    let dirs = detect_named("f-term", "f-term-data");
    // The project used to be called "f-terminal": move old folders over once.
    let old = detect_named("f-terminal", "f-terminal-data");
    for (from, to) in [(&old.config, &dirs.config), (&old.data, &dirs.data), (&old.cache, &dirs.cache)] {
        if from != to && from.is_dir() && !to.exists() {
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(from, to);
        }
    }
    dirs
}

pub fn init(override_root: Option<PathBuf>) {
    let dirs = match override_root {
        Some(root) => Dirs { config: root.clone(), data: root.join("data"), cache: root.join("cache") },
        None => detect(),
    };
    let _ = BASE.set(dirs);
}

pub fn dirs() -> &'static Dirs {
    BASE.get_or_init(detect)
}

pub fn config_dir() -> PathBuf {
    let d = dirs().config.clone();
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn data_dir() -> PathBuf {
    let d = dirs().data.clone();
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cache_dir() -> PathBuf {
    let d = dirs().cache.clone();
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Make a string safe to use as a file name on every platform.
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() {
        "_".into()
    } else {
        s
    }
}

/// Write a file atomically-ish (write temp, then rename) and restrict permissions on unix.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    // Windows 9x can't rename over an existing file.
    if cfg!(windows) && path.exists() {
        let _ = std::fs::remove_file(path);
    }
    if std::fs::rename(&tmp, path).is_ok() {
        return Ok(());
    }
    // Renaming can fail on old systems; write the file in place instead.
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(path, contents)
}
