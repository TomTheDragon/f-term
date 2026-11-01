//! F-Term: a terminal client for F-Chat (F-List).

// `Typing::Typing` / `Action::FriendRequestAction` read naturally; small tuple types are fine here.
#![allow(clippy::enum_variant_names, clippy::type_complexity)]

mod api;
mod bbcode;
mod commands;
mod config;
mod diagnose;
mod eicons;
mod images;
mod logs;
mod net;
mod notify;
mod paths;
mod protocol;
mod site;
mod state;
mod timeutil;
mod tls;
mod ui;

use std::path::PathBuf;

fn usage() -> String {
    format!(
        "F-Term {} — F-Chat client for the terminal

Usage: f-term [options]

Options:
  -a, --account <name>     Account to log in with (uses saved login if present)
  -c, --character <name>   Character to connect as
      --no-autologin       Don't log in automatically
      --config-dir <dir>   Store settings, logs and cache in <dir>
      --debug              Write a protocol debug log (debug.log in the config dir)
      --diagnose           Check the connection to F-List step by step and exit
      --forget <account>   Remove a saved login and exit
      --paths              Print where settings and logs are stored and exit
  -h, --help               Show this help
  -V, --version            Show the version

Settings are stored in settings.toml; edit them in the app with F6 or /set.
Portable mode: put a file named \"portable\" next to the executable.
TLS backend: {}",
        env!("CARGO_PKG_VERSION"),
        tls::backend_name()
    )
}

/// Print an error about the command line and exit.
fn exit_with(msg: &str) -> ! {
    #[cfg(all(windows, feature = "gui"))]
    let _console = ui::gui::CliConsole::open();
    eprintln!("{msg}");
    #[cfg(all(windows, feature = "gui"))]
    drop(_console);
    std::process::exit(2);
}

fn main() {
    // The GUI build has no console: open one for options that print something.
    #[cfg(all(windows, feature = "gui"))]
    let _console = std::env::args()
        .skip(1)
        .any(|a| matches!(a.as_str(), "-h" | "--help" | "-V" | "--version" | "--diagnose" | "--forget" | "--paths"))
        .then(ui::gui::CliConsole::open);
    let mut opts = ui::Options { account: None, character: None, no_autologin: false };
    let mut config_dir: Option<PathBuf> = None;
    let mut debug = false;
    let mut forget: Option<String> = None;
    let mut show_paths = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| -> String { args.next().unwrap_or_else(|| exit_with(&format!("{name} needs a value"))) };
        match a.as_str() {
            "-a" | "--account" => opts.account = Some(value("--account")),
            "-c" | "--character" => opts.character = Some(value("--character")),
            "--no-autologin" => opts.no_autologin = true,
            "--config-dir" => config_dir = Some(PathBuf::from(value("--config-dir"))),
            "--debug" => debug = true,
            "--diagnose" => {
                diagnose::run();
                return;
            }
            "--forget" => forget = Some(value("--forget")),
            "--paths" => show_paths = true,
            "-h" | "--help" => {
                println!("{}", usage());
                return;
            }
            "-V" | "--version" => {
                println!("F-Term {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => exit_with(&format!("Unknown option {other}\n\n{}", usage())),
        }
    }

    paths::init(config_dir);
    if show_paths {
        let d = paths::dirs();
        println!("config: {}\nlogs:   {}\ncache:  {}", d.config.display(), d.data.join("logs").display(), d.cache.display());
        return;
    }
    if let Some(acc) = forget {
        let mut accounts = config::Accounts::load();
        accounts.remove(&acc);
        match accounts.save() {
            Ok(()) => println!("Removed saved login for {acc}."),
            Err(e) => eprintln!("Could not save: {e}"),
        }
        return;
    }
    if debug {
        net::log::enable(&paths::config_dir().join("debug.log"));
    }

    // Restore the terminal if we panic.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ui::restore_terminal();
        // Also keep the message in a file: the console may close before it can be read.
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(paths::config_dir().join("crash.log")).and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "F-Term {}: {info}", env!("CARGO_PKG_VERSION"))
        });
        #[cfg(all(windows, feature = "gui"))]
        ui::gui::show_error(&format!("F-Term crashed:\n\n{info}"));
        default_hook(info);
    }));

    let settings = config::Settings::load();
    let accounts = config::Accounts::load();
    let mut app = ui::App::new(settings, accounts, opts);
    if let Err(e) = app.run() {
        ui::restore_terminal();
        #[cfg(all(windows, feature = "gui"))]
        ui::gui::show_error(&format!("Error: {e}"));
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    /// Panics must unwind (not abort) so the terminal can be restored;
    /// this matters for the llvm-mingw Windows builds.
    #[test]
    fn panics_unwind() {
        let r = std::panic::catch_unwind(|| panic!("expected test panic"));
        assert!(r.is_err());
    }
}
