//! Terminal user interface: application state and main loop.

mod actions;
mod complete;
mod console16;
mod dialogs;
mod draw;
#[cfg(feature = "gui")]
pub mod gui;
pub mod input;
mod login;
mod profile;
mod server;
mod widgets;

use std::collections::VecDeque;
use std::io::Write;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use console16::ConsoleBackend;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use serde_json::Value;

use crate::api::{Api, FriendLists, LoginInfo, Mapping};
use crate::config::{Accounts, CharacterState, ImageMode, Settings};
use crate::images::{ImageStore, Loaded};
use crate::logs;
use crate::net::{self, Connection, NetEvent};
use crate::protocol;
use crate::state::{Chat, ConvKind, Message, MsgKind, Status, Typing};

pub use dialogs::Dialog;
use input::{EditResult, LineEditor};
use login::LoginScreen;

pub enum AppEvent {
    Net(u64, NetEvent),
    Login(Result<LoginInfo, String>),
    Ticket(u64, Result<String, String>),
    Api(ApiReply),
    Image(Loaded),
}

pub enum ApiReply {
    Profile(String, Result<Value, String>),
    Mapping(Result<Mapping, String>),
    FriendLists(Result<FriendLists, String>),
    Memo(String, Result<String, String>),
    Guestbook(String, u32, Result<Value, String>),
    CharFriends(String, Result<Vec<String>, String>),
    SearchFields(Result<Value, String>),
    Report { character: String, reason: String, result: Result<i64, String> },
    Done(String, Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Offline,
    Connecting,
    Online,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Users,
    Tabs,
}

/// Screen areas from the last frame, for mouse handling.
/// How long online/offline notifications stay.
const TOAST_TIME: Duration = Duration::from_secs(6);

#[derive(Debug, Default, Clone)]
pub struct Hit {
    pub tabs: Rect,
    pub tab_rows: Vec<(u16, usize)>,
    pub users: Rect,
    pub user_rows: Vec<(u16, String)>,
    pub messages: Rect,
    pub input: Rect,
    pub dialog: Rect,
    pub dialog_list: Rect,
    pub dialog_list_offset: usize,
    /// Online friends/bookmarks panel of the console tab.
    pub friends: Rect,
    pub friend_rows: Vec<(u16, String)>,
    /// Login screen: clickable fields with their focus index, and the character list.
    pub login_fields: Vec<(Rect, usize)>,
    pub login_list: Rect,
    pub login_list_offset: usize,
}

pub struct Completion {
    /// Byte offset where the completed word starts.
    pub start: usize,
    pub items: Vec<(String, String)>,
    pub sel: usize,
    /// True when the user cycles with Tab (input already shows the item).
    pub cycling: bool,
}

pub struct Options {
    pub account: Option<String>,
    pub character: Option<String>,
    pub no_autologin: bool,
}

pub struct App {
    pub settings: Settings,
    pub accounts: Accounts,
    pub login: Option<LoginScreen>,
    pub chat: Chat,
    pub char_state: CharacterState,
    pub api: Option<Api>,
    pub login_info: Option<LoginInfo>,
    pub conn: Option<Connection>,
    pub conn_id: u64,
    pub conn_state: ConnState,
    pub tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
    pub input: LineEditor,
    pub dialogs: Vec<Dialog>,
    pub images: ImageStore,
    pub focus: Focus,
    pub user_sel: usize,
    /// First row shown in the online friends panel.
    pub friends_scroll: usize,
    /// The online friends panel is shown in this frame.
    friends_panel_shown: bool,
    /// Friends/bookmarks that came online or went offline: (name, online, when).
    pub toasts: Vec<(String, bool, Instant)>,
    pub completion: Option<Completion>,
    pub quit: bool,
    /// Use the compact layout on screens up to 80 columns (Windows 9x console).
    pub narrow_console: bool,
    /// The compact layout is in use for the current frame.
    compact: bool,
    pub reconnect_at: Option<Instant>,
    pub reconnect_attempts: u32,
    pub no_reconnect: bool,
    pub last_activity: Instant,
    pub auto_idle: Option<(Status, String)>,
    pub my_status: Status,
    pub my_status_msg: String,
    pub typing_sent: Option<(String, Typing, Instant)>,
    pub focused: bool,
    pub mapping: Option<Arc<Mapping>>,
    pub hit: Hit,
    pub outbox: VecDeque<(String, bool)>,
    pub last_chat_send: Instant,
    pub flash: Option<(String, Instant)>,
    pub search_fields: Option<Value>,
    pub joined_after_login: bool,
    pub kinks_dump: Vec<String>,
    pub info_dump: Vec<String>,
    pub character: String,
    pub dirty: bool,
    pub full_redraw: bool,
    pub opts_character: Option<String>,
}

const RECONNECT_DELAYS: [u64; 6] = [3, 5, 10, 30, 60, 120];

impl App {
    pub fn new(settings: Settings, accounts: Accounts, opts: Options) -> App {
        let (tx, rx) = mpsc::channel();
        let login = LoginScreen::new(&accounts, opts.account.as_deref(), opts.no_autologin);
        App {
            settings,
            accounts,
            login: Some(login),
            chat: Chat::new(),
            char_state: CharacterState::default(),
            api: None,
            login_info: None,
            conn: None,
            conn_id: 0,
            conn_state: ConnState::Offline,
            tx,
            rx,
            input: {
                let mut e = LineEditor::new();
                e.multiline = true;
                e
            },
            dialogs: Vec::new(),
            images: ImageStore::new(),
            focus: Focus::Input,
            user_sel: 0,
            friends_scroll: 0,
            friends_panel_shown: false,
            toasts: Vec::new(),
            completion: None,
            quit: false,
            narrow_console: cfg!(feature = "narrow-console"),
            compact: false,
            reconnect_at: None,
            reconnect_attempts: 0,
            no_reconnect: false,
            last_activity: Instant::now(),
            auto_idle: None,
            my_status: Status::Online,
            my_status_msg: String::new(),
            typing_sent: None,
            focused: true,
            mapping: None,
            hit: Hit::default(),
            outbox: VecDeque::new(),
            last_chat_send: Instant::now() - Duration::from_secs(5),
            flash: None,
            search_fields: None,
            joined_after_login: false,
            kinks_dump: Vec::new(),
            info_dump: Vec::new(),
            character: String::new(),
            dirty: true,
            full_redraw: false,
            opts_character: opts.character,
        }
    }

    // ------------------------------------------------------------------
    // Main loop

    pub fn run(&mut self) -> std::io::Result<()> {
        #[cfg(feature = "gui")]
        if gui_display_available() {
            return self.run_gui();
        }
        self.run_console()
    }

    /// Run in our own window instead of the terminal (`gui` feature).
    #[cfg(feature = "gui")]
    fn run_gui(&mut self) -> std::io::Result<()> {
        let settings = self.settings.clone();
        gui::run(&settings, |backend, mut events| {
            let mut terminal = Terminal::new(backend)?;
            self.start_autologin();
            let result = self.main_loop(&mut terminal, &mut |t| events.next(t));
            self.disconnect();
            // Open the window with the same size next time.
            if let Ok(size) = terminal.size() {
                if (size.width, size.height) != (self.settings.gui_columns, self.settings.gui_rows) {
                    self.settings.gui_columns = size.width;
                    self.settings.gui_rows = size.height;
                    let _ = self.settings.save();
                }
            }
            result
        })
    }

    fn start_autologin(&mut self) {
        if let Some(l) = &mut self.login {
            if l.wants_autologin() {
                let (a, p) = (l.account.text().to_string(), l.password.text().to_string());
                self.start_login(&a, &p);
            }
        }
    }

    fn run_console(&mut self) -> std::io::Result<()> {
        let mut stdout = std::io::stdout();
        crossterm::terminal::enable_raw_mode()?;
        // On the legacy Windows console crossterm's alternate screen is a second
        // console buffer, but our output still goes to the first, hidden one, so
        // the window stays black. Draw on the normal screen there instead.
        if !console16::is_legacy_console() {
            crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
        }
        // Optional extras: the legacy Windows console (Windows 9x, and Windows 7
        // without ANSI support) rejects some of them.
        let _ = crossterm::execute!(stdout, crossterm::event::EnableBracketedPaste);
        let _ = crossterm::execute!(stdout, crossterm::event::EnableFocusChange);
        if self.settings.mouse {
            let _ = crossterm::execute!(stdout, crossterm::event::EnableMouseCapture);
        }
        let backend = ConsoleBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        self.start_autologin();

        let mut next_event = |timeout: Duration| -> std::io::Result<Option<Event>> {
            if event::poll(timeout)? {
                event::read().map(Some)
            } else {
                Ok(None)
            }
        };
        let result = self.main_loop(&mut terminal, &mut next_event);

        self.disconnect();
        restore_terminal();
        result
    }

    /// `next_event` waits up to the given time for input; a `BrokenPipe`
    /// error from it means the window is gone.
    fn main_loop<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        next_event: &mut dyn FnMut(Duration) -> std::io::Result<Option<Event>>,
    ) -> std::io::Result<()> {
        let mut last_tick = Instant::now();
        while !self.quit {
            if self.dirty {
                self.images.begin_frame();
                terminal.draw(|f| self.draw(f))?;
                self.dirty = false;
                if self.images.end_frame() || self.full_redraw {
                    self.full_redraw = false;
                    if self.settings.image_mode == ImageMode::Native {
                        // Remove kitty image placements; other terminals ignore this.
                        let _ = write!(std::io::stdout(), "\x1b_Ga=d,d=A,q=2\x1b\\");
                    }
                    terminal.clear()?;
                    self.dirty = true;
                }
            }
            let mut wait = Duration::from_millis(30);
            loop {
                match next_event(wait) {
                    Ok(Some(ev)) => {
                        self.handle_terminal_event(ev);
                        self.dirty = true;
                        if self.quit {
                            break;
                        }
                        wait = Duration::ZERO;
                    }
                    Ok(None) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                        self.quit = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            let mut n = 0;
            while let Ok(ev) = self.rx.try_recv() {
                self.handle_app_event(ev);
                self.dirty = true;
                n += 1;
                if n > 2000 {
                    break;
                }
            }
            self.flush_outbox();
            if last_tick.elapsed() >= Duration::from_millis(500) {
                last_tick = Instant::now();
                self.tick();
            }
        }
        Ok(())
    }

    fn handle_terminal_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                self.activity();
                self.on_key(k);
            }
            Event::Paste(s) => {
                self.activity();
                if let Some(d) = self.dialogs.last_mut() {
                    d.paste(&s);
                } else if let Some(l) = &mut self.login {
                    l.paste(&s);
                } else {
                    self.input.insert_str(&s);
                    self.on_input_changed();
                }
            }
            Event::Mouse(m) => self.on_mouse(m),
            Event::FocusGained => self.focused = true,
            Event::FocusLost => self.focused = false,
            Event::Resize(_, _) => {
                self.images.clear_render_cache();
                self.full_redraw = true;
            }
            _ => {}
        }
    }

    fn handle_app_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Net(id, ev) => {
                if id == self.conn_id {
                    self.on_net(ev);
                }
            }
            AppEvent::Login(r) => self.on_login_result(r),
            AppEvent::Ticket(id, r) => {
                if id == self.conn_id {
                    self.on_ticket(r);
                }
            }
            AppEvent::Api(r) => self.on_api(r),
            AppEvent::Image(l) => {
                self.images.finish(l);
                self.full_redraw |= self.settings.image_mode == ImageMode::Native;
            }
        }
    }

    fn activity(&mut self) {
        self.last_activity = Instant::now();
        if let Some((st, msg)) = self.auto_idle.take() {
            if self.conn_state == ConnState::Online && self.my_status == Status::Idle {
                self.set_status(st, &msg, false);
            }
        }
    }

    /// Periodic housekeeping: reconnects, auto-idle, typing timeouts.
    fn tick(&mut self) {
        if let Some(at) = self.reconnect_at {
            if Instant::now() >= at {
                self.reconnect_at = None;
                self.reconnect();
            }
            self.dirty = true;
        }
        if self.conn_state == ConnState::Online && self.settings.idle_minutes > 0 && self.auto_idle.is_none() {
            let idle = Duration::from_secs(self.settings.idle_minutes as u64 * 60);
            if self.last_activity.elapsed() >= idle && matches!(self.my_status, Status::Online | Status::Looking) {
                self.auto_idle = Some((self.my_status, self.my_status_msg.clone()));
                let msg = self.my_status_msg.clone();
                self.set_status(Status::Idle, &msg, false);
            }
        }
        if let Some((who, Typing::Typing, at)) = self.typing_sent.clone() {
            if at.elapsed() > Duration::from_secs(5) {
                self.send(protocol::tpn(&who, "paused"));
                self.typing_sent = Some((who, Typing::Paused, Instant::now()));
            }
        }
        // Online/offline notifications disappear after a while.
        let before = self.toasts.len();
        self.toasts.retain(|(_, _, at)| at.elapsed() < TOAST_TIME);
        if self.toasts.len() != before {
            self.dirty = true;
        }
        if let Some((_, at)) = &self.flash {
            if at.elapsed() > Duration::from_secs(6) {
                self.flash = None;
            }
        }
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // Login & connection

    pub fn start_login(&mut self, account: &str, password: &str) {
        let api = Api::new(&self.settings.api_base, account, password);
        self.api = Some(api.clone());
        if let Some(l) = &mut self.login {
            l.busy = true;
            l.error.clear();
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = api.login();
            let _ = tx.send(AppEvent::Login(r));
        });
    }

    fn on_login_result(&mut self, r: Result<LoginInfo, String>) {
        let Some(l) = &mut self.login else { return };
        l.busy = false;
        match r {
            Err(e) => {
                l.error = e;
                self.api = None;
            }
            Ok(info) => {
                // Save the login.
                let account = l.account.text().trim().to_string();
                let prev = self.accounts.find(&account).cloned().unwrap_or_default();
                let saved = crate::config::SavedAccount {
                    account: account.clone(),
                    password: if l.save_password { crate::config::obfuscate(l.password.text()) } else { String::new() },
                    last_character: prev.last_character.clone(),
                    auto_login: l.auto_login && l.save_password,
                };
                self.accounts.upsert(saved);
                let _ = self.accounts.save();

                let preferred = self
                    .opts_character
                    .take()
                    .filter(|c| info.characters.iter().any(|x| x.eq_ignore_ascii_case(c)))
                    .or_else(|| Some(prev.last_character.clone()).filter(|c| !c.is_empty()))
                    .unwrap_or_else(|| info.default_character.clone());
                l.characters = info.characters.clone();
                l.char_sel = info.characters.iter().position(|c| c.eq_ignore_ascii_case(&preferred)).unwrap_or(0);
                l.stage = login::Stage::Character;
                let auto = l.auto_pick;
                self.login_info = Some(info);
                if auto {
                    if let Some(c) = l.characters.get(l.char_sel).cloned() {
                        self.choose_character(&c);
                    }
                }
            }
        }
    }

    pub fn choose_character(&mut self, character: &str) {
        let account = self.api.as_ref().map(|a| a.account.clone()).unwrap_or_default();
        if let Some(a) = self.accounts.accounts.iter_mut().find(|a| a.account.eq_ignore_ascii_case(&account)) {
            a.last_character = character.to_string();
        }
        let _ = self.accounts.save();

        self.character = character.to_string();
        self.chat = Chat::new();
        self.chat.me = character.to_string();
        self.char_state = CharacterState::load(character);
        self.my_status = Status::parse(if self.char_state.last_status.is_empty() { "online" } else { &self.char_state.last_status });
        if self.my_status == Status::Idle {
            self.my_status = Status::Online;
        }
        self.my_status_msg = self.char_state.last_status_message.clone();
        if let Some(info) = &self.login_info {
            for (mine, theirs) in &info.friends {
                if mine.eq_ignore_ascii_case(character) {
                    self.chat.friends.insert(theirs.to_lowercase());
                }
            }
            for b in &info.bookmarks {
                self.chat.bookmarks.insert(b.to_lowercase());
            }
        }
        self.login = None;
        self.sys(&format!("Logging in as {character}…"));

        // Recreate tabs from last session.
        if self.settings.rejoin_channels {
            for (id, title) in self.char_state.channels.clone() {
                let i = self.chat.ensure(ConvKind::Channel(id.clone()), &title);
                self.chat.conversations[i].pinned = self.char_state.pinned.contains(&id);
            }
        }
        if self.settings.reopen_private_chats {
            for name in self.char_state.private_chats.clone() {
                self.open_private(&name, false);
            }
        }
        for c in &mut self.chat.conversations {
            let key = c.key();
            c.muted = self.char_state.muted.contains(&key);
            c.show_ads = self.settings.show_ads && !self.char_state.ads_hidden.contains(&key);
        }
        self.chat.active = 0;
        self.connect();
    }

    pub fn connect(&mut self) {
        let Some(api) = self.api.clone() else {
            self.sys("Not logged in.");
            return;
        };
        self.disconnect();
        self.conn_id += 1;
        self.conn_state = ConnState::Connecting;
        self.joined_after_login = false;
        let id = self.conn_id;
        let tx = self.tx.clone();
        self.sys("Requesting login ticket…");
        std::thread::spawn(move || {
            let r = api.ticket();
            let _ = tx.send(AppEvent::Ticket(id, r));
        });
    }

    fn on_ticket(&mut self, r: Result<String, String>) {
        match r {
            Err(e) => {
                self.sys_err(&format!("Could not get a login ticket: {e}"));
                self.conn_state = ConnState::Offline;
                self.schedule_reconnect();
            }
            Ok(ticket) => {
                let account = self.api.as_ref().map(|a| a.account.clone()).unwrap_or_default();
                let idn = protocol::idn(&account, &ticket, &self.character);
                self.sys(&format!("Connecting to {}…", self.settings.server));
                self.conn = Some(net::connect(self.settings.server.clone(), idn, self.conn_id, self.tx.clone(), AppEvent::Net));
            }
        }
    }

    pub fn disconnect(&mut self) {
        if let Some(c) = self.conn.take() {
            c.close();
        }
        self.conn_id += 1;
        self.conn_state = ConnState::Offline;
        self.reconnect_at = None;
    }

    pub fn reconnect(&mut self) {
        self.no_reconnect = false;
        self.connect();
    }

    fn schedule_reconnect(&mut self) {
        if !self.settings.auto_reconnect || self.no_reconnect || self.character.is_empty() {
            return;
        }
        let delay = RECONNECT_DELAYS[(self.reconnect_attempts as usize).min(RECONNECT_DELAYS.len() - 1)];
        self.reconnect_attempts += 1;
        self.reconnect_at = Some(Instant::now() + Duration::from_secs(delay));
        self.sys(&format!("Reconnecting in {delay} seconds… (/reconnect to retry now)"));
    }

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connected => self.sys("Connected. Identifying…"),
            NetEvent::Command(cmd) => self.on_server(cmd),
            NetEvent::BadLine(line, err) => {
                net::log::debug(&format!("unparsable line ({err}): {line}"));
            }
            NetEvent::Disconnected(reason) => {
                self.conn = None;
                let was_online = self.conn_state == ConnState::Online;
                self.conn_state = ConnState::Offline;
                self.sys_err(&reason);
                for c in &mut self.chat.conversations {
                    if c.joined {
                        c.joined = false;
                        c.members.clear();
                    }
                }
                self.chat.characters.clear();
                if !self.no_reconnect {
                    self.schedule_reconnect();
                } else if was_online {
                    self.sys("Not reconnecting automatically. Use /reconnect to try again.");
                }
            }
        }
    }

    pub fn send(&mut self, line: String) {
        match &self.conn {
            Some(c) if self.conn_state == ConnState::Online || line.starts_with("IDN") => c.send(line),
            _ => self.flash("Not connected."),
        }
    }

    /// Queue a chat message (MSG/PRI/LRP) respecting the server's flood limit.
    pub fn send_chat(&mut self, line: String) {
        self.outbox.push_back((line, true));
        self.flush_outbox();
    }

    fn flush_outbox(&mut self) {
        let flood = Duration::from_millis(((self.chat.vars.msg_flood.max(0.5)) * 1000.0) as u64 + 50);
        while let Some((line, _)) = self.outbox.front() {
            if self.last_chat_send.elapsed() < flood {
                break;
            }
            let line = line.clone();
            self.outbox.pop_front();
            self.last_chat_send = Instant::now();
            self.send(line);
        }
    }

    // ------------------------------------------------------------------
    // Messages

    pub fn flash(&mut self, text: &str) {
        self.flash = Some((text.to_string(), Instant::now()));
    }

    pub fn sys(&mut self, text: &str) {
        let max = self.settings.max_messages;
        self.chat.console().push(Message::system(text), max);
    }

    pub fn sys_err(&mut self, text: &str) {
        let max = self.settings.max_messages;
        self.chat.console().push(Message::new(MsgKind::Error, "", text), max);
        if self.chat.active != 0 {
            let i = self.chat.active;
            self.chat.conversations[i].push(Message::new(MsgKind::Error, "", text), max);
        }
    }

    /// Show a system message in the current tab.
    pub fn info(&mut self, text: &str) {
        let max = self.settings.max_messages;
        self.chat.active_conv_mut().push(Message::system(text), max);
    }

    pub fn is_highlight(&self, text: &str) -> bool {
        let plain = crate::bbcode::strip(text).to_lowercase();
        if self.settings.highlight_own_name && !self.chat.me.is_empty() && plain.contains(&self.chat.me.to_lowercase()) {
            return true;
        }
        self.settings.highlight_words.iter().any(|w| !w.trim().is_empty() && plain.contains(&w.trim().to_lowercase()))
    }

    /// Add a message to a conversation, handling logs, unread counts and notifications.
    pub fn add_message(&mut self, idx: usize, mut msg: Message) {
        let max = self.settings.max_messages;
        let is_private = matches!(self.chat.conversations[idx].kind, ConvKind::Private(_));
        let from_me = msg.sender.eq_ignore_ascii_case(&self.chat.me);
        if matches!(msg.kind, MsgKind::Chat | MsgKind::Action | MsgKind::Ad) && !from_me && !is_private && self.is_highlight(&msg.text) {
            msg.highlight = true;
        }
        let conv = &self.chat.conversations[idx];
        let key = conv.key();
        let title = conv.title.clone();
        let muted = conv.muted;
        let loggable = match msg.kind {
            MsgKind::Ad => self.settings.log_ads,
            MsgKind::Event | MsgKind::History => false,
            _ => true,
        };
        if self.settings.log_messages && loggable && idx != 0 {
            let kind = if is_private { "private" } else { "channel" };
            logs::append(&self.chat.me, &key, &title, kind, &msg);
        }
        let viewing = idx == self.chat.active && self.dialogs.is_empty();
        let counts = matches!(msg.kind, MsgKind::Chat | MsgKind::Action | MsgKind::Ad | MsgKind::Roll | MsgKind::Warn | MsgKind::Broadcast);
        let notify = !from_me
            && !muted
            && ((is_private && counts && self.settings.notify_private) || (msg.highlight && self.settings.notify_highlight))
            && !(self.settings.notify_only_unfocused && self.focused && viewing);
        let preview = format!("{}{}", if msg.kind == MsgKind::Action { "* " } else { "" }, crate::bbcode::strip(&msg.text));
        let highlight = msg.highlight;
        let sender = msg.sender.clone();
        let conv = &mut self.chat.conversations[idx];
        if !viewing && counts && !from_me {
            conv.unread += 1;
            if highlight || is_private {
                conv.unread_highlight = true;
            }
        }
        conv.push(msg, max);
        if notify {
            let title = if is_private { format!("{sender} (private)") } else { format!("{sender} in {title}") };
            crate::notify::notify(&self.settings, &title, &preview);
        }
    }

    pub fn open_private(&mut self, name: &str, focus: bool) -> usize {
        let display = self.chat.character(name).map(|c| c.name.clone()).unwrap_or_else(|| name.to_string());
        let existed = self.chat.find_private(&display).is_some();
        let idx = self.chat.ensure(ConvKind::Private(display.clone()), &display);
        if !existed {
            let key = self.chat.conversations[idx].key();
            self.chat.conversations[idx].muted = self.char_state.muted.contains(&key);
            self.load_history(idx);
            self.remember_tabs();
        }
        if focus {
            self.switch_to(idx);
        }
        idx
    }

    /// Note a friend/bookmark coming online or going offline, if wanted.
    pub(super) fn watch_toast(&mut self, name: &str, online: bool) {
        if !self.settings.notify_online.wants(self.chat.is_friend(name), self.chat.is_bookmarked(name)) {
            return;
        }
        self.toasts.retain(|(n, _, _)| !n.eq_ignore_ascii_case(name));
        self.toasts.push((name.to_string(), online, Instant::now()));
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// Show the last lines of the log in a newly opened channel or private
    /// chat, before anything already in it.
    pub fn load_history(&mut self, idx: usize) {
        let n = self.settings.load_history_lines;
        let Some(conv) = self.chat.conversations.get(idx) else { return };
        if n == 0 || conv.history_loaded || self.chat.me.is_empty() || conv.kind == ConvKind::Console {
            return;
        }
        let hist = logs::tail(&self.chat.me, &conv.key(), n);
        let max = self.settings.max_messages;
        let conv = &mut self.chat.conversations[idx];
        conv.history_loaded = true;
        if hist.is_empty() {
            return;
        }
        let mut front = hist;
        front.push(Message::new(MsgKind::Event, "", "Earlier messages from the log are shown above."));
        for m in front.into_iter().rev() {
            conv.messages.push_front(m);
        }
        while conv.messages.len() > max.max(1) {
            conv.messages.pop_front();
        }
    }

    pub fn switch_to(&mut self, idx: usize) {
        if idx >= self.chat.conversations.len() {
            return;
        }
        // Save draft of the current tab.
        let draft = self.input.text().to_string();
        self.chat.active_conv_mut().draft = draft;
        self.clear_typing();
        self.chat.active = idx;
        let conv = &mut self.chat.conversations[idx];
        conv.unread = 0;
        conv.unread_highlight = false;
        let d = conv.draft.clone();
        self.input.set(&d);
        self.completion = None;
        self.user_sel = 0;
        if self.settings.image_mode == ImageMode::Native {
            self.full_redraw = true;
        }
    }

    pub fn close_conversation(&mut self, idx: usize) {
        if idx == 0 || idx >= self.chat.conversations.len() {
            return;
        }
        let conv = &self.chat.conversations[idx];
        if let ConvKind::Channel(id) = &conv.kind {
            if conv.joined {
                let id = id.clone();
                self.send(protocol::lch(&id));
            }
        }
        self.chat.remove(idx);
        let d = self.chat.active_conv().draft.clone();
        self.input.set(&d);
        self.remember_tabs();
    }

    /// Persist open channels/private chats for the next login.
    pub fn remember_tabs(&mut self) {
        if self.character.is_empty() {
            return;
        }
        self.char_state.channels =
            self.chat.conversations.iter().filter_map(|c| c.channel_id().map(|id| (id.to_string(), c.title.clone()))).collect();
        self.char_state.private_chats = self.chat.conversations.iter().filter_map(|c| c.partner().map(String::from)).collect();
        self.char_state.pinned = self.chat.conversations.iter().filter(|c| c.pinned).map(|c| c.key()).collect();
        let _ = self.char_state.save(&self.character);
    }

    pub fn set_status(&mut self, status: Status, msg: &str, remember: bool) {
        self.my_status = status;
        self.my_status_msg = msg.to_string();
        self.send(protocol::sta(status.as_str(), msg));
        if remember {
            self.auto_idle = None;
            self.char_state.last_status = status.as_str().to_string();
            self.char_state.last_status_message = msg.to_string();
            self.char_state.push_recent_status(msg);
            let _ = self.char_state.save(&self.character);
        }
    }

    pub fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.flash(&format!("Could not save settings: {e}"));
        }
    }

    // ------------------------------------------------------------------
    // Typing notifications

    fn on_input_changed(&mut self) {
        self.completion_refresh();
        if !self.settings.send_typing || self.conn_state != ConnState::Online {
            return;
        }
        let Some(partner) = self.chat.active_conv().partner().map(String::from) else { return };
        let text = self.input.text();
        let want = if text.is_empty() || text.starts_with('/') { Typing::Clear } else { Typing::Typing };
        let current = self.typing_sent.as_ref().filter(|(w, _, _)| w.eq_ignore_ascii_case(&partner)).map(|t| t.1).unwrap_or(Typing::Clear);
        if want == Typing::Typing {
            if current != Typing::Typing {
                self.send(protocol::tpn(&partner, "typing"));
            }
            self.typing_sent = Some((partner, Typing::Typing, Instant::now()));
        } else if current != Typing::Clear {
            self.send(protocol::tpn(&partner, "clear"));
            self.typing_sent = None;
        }
    }

    fn clear_typing(&mut self) {
        if let Some((who, st, _)) = self.typing_sent.take() {
            if st != Typing::Clear && self.conn_state == ConnState::Online {
                self.send(protocol::tpn(&who, "clear"));
            }
        }
    }

    // ------------------------------------------------------------------
    // API helpers

    pub fn api_call<F>(&mut self, f: F)
    where
        F: FnOnce(&Api) -> ApiReply + Send + 'static,
    {
        let Some(api) = self.api.clone() else {
            self.flash("Not logged in.");
            return;
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = f(&api);
            let _ = tx.send(AppEvent::Api(r));
        });
    }

    pub fn ensure_mapping(&mut self) {
        if self.mapping.is_none() {
            self.api_call(|api| ApiReply::Mapping(api.mapping_list()));
        }
    }

    pub fn request_image(&mut self, url: &str) {
        if self.settings.image_mode != ImageMode::Off {
            self.images.request(url, &self.tx, AppEvent::Image);
        }
    }

    // ------------------------------------------------------------------
    // Keyboard

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') && self.dialogs.is_empty() && self.login.is_none() && !self.input.is_empty() {
            self.input.clear();
            self.on_input_changed();
            return;
        }
        if ctrl && (key.code == KeyCode::Char('q') || (key.code == KeyCode::Char('c') && self.dialogs.is_empty())) {
            if self.settings.confirm_quit && self.login.is_none() {
                self.dialogs.push(Dialog::confirm("Quit F-Term?", actions::Action::Quit));
            } else {
                self.quit = true;
            }
            return;
        }
        if !self.dialogs.is_empty() {
            self.dialog_key(key);
            return;
        }
        if self.login.is_some() {
            self.login_key(key);
            return;
        }
        self.chat_key(key);
    }

    fn chat_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let n = self.chat.conversations.len();

        // Global shortcuts.
        match key.code {
            KeyCode::F(1) => return self.run_command("/help"),
            KeyCode::F(2) => return self.run_command("/channels"),
            KeyCode::F(3) => return self.run_command("/friends"),
            KeyCode::F(4) => {
                let target = self.chat.active_conv().partner().map(String::from);
                match target {
                    Some(t) => self.open_profile(&t),
                    None => self.prompt_character("View profile of", actions::PromptKind::Profile),
                }
                return;
            }
            KeyCode::F(5) => return self.open_status_dialog(),
            KeyCode::F(6) => return self.run_command("/settings"),
            KeyCode::F(7) => return self.run_command("/logs"),
            KeyCode::F(8) => return self.run_command("/search"),
            KeyCode::F(9) => {
                self.settings.show_user_list = !self.settings.show_user_list;
                self.save_settings();
                return;
            }
            KeyCode::F(10) => {
                self.settings.show_sidebar = !self.settings.show_sidebar;
                self.save_settings();
                return;
            }
            KeyCode::F(11) => return self.run_command("/links"),
            KeyCode::Char('n') if ctrl => return self.switch_to((self.chat.active + 1) % n),
            KeyCode::Char('p') if ctrl => return self.switch_to((self.chat.active + n - 1) % n),
            KeyCode::Down if alt => return self.switch_to((self.chat.active + 1) % n),
            KeyCode::Up if alt => return self.switch_to((self.chat.active + n - 1) % n),
            KeyCode::Right if alt => return self.switch_to((self.chat.active + 1) % n),
            KeyCode::Left if alt => return self.switch_to((self.chat.active + n - 1) % n),
            KeyCode::Char(c @ '0'..='9') if alt => {
                let i = if c == '0' { 9 } else { c as usize - '1' as usize };
                return self.switch_to(i.min(n - 1));
            }
            KeyCode::Char('a') if alt => {
                // Jump to the next tab with unread messages (highlights first).
                let order: Vec<usize> = (1..=n).map(|d| (self.chat.active + d) % n).collect();
                let target = order
                    .iter()
                    .find(|&&i| self.chat.conversations[i].unread_highlight)
                    .or_else(|| order.iter().find(|&&i| self.chat.conversations[i].unread > 0))
                    .copied();
                if let Some(t) = target {
                    self.switch_to(t);
                }
                return;
            }
            KeyCode::Char('j') if alt => return self.prompt("Join channel (name or ADH- id)", "", actions::PromptKind::Join, false),
            KeyCode::Char('p') if alt => return self.prompt("Open private chat with", "", actions::PromptKind::PrivateChat, false),
            KeyCode::Char('w') if alt => {
                let i = self.chat.active;
                return self.close_conversation(i);
            }
            KeyCode::Char('o') if ctrl => {
                self.focus = match self.focus {
                    Focus::Input if self.settings.show_user_list && self.chat.active_conv().channel_id().is_some() => Focus::Users,
                    Focus::Input | Focus::Users if self.settings.show_sidebar => Focus::Tabs,
                    _ => Focus::Input,
                };
                return;
            }
            KeyCode::Char('l') if ctrl => {
                self.full_redraw = true;
                self.images.clear_render_cache();
                return;
            }
            KeyCode::PageUp => {
                let h = self.hit.messages.height.max(2) as usize;
                let c = self.chat.active_conv_mut();
                c.scroll += h.saturating_sub(1);
                return;
            }
            KeyCode::PageDown => {
                let h = self.hit.messages.height.max(2) as usize;
                let c = self.chat.active_conv_mut();
                c.scroll = c.scroll.saturating_sub(h.saturating_sub(1));
                return;
            }
            KeyCode::Home if ctrl => {
                self.chat.active_conv_mut().scroll = usize::MAX / 2;
                return;
            }
            KeyCode::End if ctrl => {
                self.chat.active_conv_mut().scroll = 0;
                return;
            }
            KeyCode::Esc if self.focus != Focus::Input => {
                self.focus = Focus::Input;
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Users => return self.users_key(key),
            Focus::Tabs => return self.tabs_key(key),
            Focus::Input => {}
        }

        // Completion popup navigation.
        if self.completion.is_some() {
            match key.code {
                KeyCode::Tab => {
                    self.completion_step(1);
                    return;
                }
                KeyCode::BackTab => {
                    self.completion_step(-1);
                    return;
                }
                KeyCode::Down | KeyCode::Up if !self.input.text().contains('\n') => {
                    if let Some(c) = &mut self.completion {
                        let n = c.items.len();
                        c.sel = if key.code == KeyCode::Down { (c.sel + 1) % n } else { (c.sel + n - 1) % n };
                        c.cycling = false;
                    }
                    return;
                }
                KeyCode::Esc => {
                    self.completion = None;
                    return;
                }
                KeyCode::Enter
                    if !self.completion.as_ref().map(|c| c.cycling).unwrap_or(true)
                    // Accept the highlighted suggestion (or send if it is already typed).
                    && self.completion_accept() =>
                {
                    return;
                }
                _ => {}
            }
        } else if key.code == KeyCode::Tab {
            self.completion_start();
            return;
        }

        match self.input.handle(key) {
            EditResult::Submit => {
                let text = self.input.text().to_string();
                if text.trim().is_empty() {
                    return;
                }
                self.input.push_history(&text);
                self.input.clear();
                self.completion = None;
                self.clear_typing();
                self.submit(&text);
            }
            EditResult::Changed => {
                if let Some(c) = &mut self.completion {
                    c.cycling = false;
                }
                self.on_input_changed();
            }
            EditResult::Ignored => {
                if key.code == KeyCode::Esc {
                    self.chat.active_conv_mut().scroll = 0;
                }
                if key.code == KeyCode::Up {
                    self.chat.active_conv_mut().scroll += 1;
                }
                if key.code == KeyCode::Down {
                    let c = self.chat.active_conv_mut();
                    c.scroll = c.scroll.saturating_sub(1);
                }
            }
        }
    }

    fn users_key(&mut self, key: KeyEvent) {
        let members = self.chat.sorted_members(self.chat.active_conv());
        if members.is_empty() {
            self.focus = Focus::Input;
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.user_sel = self.user_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.user_sel = (self.user_sel + 1).min(members.len() - 1),
            KeyCode::PageUp => self.user_sel = self.user_sel.saturating_sub(10),
            KeyCode::PageDown => self.user_sel = (self.user_sel + 10).min(members.len() - 1),
            KeyCode::Home => self.user_sel = 0,
            KeyCode::End => self.user_sel = members.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(name) = members.get(self.user_sel.min(members.len() - 1)) {
                    let name = name.clone();
                    self.open_user_menu(&name);
                }
            }
            KeyCode::Tab => self.focus = Focus::Input,
            KeyCode::Char(c) if c.is_alphanumeric() => {
                // Jump to the first name starting with the typed letter.
                let lc = c.to_lowercase().to_string();
                let start = self.user_sel + 1;
                if let Some(i) =
                    (0..members.len()).map(|d| (start + d) % members.len()).find(|&i| members[i].to_lowercase().starts_with(&lc))
                {
                    self.user_sel = i;
                }
            }
            _ => {}
        }
    }

    fn tabs_key(&mut self, key: KeyEvent) {
        let n = self.chat.conversations.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.switch_to((self.chat.active + n - 1) % n),
            KeyCode::Down | KeyCode::Char('j') => self.switch_to((self.chat.active + 1) % n),
            KeyCode::Enter | KeyCode::Tab => self.focus = Focus::Input,
            KeyCode::Delete | KeyCode::Char('x') => {
                let i = self.chat.active;
                self.close_conversation(i);
            }
            KeyCode::Char('K') if self.chat.active > 1 => {
                // Move tab up.
                let i = self.chat.active;
                self.chat.conversations.swap(i, i - 1);
                self.chat.active = i - 1;
                self.remember_tabs();
            }
            KeyCode::Char('J') if self.chat.active > 0 && self.chat.active + 1 < n => {
                let i = self.chat.active;
                self.chat.conversations.swap(i, i + 1);
                self.chat.active = i + 1;
                self.remember_tabs();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Mouse

    fn on_mouse(&mut self, m: MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        if !self.settings.mouse {
            return;
        }
        let inside = |r: Rect| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        let key = |code: KeyCode| KeyEvent::new(code, KeyModifiers::NONE);
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        // Key hints (status bar, dialogs, login): clicking one presses its key.
        if left {
            if let Some(k) = widgets::key_at(m.column, m.row) {
                self.activity();
                self.on_key(k);
                return;
            }
        }
        if !self.dialogs.is_empty() {
            let list = self.hit.dialog_list;
            match m.kind {
                MouseEventKind::ScrollUp => self.dialog_key(key(KeyCode::Up)),
                MouseEventKind::ScrollDown => self.dialog_key(key(KeyCode::Down)),
                MouseEventKind::Down(MouseButton::Left) if inside(list) => {
                    let row = (m.row - list.y) as usize + self.hit.dialog_list_offset;
                    if let Some(d) = self.dialogs.last_mut() {
                        if d.select_row(row) {
                            self.dialog_key(key(KeyCode::Enter));
                        }
                    }
                }
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) if !inside(self.hit.dialog) => {
                    self.dialogs.pop();
                }
                _ => {}
            }
            return;
        }
        if let Some(l) = &mut self.login {
            let list = self.hit.login_list;
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(&(_, i)) = self.hit.login_fields.iter().find(|(r, _)| inside(*r)) {
                        l.focus = i;
                        // Checkboxes toggle, the button logs in.
                        if i >= 2 {
                            self.login_key(key(KeyCode::Enter));
                        }
                    } else if inside(list) {
                        let idx = (m.row - list.y) as usize + self.hit.login_list_offset;
                        if idx < l.filtered().len() {
                            // A click selects; clicking the selected one connects.
                            if l.char_sel == idx {
                                self.login_key(key(KeyCode::Enter));
                            } else {
                                l.char_sel = idx;
                            }
                        }
                    }
                }
                MouseEventKind::ScrollUp if inside(list) => self.login_key(key(KeyCode::Up)),
                MouseEventKind::ScrollDown if inside(list) => self.login_key(key(KeyCode::Down)),
                _ => {}
            }
            return;
        }
        let row_at = |rows: &[(u16, String)]| rows.iter().find(|(y, _)| *y == m.row).map(|(_, n)| n.clone());
        match m.kind {
            MouseEventKind::ScrollUp if inside(self.hit.messages) => self.chat.active_conv_mut().scroll += 3,
            MouseEventKind::ScrollDown if inside(self.hit.messages) => {
                let c = self.chat.active_conv_mut();
                c.scroll = c.scroll.saturating_sub(3);
            }
            MouseEventKind::ScrollUp if inside(self.hit.users) => self.user_sel = self.user_sel.saturating_sub(3),
            MouseEventKind::ScrollDown if inside(self.hit.users) => self.user_sel += 3,
            MouseEventKind::ScrollUp if inside(self.hit.friends) => self.friends_scroll = self.friends_scroll.saturating_sub(3),
            MouseEventKind::ScrollDown if inside(self.hit.friends) => self.friends_scroll += 3,
            // Scrolling over the tab list switches tabs.
            MouseEventKind::ScrollUp if inside(self.hit.tabs) => {
                let i = self.chat.active.saturating_sub(1);
                self.switch_to(i);
            }
            MouseEventKind::ScrollDown if inside(self.hit.tabs) => {
                let i = self.chat.active + 1;
                self.switch_to(i);
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.hit.tabs) => {
                if let Some((_, idx)) = self.hit.tab_rows.iter().find(|(y, _)| *y == m.row) {
                    let idx = *idx;
                    self.switch_to(idx);
                }
            }
            MouseEventKind::Down(MouseButton::Middle) if inside(self.hit.tabs) => {
                if let Some((_, idx)) = self.hit.tab_rows.iter().find(|(y, _)| *y == m.row) {
                    let idx = *idx;
                    self.close_conversation(idx);
                }
            }
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) if inside(self.hit.users) => {
                if let Some(name) = row_at(&self.hit.user_rows) {
                    let members = self.chat.sorted_members(self.chat.active_conv());
                    self.user_sel = members.iter().position(|n| *n == name).unwrap_or(0);
                    self.open_user_menu(&name);
                }
            }
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) if inside(self.hit.friends) => {
                if let Some(name) = row_at(&self.hit.friend_rows) {
                    self.open_user_menu(&name);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.hit.input) => self.focus = Focus::Input,
            _ => {}
        }
    }
}

/// Whether a window can be opened. On Linux without a display (e.g. over
/// SSH), the GUI build runs in the terminal instead.
#[cfg(feature = "gui")]
fn gui_display_available() -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"].iter().any(|v| std::env::var_os(v).is_some_and(|v| !v.is_empty()))
}

pub fn restore_terminal() {
    #[cfg(feature = "gui")]
    if gui::active() {
        return;
    }
    // One command at a time, so a command the console rejects doesn't skip the rest.
    let mut stdout = std::io::stdout();
    let _ = crossterm::execute!(stdout, crossterm::event::DisableMouseCapture);
    let _ = crossterm::execute!(stdout, crossterm::event::DisableFocusChange);
    let _ = crossterm::execute!(stdout, crossterm::event::DisableBracketedPaste);
    if console16::is_legacy_console() {
        // No alternate screen there: clear what we drew instead.
        let _ = crossterm::execute!(
            stdout,
            crossterm::style::ResetColor,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        );
    } else {
        let _ = crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);
    }
    let _ = crossterm::execute!(stdout, crossterm::cursor::Show);
    let _ = crossterm::terminal::disable_raw_mode();
}

#[cfg(test)]
mod tests;
