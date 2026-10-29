//! End-to-end test against an in-process mock F-Chat server (HTTP API + websocket).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use super::{App, ConnState, Options};
use crate::config::{Accounts, Settings};

type Log = Arc<Mutex<Vec<String>>>;

fn http_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || handle_http(s));
        }
    });
    port
}

fn handle_http(s: TcpStream) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut first = String::new();
    r.read_line(&mut first).unwrap();
    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut len = 0;
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        if h.trim().is_empty() {
            break;
        }
        if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    let body = String::from_utf8_lossy(&body).to_string();
    let reply = match path.as_str() {
        "/json/getApiTicket.php" => {
            if body.contains("password=wrong") {
                json!({"error": "Login failed."})
            } else {
                json!({"ticket": "tkt", "characters": {"Tester": 1, "Alt Char": 2}, "default_character": 1,
                       "friends": [{"dest_name": "Tester", "source_name": "Friendo"}], "bookmarks": [{"name": "Booky"}], "error": ""})
            }
        }
        "/json/api/mapping-list.php" => json!({
            "kinks": [{"id": "1", "name": "Hugging", "description": "", "group_id": "1"}],
            "kink_groups": [{"id": "1", "name": "General"}],
            "infotags": [{"id": "3", "name": "Gender", "type": "list", "list": "gender", "group_id": "1"}],
            "infotag_groups": [{"id": "1", "name": "General details"}],
            "listitems": [{"id": "9", "name": "gender", "value": "Female"}],
            "error": ""}),
        "/json/api/character-data.php" => json!({
            "id": 5, "name": "Friendo", "description": "[b]Hello[/b] I am a test.", "views": 42, "custom_title": null,
            "created_at": 1600000000, "updated_at": 1700000000, "badges": [],
            "kinks": {"1": "fave"}, "custom_kinks": {}, "infotags": {"3": "9"}, "images": [], "error": ""}),
        "/json/api/friend-bookmark-lists.php" => {
            json!({"bookmarklist": ["Booky"], "friendlist": [{"source": "Friendo", "dest": "Tester", "last_online": 0}],
            "requestlist": [{"id": "77", "source": "Stranger", "dest": "Tester"}], "requestpending": [], "error": ""})
        }
        _ => json!({"error": ""}),
    };
    let body = reply.to_string();
    let mut s = s;
    let _ = write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
}

fn ws_server(log: Log) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                let mut ws = tungstenite::accept(s).unwrap();
                let send = |ws: &mut tungstenite::WebSocket<TcpStream>, line: String| {
                    ws.send(tungstenite::Message::Text(line)).unwrap();
                };
                loop {
                    let msg = match ws.read() {
                        Ok(tungstenite::Message::Text(t)) => t,
                        Ok(_) => continue,
                        Err(_) => return,
                    };
                    log.lock().unwrap().push(msg.clone());
                    let (code, body) = msg.split_at(3);
                    let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
                    match code {
                        "IDN" => {
                            assert_eq!(v["ticket"], "tkt");
                            send(&mut ws, r#"HLO {"message":"Welcome to the mock server."}"#.into());
                            send(&mut ws, r#"IDN {"character":"Tester"}"#.into());
                            send(&mut ws, r#"VAR {"variable":"msg_flood","value":0.1}"#.into());
                            send(&mut ws, r#"ADL {"ops":["Admin"]}"#.into());
                            send(&mut ws, r#"CON {"count":3}"#.into());
                            send(&mut ws, r#"LIS {"characters":[["Friendo","Female","looking","Up for RP"],["Booky","Male","online",""]]}"#.into());
                            send(&mut ws, r#"IGN {"action":"init","characters":["Pest"]}"#.into());
                            send(&mut ws, r#"NLN {"identity":"Tester","gender":"Herm","status":"online"}"#.into());
                        }
                        "JCH" => {
                            let ch = v["channel"].as_str().unwrap().to_string();
                            send(&mut ws, format!(r#"JCH {{"channel":"{ch}","character":{{"identity":"Tester"}},"title":"{ch}"}}"#));
                            send(&mut ws, format!(r#"ICH {{"channel":"{ch}","mode":"both","users":[{{"identity":"Tester"}},{{"identity":"Friendo"}}]}}"#));
                            send(&mut ws, format!(r#"CDS {{"channel":"{ch}","description":"[b]Welcome[/b] to {ch}"}}"#));
                            send(&mut ws, format!(r#"COL {{"channel":"{ch}","oplist":["Friendo"]}}"#));
                        }
                        "MSG" => {
                            let ch = v["channel"].as_str().unwrap();
                            let m = v["message"].as_str().unwrap();
                            send(&mut ws, format!("MSG {}", json!({"channel": ch, "character": "Friendo", "message": format!("echo {m} Tester")})));
                            send(&mut ws, format!("LRP {}", json!({"channel": ch, "character": "Booky", "message": "[color=green]Looking for RP![/color]"})));
                        }
                        "PRI" => {
                            send(&mut ws, r#"TPN {"character":"Friendo","status":"typing"}"#.into());
                            send(&mut ws, format!("PRI {}", json!({"character": "Friendo", "message": "/me waves back [eicon]wave[/eicon]"})));
                        }
                        "CHA" => send(&mut ws, r#"CHA {"channels":[{"name":"Frontpage","mode":"both","characters":100},{"name":"Writing","mode":"chat","characters":5}]}"#.into()),
                        "ORS" => send(&mut ws, r#"ORS {"channels":[{"name":"ADH-123","title":"Cozy Room","characters":2}]}"#.into()),
                        "STA" => send(&mut ws, format!("STA {}", json!({"character": "Tester", "status": v["status"], "statusmsg": v["statusmsg"]}))),
                        "RLL" => send(&mut ws, format!("RLL {}", json!({"channel": v["channel"], "character": "Tester", "type": "dice", "message": "[b]Tester[/b] rolls 1d20: [b]17[/b]", "results": [17], "rolls": ["1d20"], "endresult": 17}))),
                        _ => {}
                    }
                }
            });
        }
    });
    port
}

fn pump(app: &mut App, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while let Ok(ev) = app.rx.try_recv() {
            app.handle_app_event(ev);
        }
        app.flush_outbox();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn pump_until(app: &mut App, what: &str, f: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f(app) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        pump(app, 20);
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_terminal_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn type_text(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn show(s: &str) {
    if std::env::var("SHOW_SCREENS").is_ok() {
        eprintln!("{s}");
    }
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    // Every screen must also draw without panicking on small consoles
    // (80x25 is the Windows 9x default) and tiny windows.
    for (sw, sh) in [(80u16, 25u16), (40, 16), (39, 30), (12, 5), (1, 1)] {
        let mut t = Terminal::new(TestBackend::new(sw, sh)).unwrap();
        app.images.begin_frame();
        t.draw(|f| app.draw(f)).unwrap();
    }
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    app.images.begin_frame();
    t.draw(|f| app.draw(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn end_to_end() {
    let dir = std::env::temp_dir().join(format!("f-term-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crate::paths::init(Some(dir.clone()));
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let http = http_server();
    let ws = ws_server(log.clone());
    let settings = Settings {
        api_base: format!("http://127.0.0.1:{http}"),
        server: format!("ws://127.0.0.1:{ws}"),
        idle_minutes: 0,
        ..Default::default()
    };

    let mut app = App::new(settings, Accounts::default(), Options { account: None, character: None, no_autologin: true });
    let s = screen(&mut app, 100, 30);
    show(&s);
    assert!(s.contains("F-Chat login"), "{s}");

    // Wrong password shows an error.
    type_text(&mut app, "acct");
    key(&mut app, KeyCode::Enter);
    type_text(&mut app, "wrong");
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "login error", |a| a.login.as_ref().map(|l| !l.busy && !l.error.is_empty()).unwrap_or(false));
    assert!(screen(&mut app, 100, 30).contains("Login failed."));

    // Correct password, save it.
    app.login.as_mut().unwrap().password.set("secret");
    app.login.as_mut().unwrap().save_password = true;
    app.login.as_mut().unwrap().focus = 4;
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "character list", |a| a.login.as_ref().map(|l| l.stage == super::login::Stage::Character).unwrap_or(false));
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Alt Char") && s.contains("Tester"), "{s}");
    let saved = Accounts::load();
    assert_eq!(saved.accounts[0].account, "acct");
    assert_eq!(crate::config::deobfuscate(&saved.accounts[0].password), "secret");

    // Pick "Tester" (default character is preselected).
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "online", |a| a.conn_state == ConnState::Online);
    assert_eq!(app.chat.me, "Tester");
    assert!(app.chat.is_friend("Friendo"));
    assert!(app.chat.is_ignored("pest"));

    // Online friends and bookmarks in the console tab, with their status.
    let s = screen(&mut app, 120, 30);
    show(&s);
    assert!(s.contains("Online (2)") && s.contains("Friendo") && s.contains("Looking") && s.contains("Booky"), "{s}");
    // The header doesn't repeat the count while the panel is shown.
    assert!(s.contains("3 users online") && !s.contains("bookmarks/friends online"), "{s}");
    app.narrow_console = true;
    show(&screen(&mut app, 80, 25));
    app.narrow_console = false;

    // Mouse support (off by default): clicking a key hint presses the key,
    // clicking a friend opens the user menu.
    assert!(!Settings::default().mouse);
    app.settings.mouse = true;
    let click = |app: &mut App, column: u16, row: u16| {
        app.handle_terminal_event(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    };
    let s = screen(&mut app, 120, 30);
    let status = s.lines().nth(29).unwrap();
    let col = status[..status.find("F1 Help").unwrap()].chars().count();
    click(&mut app, col as u16, 29);
    assert!(screen(&mut app, 120, 30).contains("Channel browser"), "F1 click should open the help");
    key(&mut app, KeyCode::Esc);
    let s = screen(&mut app, 120, 30);
    let (row, line) = s.lines().enumerate().find(|(_, l)| l.contains("♥ Friendo")).unwrap();
    let col = line[..line.find("♥ Friendo").unwrap()].chars().count();
    click(&mut app, col as u16, row as u16);
    assert!(matches!(app.dialogs.last(), Some(super::dialogs::Dialog::Menu { .. })));
    key(&mut app, KeyCode::Esc);
    app.settings.mouse = false;

    // An old log of a channel shows when it is opened.
    let old = crate::state::Message::new(crate::state::MsgKind::Chat, "Friendo", "hello from last time");
    crate::logs::append("Tester", "Frontpage", "Frontpage", "channel", &old);

    // Command autocompletion.
    type_text(&mut app, "/jo");
    show(&screen(&mut app, 120, 30));
    assert!(app.completion.as_ref().map(|c| c.items.iter().any(|i| i.0 == "/join")).unwrap_or(false));
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), "/join ");
    app.input.clear();

    // The full command list must fit on small screens (80x25 is the Windows 9x console).
    type_text(&mut app, "/");
    for (w, h) in [(80, 25), (80, 50), (40, 12), (20, 8)] {
        screen(&mut app, w, h);
    }
    app.input.clear();
    app.completion = None;

    // Join a channel through the command.
    type_text(&mut app, "/join Frontpage");
    app.completion = None;
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "joined", |a| {
        a.chat.find_channel("Frontpage").map(|i| a.chat.conversations[i].members.len() == 2).unwrap_or(false)
    });
    assert_eq!(app.chat.active_conv().title, "Frontpage");
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("hello from last time") && s.contains("Earlier messages from the log"), "{s}");

    // Online/offline notifications at the top right (off by default).
    app.watch_toast("Friendo", false);
    assert!(app.toasts.is_empty());
    app.settings.notify_online = crate::config::WatchNotify::Friends;
    app.watch_toast("Booky", true);
    assert!(app.toasts.is_empty(), "Booky is a bookmark, not a friend");
    app.watch_toast("Friendo", false);
    let s = screen(&mut app, 120, 30);
    show(&s);
    let first = s.lines().next().unwrap();
    assert!(first.contains("Notifications") && s.contains("Friendo went offline"), "{s}");
    // Not in the console tab, which has the online panel.
    app.switch_to(0);
    assert!(!screen(&mut app, 120, 30).contains("Friendo went offline"));
    app.switch_to(app.chat.find_channel("Frontpage").unwrap());
    // They go away after a while.
    app.toasts[0].2 -= std::time::Duration::from_secs(60);
    app.tick();
    assert!(app.toasts.is_empty());
    app.settings.notify_online = crate::config::WatchNotify::Off;
    // Compact layout of the Windows 9x console build.
    app.narrow_console = true;
    let s = screen(&mut app, 80, 25);
    show(&s);
    assert!(s.contains("2 # Frontpage") && s.contains("You joined Frontpage"), "{s}");
    app.narrow_console = false;

    // Name completion in normal text.
    type_text(&mut app, "hi Fri");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), "hi Friendo");
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "echo", |a| a.chat.active_conv().messages.iter().any(|m| m.text.starts_with("echo")));
    pump(&mut app, 200);
    let conv = app.chat.active_conv();
    let echo = conv.messages.iter().find(|m| m.text.starts_with("echo")).unwrap();
    assert!(echo.highlight, "own name should highlight");
    let s = screen(&mut app, 120, 30);
    show(&s);
    assert!(s.contains("Welcome to Frontpage"), "{s}");
    assert!(s.contains("echo hi Friendo"), "{s}");
    assert!(s.contains("Looking for RP!"), "{s}");
    assert!(s.contains("Users (2)"), "{s}");

    // Dice.
    type_text(&mut app, "/roll 1d20");
    app.completion = None;
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "roll", |a| a.chat.active_conv().messages.iter().any(|m| m.text.contains("rolls 1d20")));

    // Private chat with typing notification and /me.
    type_text(&mut app, "/priv Friendo");
    app.completion = None;
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.chat.active_conv().partner(), Some("Friendo"));
    type_text(&mut app, "hello there");
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "pm reply", |a| a.chat.active_conv().messages.iter().any(|m| m.text.contains("waves back")));
    let s = screen(&mut app, 120, 30);
    show(&s);
    assert!(s.contains("* Friendo waves back"), "{s}");

    // Status dialog via command.
    type_text(&mut app, "/status looking Open for scenes");
    app.completion = None;
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "status", |a| a.my_status == crate::state::Status::Looking);
    assert_eq!(app.my_status_msg, "Open for scenes");

    // Channel browser.
    key(&mut app, KeyCode::F(2));
    pump(&mut app, 200);
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("Writing") && s.contains("Public channels (2)"), "{s}");
    key(&mut app, KeyCode::Tab);
    assert!(screen(&mut app, 120, 30).contains("Cozy Room"));
    key(&mut app, KeyCode::Esc);

    // Profile viewer.
    type_text(&mut app, "/profile Friendo");
    app.completion = None;
    key(&mut app, KeyCode::Enter);
    pump(&mut app, 500);
    let s = screen(&mut app, 120, 40);
    show(&s);
    assert!(s.contains("Profile: Friendo") && s.contains("Views: 42"), "{s}");
    assert!(s.contains("Gender: Female"), "{s}");
    key(&mut app, KeyCode::Right);
    let s = screen(&mut app, 120, 40);
    assert!(s.contains("Hello I am a test."), "{s}");
    key(&mut app, KeyCode::Right);
    assert!(screen(&mut app, 120, 40).contains("Hugging"));
    key(&mut app, KeyCode::Esc);

    // Friends dialog.
    key(&mut app, KeyCode::F(3));
    pump(&mut app, 400);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    assert!(screen(&mut app, 120, 30).contains("Stranger"));
    key(&mut app, KeyCode::Esc);

    // Logs were written and are browsable.
    let logs = crate::logs::conversations("Tester");
    assert!(logs.iter().any(|(_, t)| t == "Frontpage"), "{logs:?}");
    key(&mut app, KeyCode::F(7));
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("Logs: Friendo"), "{s}");
    key(&mut app, KeyCode::Enter);
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("hello there") && s.contains("waves back"), "{s}");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(screen(&mut app, 120, 30).contains("Frontpage"));
    key(&mut app, KeyCode::Esc);
    assert!(app.dialogs.is_empty());

    // Recent conversations come from the logs.
    let recent = crate::logs::recent("Tester", 10);
    assert!(recent.iter().any(|r| r.1 == "Friendo" && r.2 == "private"), "{recent:?}");
    assert!(recent.iter().any(|r| r.0 == "Frontpage" && r.2 == "channel"), "{recent:?}");

    // Help and settings render.
    key(&mut app, KeyCode::F(1));
    assert!(screen(&mut app, 120, 30).contains("Channel browser"));
    for _ in 0..5 {
        key(&mut app, KeyCode::PageDown);
    }
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("/timeout"), "{s}");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::F(6));
    assert!(screen(&mut app, 120, 50).contains("image_mode"));
    key(&mut app, KeyCode::Esc);

    // Leaving remembers channels for next time.
    let state = crate::config::CharacterState::load("Tester");
    assert!(state.channels.iter().any(|c| c.0 == "Frontpage"));
    assert!(state.private_chats.iter().any(|c| c == "Friendo"));

    let sent = log.lock().unwrap().join("\n");
    assert!(sent.contains("TPN {\"character\":\"Friendo\",\"status\":\"typing\"}"), "{sent}");
    assert!(sent.contains("STA {\"status\":\"looking\",\"statusmsg\":\"Open for scenes\"}"), "{sent}");
    app.disconnect();
    let _ = std::fs::remove_dir_all(&dir);
}
