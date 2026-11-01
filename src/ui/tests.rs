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

/// Website requests the mock server got: "path body".
static SITE_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

const MESSAGES_PAGE: &str = r#"<html><head><meta name="csrf-token" content="tok123"></head><body>
<a href="https://www.f-list.net/messages.php" class="ImageLink" id="NavigationMessages">1 Messages</a>
<a href="https://www.f-list.net/read_notes.php" id="NavigationNotecount" class="ImageLink">1 Notes</a>
<a href="https://www.f-list.net/action/script_logout.php" id="NavigationLogout">Logout</a>
<a href="?show=friends" id="Messagecenter_friends">Friend requests (0)</a>
<a href="?show=guestbookpost" id="Messagecenter_guestbookpostCategory">Guestbook posts (1)</a>
<td id="Content"><h2>Message center: Guestbook posts (Showing message 1 to 20)</h2>
<input type="hidden" id="VarMessageShow" value="guestbookpost"/>
<table class='content-navigation'><tr><td><a class='next' href="?offset=20&show=guestbookpost">Next 20</a></td></tr></table>
<div class="Messagecenter_Message" id="Messagecenter_Message55"><a href='https://www.f-list.net/c/Friendo/'>Friendo</a> posted in your guestbook: <b>Nice profile!</b></div>
<div class='Messagecenter_Message MessagesGroupcharacterupdate' id='Messagecenter_Message56'><a class='ImageLink Messagecenter_Deletelink' id='Messagecenter_Deletelink56' onclick='FList.Messages_delete(56);'>Delete</a><span class='message-summary'><a onclick="x">"Fresh look", 2h ago</a> by <a href='c/friendo/'>Friendo</a></span></div><div class='message-expanded FormattedBlock' id='message-expanded-56' style='display:none;'>V:2 D&gt;:+10 AU</div>
</td><a name="bottom-of-page"></a></body></html>"#;

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
    let mut cookie = String::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        if h.trim().is_empty() {
            break;
        }
        if let Some(v) = h.to_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if h.to_lowercase().starts_with("cookie:") {
            cookie = h[7..].trim().to_string();
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    let body = String::from_utf8_lossy(&body).to_string();
    let route = path.split('?').next().unwrap_or("").to_string();
    let logged_in = cookie.contains("FLS=ok");
    if !route.starts_with("/json/api/") && route != "/json/getApiTicket.php" {
        SITE_LOG.lock().unwrap().push(format!("{path} {body}"));
    }
    // The website (login with cookies, HTML pages, website JSON).
    let html = |b: &str| ("200 OK", "text/html", String::new(), b.to_string());
    let site: Option<(&str, &str, String, String)> = match route.as_str() {
        "/" => Some(html(r#"<form><input type="hidden" name="csrf_token" value="login-token"/></form>"#)),
        "/action/script_login.php" if body.contains("csrf_token=login-token") && body.contains("password=secret") => {
            Some(("302 Found", "text/html", "Location: /\r\nSet-Cookie: __Host-FLS=ok; path=/; secure\r\n".into(), String::new()))
        }
        "/action/script_login.php" => Some(html("<span id='DisplayedMessage'>Login Failed. Try again.</span>")),
        _ if !logged_in && !route.starts_with("/json/api/") && route != "/json/getApiTicket.php" => {
            Some(("200 OK", "application/json", String::new(), json!({"error": "You have to be logged in."}).to_string()))
        }
        "/messages.php" => Some(html(MESSAGES_PAGE)),
        "/view_note.php" => Some(html(
            "<a href='/action/script_logout.php'>Logout</a><h2 id='Note_PageHeader' style=''>Hello there</h2>Sent 5m ago, by <a href='https://www.f-list.net/c/Friendo/'>Friendo</a> \
             to <a href='https://www.f-list.net/c/Tester/'>Tester</a><br/><br/><div class='FormattedBlock' style=''>[b]Hi[/b] Tester,\nline two &amp; more</div></div>\
             <div class=\"panel StyledForm\">",
        )),
        "/json/notes-getfolders.json" => Some(("200 OK", "application/json", String::new(), json!({"folders": [
            {"name": "Inbox", "folder_id": 1, "notecount": 1, "unreadcount": 1},
            {"name": "Outbox", "folder_id": 2, "notecount": 0, "unreadcount": 0},
            {"name": "Trash", "folder_id": 3, "notecount": 0, "unreadcount": 0},
            {"name": "Stories", "folder_id": 9, "notecount": 0, "unreadcount": 0}], "error": ""}).to_string())),
        "/json/notes-get.json" | "/json/notes-getunread.json" | "/json/notes-searchuser.json" => Some(("200 OK", "application/json", String::new(), json!({"notes": [
            {"note_id": 7, "title": "Hello there", "source_name": "Friendo", "dest_name": "Tester", "datetime_sent": "5m ago", "folder_id": 1, "read": 0, "starred": 0}],
            "total": 1, "error": ""}).to_string())),
        "/json/getNewsposts.json" => Some(("200 OK", "application/json", String::new(), json!({"posts": [
            {"title": "Server update", "name": "Staffer", "content": "Things are [b]better[/b] now.", "datetime_posted": "1d ago",
             "datetime_posted_timestamp": 1700000000, "commentcount": 3}], "count": 1, "error": ""}).to_string())),
        "/json/personals-get.json" => Some(("200 OK", "application/json", String::new(), json!({"characters": [
            {"name": "Adwriter", "gender": "Female", "orientation": "Gay", "role": "Switch", "pref": "Furs", "age": "25", "message": "Looking for [i]space[/i] RP"},
            {"name": "Other Ad", "gender": "Male", "orientation": "Straight", "role": "Dom", "pref": "", "age": "", "message": "Fantasy stories"}],
            "error": ""}).to_string())),
        "/json/profile-groups.json" => Some(("200 OK", "application/json", String::new(), json!({"groups": ["Writers Guild", "Sci-Fi Fans"], "error": ""}).to_string())),
        r if r.starts_with("/json/") && !r.starts_with("/json/api/") && r != "/json/getApiTicket.php" => {
            // Changes (POSTs) need the token from the page's csrf meta tag.
            let missing_csrf = !body.is_empty() && !body.contains("csrf_token=tok123") && (r.contains("send") || r.contains("post"));
            let e = if missing_csrf { "missing csrf token" } else { "" };
            Some(("200 OK", "application/json", String::new(), json!({"error": e}).to_string()))
        }
        _ => None,
    };
    let (status, ctype, headers, body) = match site {
        Some(x) => x,
        None => {
            let reply = match route.as_str() {
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
                    "id": if body.contains("name=Tester") { 1 } else { 5 }, "name": "Friendo", "description": "[b]Hello[/b] I am a test.", "views": 42, "custom_title": null,
                    "created_at": 1600000000, "updated_at": 1700000000, "badges": [],
                    "kinks": {"1": "fave"}, "custom_kinks": {}, "infotags": {"3": "9"}, "images": [], "error": ""}),
                "/json/api/friend-bookmark-lists.php" => {
                    json!({"bookmarklist": ["Booky"], "friendlist": [{"source": "Friendo", "dest": "Tester", "last_online": 0}],
                    "requestlist": [{"id": "77", "source": "Stranger", "dest": "Tester"}], "requestpending": [], "error": ""})
                }
                _ => json!({"error": ""}),
            };
            ("200 OK", "application/json", String::new(), reply.to_string())
        }
    };
    let mut s = s;
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
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

fn key_mod(app: &mut App, code: KeyCode, m: KeyModifiers) {
    app.handle_terminal_event(Event::Key(KeyEvent::new(code, m)));
}

fn command(app: &mut App, s: &str) {
    type_text(app, s);
    app.completion = None;
    key(app, KeyCode::Enter);
}

fn site_log() -> String {
    SITE_LOG.lock().unwrap().join("\n")
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
    let mut found = false;
    for _ in 0..12 {
        key(&mut app, KeyCode::PageDown);
        found |= screen(&mut app, 120, 30).contains("/timeout");
    }
    assert!(found, "/timeout should be in the help");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::F(6));
    assert!(screen(&mut app, 120, 50).contains("image_mode"));
    key(&mut app, KeyCode::Esc);

    new_features(&mut app);

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

/// Ad cooldown, "new messages" line, in-tab search, log search and the website features.
fn new_features(app: &mut App) {
    let front = app.chat.find_channel("Frontpage").unwrap();
    app.switch_to(front);

    // Ad cooldown: the first ad starts a timer shown in the input box; a second one is refused.
    command(app, "/ad Looking for a scene");
    pump(app, 200);
    let s = screen(app, 120, 30);
    assert!(s.contains("next ad 10:00") || s.contains("next ad 9:5"), "{s}");
    command(app, "/ad Again");
    let last = app.chat.active_conv().messages.back().unwrap().text.clone();
    assert!(last.starts_with("You can post the next ad here in"), "{last}");

    // "New messages" line above messages that came while the tab was not shown.
    app.switch_to(0);
    app.add_message(front, crate::state::Message::new(crate::state::MsgKind::Chat, "Friendo", "said while you were away"));
    app.switch_to(front);
    let s = screen(app, 120, 30);
    show(&s);
    let marker = s.lines().position(|l| l.contains(" new messages ")).expect("marker line");
    let msg = s.lines().position(|l| l.contains("said while you were away")).unwrap();
    assert!(marker + 1 == msg, "{s}");
    app.switch_to(0);
    app.switch_to(front);
    assert!(!screen(app, 120, 30).contains(" new messages "));

    // In-tab search: Ctrl+F, older/newer, Esc.
    key_mod(app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    type_text(app, "echo");
    key(app, KeyCode::Enter);
    let hit = app.chat.active_conv().find_hit.expect("search hit");
    assert!(app.chat.active_conv().messages[hit].text.contains("echo hi"));
    let s = screen(app, 120, 30);
    assert!(s.contains("find \"echo\""), "{s}");
    key_mod(app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert_eq!(app.chat.active_conv().find_hit, Some(hit), "only one match");
    key(app, KeyCode::Esc);
    assert!(app.find.is_none() && app.chat.active_conv().find_hit.is_none());

    // Search all logs.
    command(app, "/logsearch waves back");
    let s = screen(app, 120, 30);
    show(&s);
    assert!(s.contains("Log search: 1 result ") && s.contains("Friendo"), "{s}");
    key(app, KeyCode::Enter);
    let s = screen(app, 120, 30);
    assert!(s.contains("Logs: Friendo") && s.contains("waves back"), "{s}");
    key(app, KeyCode::Esc);
    assert!(screen(app, 120, 30).contains("Log search"));
    key(app, KeyCode::Esc);
    assert!(app.dialogs.is_empty());

    // Website counters in the status bar.
    app.site_poll_at = Some(Instant::now());
    app.tick();
    pump_until(app, "counters", |a| a.site_counters.notes == 1);
    assert_eq!(app.site_counters.messages, 1);
    let s = screen(app, 160, 30);
    assert!(s.contains("✉ 1 note (Alt+N)") && s.contains("1 site message"), "{s}");

    // Notes: list, read, reply.
    key_mod(app, KeyCode::Char('n'), KeyModifiers::ALT);
    pump_until(
        app,
        "notes",
        |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Notes(n)) if n.notes.is_some() && !n.folders.is_empty()),
    );
    let s = screen(app, 120, 30);
    show(&s);
    assert!(s.contains("Hello there") && s.contains("Stories") && s.contains("Inbox 1"), "{s}");
    key(app, KeyCode::Enter);
    pump_until(app, "note", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Notes(n)) if matches!(n.view, Some(Some(_)))));
    let s = screen(app, 120, 30);
    show(&s);
    assert!(s.contains("From Friendo to Tester") && s.contains("Hi Tester,") && s.contains("line two & more"), "{s}");
    type_text(app, "r");
    type_text(app, "Thanks!");
    key_mod(app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    pump_until(app, "note sent", |_| site_log().contains("/json/notes-send.json"));
    pump(app, 200);
    let log = site_log();
    assert!(log.contains("dest=Friendo") && log.contains("source=1") && log.contains("title=Re%3A+Hello+there"), "{log}");
    assert!(app.flash.as_ref().map(|f| f.0 == "Note sent to Friendo.").unwrap_or(false), "{:?}", app.flash);
    key(app, KeyCode::Esc);
    key(app, KeyCode::Esc);
    assert!(app.dialogs.is_empty());

    // Message center.
    key_mod(app, KeyCode::Char('m'), KeyModifiers::ALT);
    pump_until(app, "messages", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Messages(m)) if m.data.is_some()));
    let s = screen(app, 120, 30);
    show(&s);
    assert!(s.contains("Guestbook posts 1") && s.contains("Nice profile!"), "{s}");
    assert!(s.contains("Site messages: message 1 to 20") && s.contains("\"Fresh look\", 2h ago by Friendo"), "{s}");
    key(app, KeyCode::Tab);
    key(app, KeyCode::Down);
    let s = screen(app, 120, 30);
    assert!(s.contains("Description changed (+10 characters)") && s.contains("New avatar") && !s.contains("Delete\""), "{s}");
    key(app, KeyCode::PageDown);
    pump_until(app, "next page", |_| site_log().contains("/messages.php?offset=20&show=guestbookpost"));
    pump_until(
        app,
        "messages",
        |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Messages(m)) if m.data.as_ref().is_some_and(|d| d.is_ok())),
    );
    type_text(app, "d");
    pump_until(app, "delete message", |_| site_log().contains("/json/deleteMessage.json?message_id=55"));
    key(app, KeyCode::Esc);

    // Roleplay ads from the front page, filtered.
    command(app, "/personals space");
    pump_until(app, "personals", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Personals(p)) if p.data.is_some()));
    let s = screen(app, 120, 30);
    show(&s);
    assert!(s.contains("Adwriter") && s.contains("Looking for space RP") && !s.contains("Other Ad"), "{s}");
    key(app, KeyCode::Esc);
    key(app, KeyCode::Esc);
    assert!(app.dialogs.is_empty());

    // News.
    command(app, "/news");
    pump_until(app, "news", |a| !a.dialogs.is_empty());
    let s = screen(app, 120, 30);
    assert!(s.contains("Server update") && s.contains("Things are better now."), "{s}");
    key(app, KeyCode::Esc);

    // Eicon picker and [eicon] completion (with a small list instead of the download).
    app.eicon_index = Some(Arc::new(crate::eicons::parse("wave\t1\nwavehello\t2\nhug\t3\n# As Of: 1\n")));
    key_mod(app, KeyCode::Char('e'), KeyModifiers::ALT);
    type_text(app, "wav");
    let s = screen(app, 120, 30);
    assert!(s.contains("Eicons · 2 of 3") && s.contains("wavehello") && !s.contains("hug"), "{s}");
    key(app, KeyCode::Down);
    key(app, KeyCode::Enter);
    assert_eq!(app.input.text(), "[eicon]wavehello[/eicon]");
    app.input.clear();
    type_text(app, "hi [eicon]hu");
    assert_eq!(app.completion.as_ref().map(|c| c.items[0].0.clone()), Some("hug[/eicon]".to_string()));
    key(app, KeyCode::Tab);
    assert_eq!(app.input.text(), "hi [eicon]hug[/eicon]");
    app.input.clear();
    app.completion = None;

    // Profile: groups tab and signing the guestbook.
    command(app, "/profile Friendo");
    pump(app, 300);
    // Kink match against your own profile (the mock gives everyone the same kinks).
    let s = screen(app, 120, 40);
    assert!(s.contains("Kink match: 1 liked by both (1 favorite), 0 conflicts"), "{s}");
    type_text(app, "3");
    let s = screen(app, 120, 40);
    assert!(s.contains("✓ Hugging") && s.contains("you like it too"), "{s}");
    type_text(app, "7");
    pump_until(app, "groups", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Profile(p)) if p.groups.is_some()));
    let s = screen(app, 120, 40);
    assert!(s.contains("Writers Guild") && s.contains("Sci-Fi Fans"), "{s}");
    type_text(app, "6");
    type_text(app, "w");
    type_text(app, "Lovely profile");
    key_mod(app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(screen(app, 120, 40).contains("Post publicly"));
    key(app, KeyCode::Enter);
    pump_until(app, "guestbook post", |_| site_log().contains("/json/guestbook-post.json"));
    let log = site_log();
    assert!(log.contains("character_id=5&source_character_id=1&private=false&message=Lovely+profile"), "{log}");
    pump(app, 200);
    key(app, KeyCode::Esc);
    assert!(app.dialogs.is_empty());
}

/// The real app against the real F-Chat server and website, read-only:
/// FLIST_TEST_ACCOUNT=… FLIST_TEST_PASSWORD=… cargo test -- --ignored live_app --nocapture
#[test]
#[ignore]
fn live_app() {
    let (Ok(account), Ok(password)) = (std::env::var("FLIST_TEST_ACCOUNT"), std::env::var("FLIST_TEST_PASSWORD")) else { return };
    let dir = std::env::temp_dir().join(format!("f-term-live-{}", std::process::id()));
    crate::paths::init(Some(dir.clone()));
    let settings = Settings { idle_minutes: 0, rejoin_channels: false, reopen_private_chats: false, ..Default::default() };
    let mut app = App::new(settings, Accounts::default(), Options { account: None, character: None, no_autologin: true });
    let slow = |app: &mut App, what: &str, f: &dyn Fn(&App) -> bool| {
        let end = Instant::now() + Duration::from_secs(45);
        while !f(app) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            pump(app, 50);
        }
    };
    app.start_login(&account, &password);
    slow(&mut app, "login", &|a| a.login.as_ref().map(|l| l.stage == super::login::Stage::Character).unwrap_or(true));
    let character = app.login_info.as_ref().unwrap().characters[0].clone();
    app.choose_character(&character);
    slow(&mut app, "online", &|a| a.conn_state == ConnState::Online);
    println!("online as {}, {} users", app.chat.me, app.chat.user_count);

    // Counters.
    app.site_poll_at = Some(Instant::now());
    app.tick();
    pump(&mut app, 4000);
    println!("counters: {:?}", app.site_counters);

    // Notes and the first note.
    key_mod(&mut app, KeyCode::Char('n'), KeyModifiers::ALT);
    slow(
        &mut app,
        "notes",
        &|a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Notes(n)) if n.notes.is_some() && !n.folders.is_empty()),
    );
    let s = screen(&mut app, 120, 30);
    println!("{s}");
    key(&mut app, KeyCode::Enter);
    slow(
        &mut app,
        "note",
        &|a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Notes(n)) if matches!(n.view, Some(Some(_)) | None)),
    );
    println!("{}", screen(&mut app, 120, 20));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);

    // Site messages.
    key_mod(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    slow(&mut app, "messages", &|a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Messages(m)) if m.data.is_some()));
    let s = screen(&mut app, 120, 20);
    println!("{s}");
    assert!(s.contains("Guestbook posts"), "{s}");
    key(&mut app, KeyCode::Esc);

    // Roleplay ads and news.
    command(&mut app, "/personals");
    slow(&mut app, "personals", &|a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Personals(p)) if p.data.is_some()));
    let s = screen(&mut app, 120, 30);
    println!("{s}");
    assert!(s.contains("Roleplay ads from the F-List front page"), "{s}");
    key(&mut app, KeyCode::Esc);
    command(&mut app, "/news");
    slow(&mut app, "news", &|a| !a.dialogs.is_empty());
    println!("{}", screen(&mut app, 120, 20));
    key(&mut app, KeyCode::Esc);

    // A profile: kink match and groups.
    command(&mut app, "/profile TomTheDragon");
    pump(&mut app, 4000);
    let s = screen(&mut app, 120, 30);
    println!("{s}");
    type_text(&mut app, "7");
    slow(&mut app, "groups", &|a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Profile(p)) if p.groups.is_some()));
    let s = screen(&mut app, 120, 30);
    println!("{s}");
    assert!(s.contains("groups on F-List") || s.contains("Not in any groups"), "{s}");
    key(&mut app, KeyCode::Esc);
    app.disconnect();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Renders the README screenshots (docs/screenshots/*.png) with made-up
/// characters through the window renderer:
/// cargo test --features gui -- --ignored screenshots
#[cfg(all(feature = "gui", feature = "images", not(windows)))]
#[test]
#[ignore]
fn screenshots() {
    use super::site::SiteReply;
    use crate::site::{MessageCenter, NoteFolder, NoteList, NoteSummary};
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = std::env::temp_dir().join(format!("f-term-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crate::paths::init(Some(dir.clone()));
    // Kink and info lists for the profile.
    let kinks = [
        "Cuddling",
        "Romance",
        "Slow Burn",
        "Long Posts",
        "Worldbuilding",
        "Banter",
        "Teasing",
        "Size Difference",
        "Magic",
        "Space Opera",
        "Mystery",
        "Horror",
        "Short Posts",
        "Time Travel",
        "Mecha",
        "Body Swap",
    ];
    let mapping = json!({
        "kinks": kinks.iter().enumerate().map(|(i, k)| json!({"id": (i + 1).to_string(), "name": k, "description": "", "group_id": "1"})).collect::<Vec<_>>(),
        "kink_groups": [{"id": "1", "name": "General"}],
        "infotags": [
            {"id": "1", "name": "Age", "type": "text", "list": "", "group_id": "1"},
            {"id": "2", "name": "Species", "type": "text", "list": "", "group_id": "1"},
            {"id": "3", "name": "Gender", "type": "list", "list": "gender", "group_id": "1"},
            {"id": "4", "name": "Height", "type": "text", "list": "", "group_id": "1"},
            {"id": "5", "name": "Post Length", "type": "list", "list": "postlength", "group_id": "2"},
            {"id": "6", "name": "Desired RP length", "type": "list", "list": "rplength", "group_id": "2"}],
        "infotag_groups": [{"id": "1", "name": "General details"}, {"id": "2", "name": "RPing preferences"}],
        "listitems": [{"id": "9", "name": "gender", "value": "Female"}, {"id": "10", "name": "postlength", "value": "Multi-paragraph"},
                      {"id": "11", "name": "rplength", "value": "Long-term"}],
        "error": ""});
    std::fs::create_dir_all(crate::paths::cache_dir()).unwrap();
    std::fs::write(crate::paths::cache_dir().join("mapping-list.json"), mapping.to_string()).unwrap();

    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let http = http_server();
    let ws = ws_server(log);
    let settings = Settings {
        api_base: format!("http://127.0.0.1:{http}"),
        server: format!("ws://127.0.0.1:{ws}"),
        idle_minutes: 0,
        image_mode: crate::config::ImageMode::Blocks,
        truecolor: true,
        inline_images: true,
        inline_image_rows: 4,
        ..Default::default()
    };
    let mut app = App::new(settings, Accounts::default(), Options { account: None, character: None, no_autologin: true });
    // Made-up pictures instead of downloads.
    let put = |app: &mut App, url: String, p: crate::images::Pixels| {
        let frames = crate::images::Frames { frames: vec![(Arc::new(p), 0)], total_ms: 0 };
        app.images.finish(crate::images::Loaded { url, result: Ok(Arc::new(frames)) });
    };
    let palettes: [(&str, [u8; 3], [u8; 3], [u8; 3]); 9] = [
        ("Vela Starling", [40, 60, 140], [230, 120, 170], [250, 220, 200]),
        ("Orrin Vale", [20, 70, 60], [90, 200, 160], [210, 170, 140]),
        ("Kestrel Moss", [90, 50, 20], [240, 170, 60], [160, 110, 80]),
        ("Quill", [60, 30, 90], [180, 120, 240], [230, 230, 240]),
        ("Brannoc Ironleaf", [50, 50, 60], [140, 160, 200], [200, 150, 120]),
        ("Mira Solace", [100, 20, 60], [250, 140, 120], [240, 200, 180]),
        ("Tamsin Gray", [30, 30, 30], [150, 150, 170], [235, 210, 190]),
        ("Ash Wren", [20, 40, 80], [80, 160, 240], [190, 140, 110]),
        ("Tester", [30, 60, 30], [160, 220, 90], [220, 180, 150]),
    ];
    for (name, bg, accent, skin) in palettes {
        put(&mut app, crate::images::avatar_url(name), avatar_picture(bg, accent, skin));
    }
    for (name, pic) in [("heart", eicon_heart()), ("star", eicon_star()), ("smile", eicon_smile())] {
        put(&mut app, crate::images::eicon_url(name), pic);
    }
    put(&mut app, crate::images::charimage_url("101", "png"), space_picture(480, 270));
    let shot = |app: &mut App, name: &str, cols: u16, rows: u16, pixels: bool| {
        app.images.begin_frame();
        app.images.clear_render_cache();
        crate::ui::gui::save_png(cols, rows, 16.0, pixels, &out.join(name), |f| app.draw(f));
    };
    let srv = |app: &mut App, line: String| app.on_server(crate::protocol::parse(&line).unwrap());

    shot(&mut app, "login.png", 100, 26, false);
    type_text(&mut app, "acct");
    key(&mut app, KeyCode::Enter);
    app.login.as_mut().unwrap().password.set("secret");
    app.login.as_mut().unwrap().focus = 4;
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "characters", |a| a.login.as_ref().map(|l| l.stage == super::login::Stage::Character).unwrap_or(false));
    key(&mut app, KeyCode::Enter);
    pump_until(&mut app, "online", |a| a.conn_state == ConnState::Online);

    // People, a channel and some chat.
    let people = [
        ("Vela Starling", "Female", "looking", "Up for a space adventure!"),
        ("Orrin Vale", "Male", "online", ""),
        ("Kestrel Moss", "Transgender", "busy", "Writing a reply"),
        ("Quill", "None", "away", "Back in 10"),
        ("Brannoc Ironleaf", "Male", "online", ""),
        ("Mira Solace", "Herm", "looking", "Fantasy or sci-fi"),
        ("Tamsin Gray", "Female", "dnd", ""),
        ("Ash Wren", "Male-Herm", "idle", ""),
    ];
    let lis: Vec<_> = people.iter().map(|p| json!([p.0, p.1, p.2, p.3])).collect();
    srv(&mut app, format!("LIS {}", json!({"characters": lis})));
    srv(&mut app, r#"FRL {"characters":["Vela Starling","Quill"]}"#.into());
    app.chat.friends.extend(["vela starling".to_string(), "quill".to_string()]);
    app.chat.bookmarks.insert("orrin vale".to_string());
    command(&mut app, "/join Sci-Fi Roleplay");
    pump_until(&mut app, "joined", |a| a.chat.find_channel("Sci-Fi Roleplay").is_some());
    let ch = "Sci-Fi Roleplay";
    for p in &people {
        srv(&mut app, format!("JCH {}", json!({"channel": ch, "character": {"identity": p.0}, "title": ch})));
    }
    srv(&mut app, format!("LCH {}", json!({"channel": ch, "character": "Friendo"})));
    srv(&mut app, format!("COL {}", json!({"channel": ch, "oplist": ["Orrin Vale"]})));
    srv(
        &mut app,
        format!(
            "CDS {}",
            json!({"channel": ch, "description": "[b]Welcome aboard![/b] Starships, aliens and [i]far-away worlds[/i]. Ads every 10 minutes."})
        ),
    );
    pump(&mut app, 200);
    let i = app.chat.find_channel(ch).unwrap();
    app.chat.conversations[i].clear_messages();
    let lines = [
        ("Orrin Vale", "Morning, crew! The station's coffee machine is fixed, finally."),
        ("Vela Starling", "/me drifts into the observation deck, a datapad tucked under one arm."),
        ("Kestrel Moss", "Anyone up for a [b]derelict ship[/b] exploration scene later?"),
        ("Mira Solace", "[color=cyan]Count me in![/color] I'll bring the scanner."),
        ("Brannoc Ironleaf", "Ooh, haunted derelict or pirate derelict?"),
        ("Orrin Vale", "[icon]Vela Starling[/icon] [icon]Kestrel Moss[/icon] Look who's back on the station!"),
        ("Kestrel Moss", "Why not both? 😄"),
        ("Mira Solace", "Group hug before we board! [eicon]heart[/eicon][eicon]star[/eicon][eicon]smile[/eicon]"),
        ("Vela Starling", "Hey Tester, want to join us? Here is the setting: [url=https://example.com/derelict]The Silent Wake[/url]"),
    ];
    for (who, m) in lines {
        srv(&mut app, format!("MSG {}", json!({"channel": ch, "character": who, "message": m})));
    }
    srv(
        &mut app,
        format!(
            "LRP {}",
            json!({"channel": ch, "character": "Quill", "message": "[color=yellow]Looking for[/color] a long-term [b]space opera[/b] partner, multi-paragraph posts. Note me!"})
        ),
    );
    srv(&mut app, format!("PRI {}", json!({"character": "Vela Starling", "message": "Hi! Loved your last post in the bar scene 🙂"})));
    app.site_counters.notes = 2;
    app.site_counters.messages = 3;
    let i = app.chat.find_channel(ch).unwrap();
    app.switch_to(i);
    type_text(&mut app, "Sounds great, I'm in!");
    shot(&mut app, "chat.png", 120, 34, true);
    shot(&mut app, "chat-blocks.png", 120, 34, false);
    app.narrow_console = true;
    shot(&mut app, "console-80x25.png", 80, 25, false);
    app.narrow_console = false;
    app.input.clear();

    // A profile with the kink match.
    app.open_profile("Vela Starling");
    pump_until(&mut app, "profile", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Profile(p)) if p.data.is_some()));
    pump_until(&mut app, "own kinks", |a| a.my_kinks.is_some());
    pump(&mut app, 300);
    let choices = |list: &[(usize, &str)]| list.iter().map(|(i, c)| ((i + 1).to_string(), json!(c))).collect::<serde_json::Map<_, _>>();
    let mine = choices(&[(0, "fave"), (1, "fave"), (2, "yes"), (3, "yes"), (4, "fave"), (5, "yes"), (9, "fave"), (11, "no"), (12, "no")]);
    app.my_kinks = Some(Arc::new(super::profile::my_kinks_from(&json!({"id": 1, "name": "Tester", "kinks": mine, "custom_kinks": {}}))));
    let hers = choices(&[
        (0, "fave"),
        (1, "fave"),
        (2, "fave"),
        (3, "yes"),
        (4, "yes"),
        (5, "yes"),
        (6, "maybe"),
        (7, "maybe"),
        (8, "yes"),
        (9, "fave"),
        (10, "maybe"),
        (11, "no"),
        (12, "yes"),
        (13, "maybe"),
        (14, "no"),
        (15, "no"),
    ]);
    let vela = Ok(json!({
            "id": 5, "name": "Vela Starling", "custom_title": "Starship Navigator", "views": 1234,
            "created_at": 1600000000, "updated_at": 1760000000,
            "description": "[center][b][color=cyan]Vela Starling[/color][/b]\n[i]Navigator of the survey ship Long Horizon[/i][/center]\n\n\
                Vela grew up on a mining station at the edge of charted space and has been chasing \
                the horizon ever since. Curious, stubborn and terrible at cards.\n\n[b]Looking for:[/b]\n\
                [*] Long-term space opera\n[*] Mystery and exploration\n[*] Slow-burn romance\n\n[quote]Ask me about the Silent Wake.[/quote]",
            "kinks": hers,
            "custom_kinks": {"1": {"name": "Found family", "description": "", "choice": "fave", "children": []}},
            "infotags": {"1": "27", "2": "Human", "3": "9", "4": "1.68 m", "5": "10", "6": "11"},
            "images": [{"image_id": "101", "extension": "png", "description": "The Long Horizon", "sort_order": 1}],
            "badges": [], "error": ""
    }));
    if let Some(super::dialogs::Dialog::Profile(p)) = app.dialogs.last_mut() {
        p.set_data(vela);
    }
    shot(&mut app, "profile.png", 120, 34, true);
    shot(&mut app, "profile-blocks.png", 120, 34, false);
    key(&mut app, KeyCode::Char('3'));
    shot(&mut app, "profile-kinks.png", 120, 34, false);
    key(&mut app, KeyCode::Char('4'));
    shot(&mut app, "profile-images.png", 120, 34, true);
    shot(&mut app, "profile-images-blocks.png", 120, 34, false);
    key(&mut app, KeyCode::Esc);
    app.dialogs.clear();

    // Notes.
    key_mod(&mut app, KeyCode::Char('n'), KeyModifiers::ALT);
    pump_until(&mut app, "notes", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Notes(n)) if n.notes.is_some()));
    pump(&mut app, 300);
    app.on_site(SiteReply::Folders(Ok(vec![
        NoteFolder { id: 1, name: "Inbox".into(), total: 5, unread: 2 },
        NoteFolder { id: 2, name: "Outbox".into(), total: 3, unread: 0 },
        NoteFolder { id: 3, name: "Trash".into(), total: 0, unread: 0 },
        NoteFolder { id: 9, name: "Story ideas".into(), total: 4, unread: 0 },
    ])));
    let note = |id, title: &str, from: &str, sent: &str, read, starred| NoteSummary {
        id,
        title: title.into(),
        from: from.into(),
        to: "Tester".into(),
        sent: sent.into(),
        folder: 1,
        read,
        starred,
    };
    app.on_site(SiteReply::Notes {
        list: NoteList::Folder(1),
        page: 0,
        result: Ok((
            vec![
                note(11, "Re: The Silent Wake", "Vela Starling", "12m ago", false, true),
                note(10, "Ad reply: space opera", "Quill", "1h, 5m ago", false, false),
                note(9, "Plot notes for chapter 3", "Kestrel Moss", "2d ago", true, true),
                note(8, "Thanks for the scene!", "Mira Solace", "1w ago", true, false),
                note(7, "Welcome to the channel", "Orrin Vale", "3w ago", true, false),
            ],
            5,
        )),
    });
    shot(&mut app, "notes.png", 120, 30, false);
    app.dialogs.clear();

    // Site messages: character updates.
    key_mod(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    pump_until(&mut app, "messages", |a| matches!(a.dialogs.last(), Some(super::dialogs::Dialog::Messages(m)) if m.data.is_some()));
    let mut mc = MessageCenter { total: 3, shown: "characterupdate".into(), range: "Showing message 1 to 3".into(), ..Default::default() };
    for (k, l) in crate::site::MESSAGE_CATEGORIES {
        let n = match *k {
            "characterupdate" => 3,
            "guestbookpost" => 1,
            _ => 0,
        };
        mc.categories.push((k.to_string(), l.to_string(), n));
    }
    let update = |title: &str, ago: &str, who: &str, rec: &str| {
        format!("\"{title}\", {ago} ago by [user]{who}[/user]\n{}", crate::site::describe_changes(rec))
    };
    mc.items = vec![
        ("3".into(), update("New ship, new look", "2h, 10m", "Vela Starling", r#"V:2 D>:+412 AU I>:"Height""#)),
        (
            "2".into(),
            update("Kinks updated", "1d, 4h", "Kestrel Moss", r#"V:2 F+:"Mystery",fave F>:"Horror",yes F+:"Magic",maybe C+:"Heists""#),
        ),
        ("1".into(), update("Fixed some typos", "3d", "Orrin Vale", "V:2 D>:-12")),
    ];
    if let Some(super::dialogs::Dialog::Messages(m)) = app.dialogs.last_mut() {
        m.data = Some(Ok(mc));
        m.cat_sel = 2;
        m.focus_items = true;
        m.sel = 1;
    }
    app.site_counters.messages = 3;
    shot(&mut app, "site-messages.png", 120, 30, false);
    let _ = std::fs::remove_dir_all(dir);
}

// ---- made-up pictures for the screenshots ----

#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn picture(w: u32, h: u32, f: impl Fn(f32, f32) -> [u8; 4]) -> crate::images::Pixels {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&f(x as f32 / w as f32, y as f32 / h as f32));
        }
    }
    crate::images::Pixels { width: w, height: h, rgba }
}

#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

/// Head and shoulders on a gradient, like a profile icon.
#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn avatar_picture(bg: [u8; 3], accent: [u8; 3], skin: [u8; 3]) -> crate::images::Pixels {
    picture(100, 100, |x, y| {
        let mut c = mix(bg, accent, y * 0.9);
        // A soft glow behind the head.
        let g = ((x - 0.5).powi(2) + (y - 0.4).powi(2)).sqrt();
        c = mix(c, accent, (0.45 - g) * 1.5);
        let head = ((x - 0.5) / 0.2).powi(2) + ((y - 0.38) / 0.23).powi(2);
        let body = ((x - 0.5) / 0.36).powi(2) + ((y - 1.02) / 0.36).powi(2);
        let hair = ((x - 0.5) / 0.24).powi(2) + ((y - 0.3) / 0.22).powi(2);
        if body < 1.0 {
            c = mix(accent, [20, 20, 30], 0.55 + (y - 0.7));
        } else if head < 1.0 {
            c = if y < 0.3 && hair < 1.0 { mix(accent, [30, 20, 20], 0.6) } else { skin };
        } else if hair < 1.0 && y < 0.36 {
            c = mix(accent, [30, 20, 20], 0.6);
        }
        [c[0], c[1], c[2], 255]
    })
}

#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn eicon_heart() -> crate::images::Pixels {
    picture(64, 64, |x, y| {
        let (px, py) = ((x - 0.5) * 2.6, (0.45 - y) * 2.6);
        let v = (px * px + py * py - 1.0).powi(3) - px * px * py.powi(3);
        if v <= 0.0 {
            let c = mix([255, 90, 120], [190, 20, 60], y * 1.2);
            [c[0], c[1], c[2], 255]
        } else {
            [0, 0, 0, 0]
        }
    })
}

#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn eicon_star() -> crate::images::Pixels {
    picture(64, 64, |x, y| {
        let (dx, dy) = (x - 0.5, y - 0.53);
        let r = (dx * dx + dy * dy).sqrt();
        let a = dy.atan2(dx) + std::f32::consts::FRAC_PI_2;
        let k = (a * 5.0 / std::f32::consts::TAU).fract().abs();
        let edge = 0.2 + 0.26 * (1.0 - (k - 0.5).abs() * 2.0).powf(1.6);
        if r < edge {
            let c = mix([255, 240, 120], [240, 160, 20], r * 2.5);
            [c[0], c[1], c[2], 255]
        } else {
            [0, 0, 0, 0]
        }
    })
}

#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn eicon_smile() -> crate::images::Pixels {
    picture(64, 64, |x, y| {
        let r = ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt();
        if r > 0.46 {
            return [0, 0, 0, 0];
        }
        let eye = |ex: f32| ((x - ex) / 0.06).powi(2) + ((y - 0.38) / 0.1).powi(2) < 1.0;
        let mouth = {
            let m = ((x - 0.5).powi(2) + (y - 0.45).powi(2)).sqrt();
            (0.22..0.29).contains(&m) && y > 0.55
        };
        if eye(0.36) || eye(0.64) || mouth {
            [70, 40, 10, 255]
        } else {
            let c = mix([255, 230, 90], [240, 170, 30], r * 2.0);
            [c[0], c[1], c[2], 255]
        }
    })
}

/// Stars, a ringed planet and a moon.
#[cfg(all(feature = "gui", feature = "images", not(windows)))]
fn space_picture(w: u32, h: u32) -> crate::images::Pixels {
    let aspect = w as f32 / h as f32;
    picture(w, h, |x, y| {
        let mut c = mix([5, 5, 25], [40, 20, 70], y);
        // Stars from a hash of the position.
        let (sx, sy) = ((x * w as f32) as u32, (y * h as f32) as u32);
        let hsh = (sx.wrapping_mul(73856093) ^ sy.wrapping_mul(19349663)).wrapping_mul(2654435761);
        if hsh % 211 == 0 {
            c = [230, 230, 255];
        }
        let (px, py) = ((x - 0.62) * aspect, y - 0.55);
        let d = (px * px + py * py).sqrt();
        // Ring behind and in front of the planet.
        let ring = ((px / 0.5).powi(2) + (py / 0.1 - px * 0.6).powi(2)).sqrt();
        let in_ring = (0.75..1.0).contains(&ring);
        if in_ring && (d > 0.26 || py > -0.02) {
            c = mix([200, 170, 120], [120, 90, 70], ring);
        } else if d < 0.26 {
            let light = (0.26 - ((px + 0.1).powi(2) + (py + 0.1).powi(2)).sqrt()) * 3.0;
            c = mix(mix([60, 90, 160], [90, 170, 200], (py * 12.0).sin() * 0.5 + 0.5), [10, 10, 30], 0.6 - light);
        }
        let (mx, my) = ((x - 0.18) * aspect, y - 0.25);
        if (mx * mx + my * my).sqrt() < 0.07 {
            c = mix([220, 220, 210], [120, 120, 130], mx * 6.0 + 0.5);
        }
        [c[0], c[1], c[2], 255]
    })
}
