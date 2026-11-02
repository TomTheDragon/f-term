//! Slash command execution and user actions (menus, prompts).

use super::dialogs::{self, Dialog, MenuItem};
use super::{ApiReply, App, ConnState, Focus};
use crate::bbcode;
use crate::commands::{self, Context, Perm};
use crate::config::ImageMode;
use crate::logs;
use crate::protocol;
use crate::state::{ConvKind, Message, MsgKind, Status};

#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    Join(String),
    OpenPrivate(String),
    Profile(String),
    Memo(String),
    ToggleBookmark(String),
    ToggleIgnore(String),
    FriendRequest(String),
    Unfriend(String),
    Kick(String, String),
    Ban(String, String),
    Timeout(String, String),
    Op(String, String),
    Deop(String, String),
    Report(String),
    OpenUrl(String),
    ShowImage(String, String),
    PostAd(String, String),
    DeleteAd(usize),
    InsertText(String),
    FriendRequestAction(String, String),
    /// A friend (theirs, mine) in the friends list.
    FriendOf(String, String),
    /// Remove a friendship of one of your characters: (mine, theirs).
    UnfriendAs(String, String),
    Site(super::site::SiteAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Profile,
    PrivateChat,
    Join,
    Memo,
    Timeout,
    Report,
    Command,
    SaveSetting,
    Find,
    /// Search in the window below the prompt.
    DialogFind,
    Site(super::site::SitePrompt),
}

impl App {
    /// Text entered in the input line.
    pub(super) fn submit(&mut self, text: &str) {
        if let Some((name, _)) = commands::split(text) {
            if commands::find(name).is_some() {
                self.run_command(text);
                return;
            }
            self.info(&format!("Unknown command /{name}. Type /help for a list (use // to send a message starting with /)."));
            return;
        }
        let text = text.strip_prefix("//").map(|t| format!("/{t}")).unwrap_or_else(|| text.to_string());
        self.send_text(&text, MsgKind::Chat);
    }

    /// Send chat text to the current conversation.
    fn send_text(&mut self, text: &str, kind: MsgKind) {
        if self.conn_state != ConnState::Online {
            self.info("You are not connected. Use /reconnect.");
            return;
        }
        let conv = self.chat.active_conv();
        let wire = match kind {
            MsgKind::Action => format!("/me {text}").replace("/me '", "/me'"),
            MsgKind::Warn => format!("/warn {text}"),
            _ => text.to_string(),
        };
        match conv.kind.clone() {
            ConvKind::Console => self.info("This is the console. Join a channel (/join, F2) or open a private chat (/priv) to talk."),
            ConvKind::Channel(id) => {
                if !conv.joined {
                    self.info("You are not in this channel. Use /join to rejoin.");
                    return;
                }
                if kind == MsgKind::Ad {
                    if conv.mode == "chat" {
                        self.info("This channel does not allow ads.");
                        return;
                    }
                    if text.len() > self.chat.vars.lfrp_max {
                        self.info(&format!("Ad is too long ({} / {} bytes).", text.len(), self.chat.vars.lfrp_max));
                        return;
                    }
                    if let Some(left) = conv.ad_wait(self.chat.vars.lfrp_flood) {
                        self.info(&format!("You can post the next ad here in {}.", crate::timeutil::mmss(left)));
                        return;
                    }
                    self.send_chat(protocol::lrp(&id, text));
                    self.chat.active_conv_mut().last_ad = Some(std::time::Instant::now());
                } else {
                    if conv.mode == "ads" {
                        self.info("This channel only allows ads. Use /ad to post one.");
                        return;
                    }
                    if wire.len() > self.chat.vars.chat_max {
                        self.info(&format!("Message is too long ({} / {} bytes).", wire.len(), self.chat.vars.chat_max));
                        return;
                    }
                    self.send_chat(protocol::msg(&id, &wire));
                }
                let i = self.chat.active;
                let me = self.chat.me.clone();
                let shown = if kind == MsgKind::Action { strip_me(&wire) } else { text.to_string() };
                self.add_message(i, Message::new(kind, &me, &shown));
            }
            ConvKind::Private(name) => {
                if kind == MsgKind::Ad {
                    self.info("Ads can only be posted in channels.");
                    return;
                }
                if !self.chat.is_online(&name) {
                    self.info(&format!("{name} is offline; the message can't be delivered."));
                    return;
                }
                if wire.len() > self.chat.vars.priv_max {
                    self.info(&format!("Message is too long ({} / {} bytes).", wire.len(), self.chat.vars.priv_max));
                    return;
                }
                self.send_chat(protocol::pri(&name, &wire));
                let i = self.chat.active;
                let me = self.chat.me.clone();
                let shown = if kind == MsgKind::Action { strip_me(&wire) } else { text.to_string() };
                self.add_message(i, Message::new(if kind == MsgKind::Warn { MsgKind::Chat } else { kind }, &me, &shown));
            }
        }
    }

    fn can(&self, perm: Perm) -> bool {
        let me = &self.chat.me;
        let conv = self.chat.active_conv();
        let chatop = self.chat.is_chatop(me);
        match perm {
            Perm::User => true,
            Perm::ChanOp => conv.is_op(me) || conv.is_owner(me) || chatop,
            Perm::ChanOwner => conv.is_owner(me) || chatop,
            Perm::ChatOp => chatop,
            Perm::Admin => chatop,
        }
    }

    pub fn run_command(&mut self, text: &str) {
        let Some((name, rest)) = commands::split(text) else { return };
        let Some(def) = commands::find(name) else { return };
        let rest = rest.trim();
        let conv = self.chat.active_conv();
        let channel = conv.channel_id().map(String::from);
        let partner = conv.partner().map(String::from);
        match def.context {
            Context::InChannel if channel.is_none() => return self.info(&format!("/{} only works in a channel.", def.name)),
            Context::InChat if channel.is_none() && partner.is_none() => {
                return self.info(&format!("/{} only works in a channel or private chat.", def.name))
            }
            _ => {}
        }
        if !self.can(def.perm) {
            self.info(&format!("You don't have permission to use /{} here.", def.name));
            return;
        }
        let needs_conn = !matches!(
            def.name,
            "help"
                | "clear"
                | "find"
                | "logsearch"
                | "notes"
                | "note"
                | "messages"
                | "news"
                | "personals"
                | "eicons"
                | "testnotify"
                | "clearall"
                | "logs"
                | "settings"
                | "set"
                | "images"
                | "quit"
                | "reconnect"
                | "logout"
                | "links"
                | "open"
                | "savead"
                | "mute"
                | "pin"
                | "eicon"
                | "image"
                | "avatar"
                | "whoami"
        );
        if needs_conn && self.conn_state != ConnState::Online {
            self.info("You are not connected. Use /reconnect.");
            return;
        }
        let ch = channel.clone().unwrap_or_default();
        let target = |s: &str| -> String { s.trim().to_string() };
        let need = |app: &mut App, v: &str| -> bool {
            if v.trim().is_empty() {
                app.info(&format!("Usage: {}", def.usage));
                false
            } else {
                true
            }
        };

        match def.name {
            "help" => {
                if rest.is_empty() {
                    self.dialogs.push(Dialog::help());
                } else if let Some(c) = commands::find(rest.trim_start_matches('/')) {
                    let aliases = if c.aliases.is_empty() { String::new() } else { format!(" (aliases: /{})", c.aliases.join(", /")) };
                    self.info(&format!("{} — {}{}", c.usage, c.help, aliases));
                } else {
                    self.info(&format!("No command named {rest}."));
                }
            }
            "join" => {
                if !need(self, rest) {
                    return;
                }
                let id = self.resolve_channel(rest);
                self.join_channel(&id);
            }
            "close" => {
                let idx = if rest.is_empty() {
                    self.chat.active
                } else {
                    match self
                        .chat
                        .conversations
                        .iter()
                        .position(|c| c.title.eq_ignore_ascii_case(rest) || c.key().eq_ignore_ascii_case(rest))
                    {
                        Some(i) => i,
                        None => return self.info(&format!("No open tab named {rest}.")),
                    }
                };
                if idx == 0 {
                    self.info("The console can't be closed.");
                } else {
                    self.close_conversation(idx);
                }
            }
            "priv" => {
                if !need(self, rest) {
                    return;
                }
                let a = commands::args(rest, 2);
                let name = self.chat.character(&a[0]).map(|c| c.name.clone()).unwrap_or_else(|| a[0].clone());
                self.open_private(&name, true);
                if let Some(msg) = a.get(1).filter(|m| !m.is_empty()) {
                    let msg = msg.clone();
                    self.send_text(&msg, MsgKind::Chat);
                }
            }
            "me" => {
                if need(self, rest) {
                    self.send_text(rest, MsgKind::Action);
                }
            }
            "ad" => {
                if need(self, rest) {
                    self.send_text(rest, MsgKind::Ad);
                }
            }
            "warn" => {
                if need(self, rest) {
                    self.send_text(rest, MsgKind::Warn);
                }
            }
            "roll" | "bottle" => {
                let dice = if def.name == "bottle" {
                    "bottle".to_string()
                } else if rest.is_empty() {
                    "1d10".into()
                } else {
                    rest.to_string()
                };
                match (&channel, &partner) {
                    (Some(c), _) => self.send(protocol::rll(c, &dice)),
                    (None, Some(p)) => self.send(protocol::rll_private(p, &dice)),
                    _ => {}
                }
            }
            "status" => {
                let (st, msg) = match rest.split_once(char::is_whitespace) {
                    Some((s, m)) => (s, m.trim()),
                    None => (rest, ""),
                };
                if st.is_empty() {
                    return self.open_status_dialog();
                }
                let status = Status::parse(st);
                if status.as_str() != st.to_ascii_lowercase() || status == Status::Crown || status == Status::Offline {
                    return self.info("Status must be one of: online, looking, busy, dnd, away, idle.");
                }
                self.set_status(status, msg, true);
                self.info(&format!("Status set to {}.", status.label()));
            }
            "away" => self.set_status(Status::Away, rest, true),
            "back" => {
                let msg = self.my_status_msg.clone();
                self.set_status(Status::Online, &msg, true);
            }
            "clear" => {
                self.chat.active_conv_mut().clear_messages();
                self.full_redraw = true;
            }
            "clearall" => {
                for c in &mut self.chat.conversations {
                    c.clear_messages();
                }
                self.full_redraw = true;
            }
            "ignore" => {
                if need(self, rest) {
                    self.send(protocol::ign("add", Some(&target(rest))));
                }
            }
            "unignore" => {
                if need(self, rest) {
                    self.send(protocol::ign("delete", Some(&target(rest))));
                }
            }
            "ignorelist" => self.send(protocol::ign("list", None)),
            "profile" => {
                let who = if rest.is_empty() { partner.clone().unwrap_or_else(|| self.chat.me.clone()) } else { rest.to_string() };
                self.open_profile(&who);
            }
            "memo" => {
                let who = if rest.is_empty() { partner.clone().unwrap_or_default() } else { rest.to_string() };
                if need(self, &who) {
                    self.open_memo(&who);
                }
            }
            "kinks" => {
                if need(self, rest) {
                    self.send(protocol::char_only("KIN", rest));
                }
            }
            "info" => {
                if need(self, rest) {
                    self.send(protocol::char_only("PRO", rest));
                }
            }
            "code" => {
                let title = self.chat.active_conv().title.clone();
                let code =
                    if ch.starts_with("ADH-") { format!("[session={title}]{ch}[/session]") } else { format!("[channel]{ch}[/channel]") };
                self.info(&format!("Channel code: [noparse]{code}[/noparse]"));
            }
            "getdescription" => {
                let d = self.chat.active_conv().description.clone();
                self.dialogs.push(Dialog::text_plain("Channel description (BBCode)", &d));
            }
            "uptime" => self.send(protocol::upt()),
            "whoami" => {
                let account = self.api.as_ref().map(|a| a.account.clone()).unwrap_or_default();
                let me = self.chat.me.clone();
                let st = self.my_status.label();
                self.info(&format!("You are {me} (account {account}), status {st}."));
            }
            "version" => {
                let v = crate::protocol::CLIENT_VERSION;
                self.info(&format!("F-Term v{v} ({}, TLS: {})", std::env::consts::OS, crate::tls::backend_name()));
            }
            "users" => {
                let m = self.chat.sorted_members(self.chat.active_conv());
                self.info(&format!("{} members: {}", m.len(), m.join(", ")));
            }
            "channels" | "rooms" => {
                self.send(protocol::cha());
                self.send(protocol::ors());
                self.dialogs.push(Dialog::channels(if def.name == "rooms" { 1 } else { 0 }, rest));
            }
            "friends" => {
                self.dialogs.push(Dialog::Friends(dialogs::FriendsState::default()));
                self.api_call(|api| ApiReply::FriendLists(api.friend_lists()));
            }
            "search" => {
                self.dialogs.push(Dialog::Search(Box::default()));
                if let Some(f) = self.search_fields.clone() {
                    if let Some(Dialog::Search(s)) = self.dialogs.last_mut() {
                        s.load_fields(&f);
                    }
                } else {
                    self.api_call(|api| ApiReply::SearchFields(api.search_fields()));
                }
            }
            "recent" => {
                let items: Vec<MenuItem> = logs::recent(&self.chat.me, 30)
                    .into_iter()
                    .map(|(key, title, kind)| {
                        if kind == "private" {
                            MenuItem::new(&format!("@ {title}"), Action::OpenPrivate(title))
                        } else {
                            MenuItem::new(&format!("# {title}"), Action::Join(key))
                        }
                    })
                    .collect();
                if items.is_empty() {
                    self.info("No recent conversations yet.");
                } else {
                    self.dialogs.push(Dialog::menu("Recent conversations", items));
                }
            }
            "notes" | "note" | "messages" | "news" | "personals" if self.api.is_none() => {
                self.info("Log in first to use the F-List website features.");
            }
            "notes" => {
                let (word, arg) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
                let list = match word.to_lowercase().as_str() {
                    "" | "inbox" => crate::site::NoteList::Folder(1),
                    "outbox" | "sent" => crate::site::NoteList::Folder(2),
                    "trash" => crate::site::NoteList::Folder(3),
                    "unread" => crate::site::NoteList::Unread,
                    "starred" => crate::site::NoteList::Starred,
                    "with" if !arg.is_empty() => crate::site::NoteList::With(arg.to_string()),
                    "search" if !arg.is_empty() => crate::site::NoteList::Containing(arg.to_string()),
                    _ => return self.info(&format!("Usage: {}", def.usage)),
                };
                self.open_notes(list);
            }
            "note" => self.compose_note(rest),
            "eicons" => self.open_eicon_picker(rest),
            "testnotify" => {
                crate::notify::notify(&self.settings, "F-Term", "This is a test notification.");
                self.info("Sent a test notification (bell, desktop notification, sound and notify_command, as set).");
            }
            "messages" => self.open_messages(),
            "news" => {
                let page = rest.parse::<usize>().unwrap_or(1).max(1);
                self.open_news(page - 1);
            }
            "personals" => self.open_personals(rest),
            "browse" => {
                let newest = matches!(rest.trim().to_lowercase().as_str(), "new" | "newest" | "created" | "recent");
                self.open_browse(newest);
            }
            "groups" => self.open_groups(rest),
            "logsearch" => {
                let me = self.chat.me.clone();
                if me.is_empty() {
                    return self.info("Log in first to search your logs.");
                }
                if !need(self, rest) {
                    return;
                }
                let mut d = dialogs::LogsState::new(&me);
                d.search(&me, rest);
                self.dialogs.push(Dialog::Logs(Box::new(d)));
            }
            "find" => {
                if rest.is_empty() {
                    self.prompt("Find in this tab", "", PromptKind::Find, false);
                } else {
                    self.find_start(rest);
                }
            }
            "logs" => {
                let me = self.chat.me.clone();
                if me.is_empty() {
                    return self.info("Log in first to see your logs.");
                }
                let mut d = dialogs::LogsState::new(&me);
                let wanted = if rest.is_empty() { self.chat.active_conv().key() } else { rest.to_lowercase() };
                if let Some(i) = d.convs.iter().position(|(k, t)| k.eq_ignore_ascii_case(&wanted) || t.eq_ignore_ascii_case(&wanted)) {
                    d.open_conv(&me, i);
                }
                self.dialogs.push(Dialog::Logs(Box::new(d)));
            }
            "settings" => self.dialogs.push(Dialog::Settings(dialogs::SettingsState::default())),
            "set" => {
                let (key, value) = match rest.split_once(char::is_whitespace) {
                    Some((k, v)) => (k, v.trim()),
                    None => (rest, ""),
                };
                if key.is_empty() {
                    return self.dialogs.push(Dialog::Settings(dialogs::SettingsState::default()));
                }
                if value.is_empty() && !self.settings.is_bool(key) {
                    match self.settings.get(key) {
                        Some(v) => self.info(&format!("{key} = {v}")),
                        None => self.info(&format!("Unknown setting {key}.")),
                    }
                    return;
                }
                match self.settings.set(key, value) {
                    Ok(()) => {
                        let v = self.settings.get(key).unwrap_or_default();
                        self.save_settings();
                        self.settings_changed();
                        self.info(&format!("{key} = {v}"));
                    }
                    Err(e) => self.info(&e),
                }
            }
            "images" => match ImageMode::parse(rest) {
                Some(m) => {
                    self.settings.set_active_image_mode(m);
                    self.save_settings();
                    self.settings_changed();
                    self.info(&format!("Images: {}", m.name()));
                }
                None => self.info("Usage: /images <off|ascii|colorascii|blocks|braille|native>"),
            },
            "avatar" => {
                let who = if rest.is_empty() { partner.clone().unwrap_or_else(|| self.chat.me.clone()) } else { rest.to_string() };
                let url = crate::images::avatar_url(&who);
                self.show_image(&format!("Avatar of {who}"), &url);
            }
            "eicon" => {
                if need(self, rest) {
                    let url = crate::images::eicon_url(rest);
                    self.show_image(&format!("Eicon {rest}"), &url);
                }
            }
            "image" => {
                if need(self, rest) {
                    self.show_image(rest, rest);
                }
            }
            "links" => self.open_links(),
            "open" => {
                if need(self, rest) {
                    if let Err(e) = crate::notify::open_url(rest) {
                        self.info(&e);
                    }
                }
            }
            "bookmark" => {
                if need(self, rest) {
                    let n = target(rest);
                    self.api_call(move |api| ApiReply::Done(format!("Bookmarked {n}."), api.bookmark_add(&n)));
                }
            }
            "unbookmark" => {
                if need(self, rest) {
                    let n = target(rest);
                    self.api_call(move |api| ApiReply::Done(format!("Removed bookmark {n}."), api.bookmark_remove(&n)));
                }
            }
            "friend" => {
                if need(self, rest) {
                    let n = target(rest);
                    let me = self.chat.me.clone();
                    self.api_call(move |api| ApiReply::Done(format!("Friend request sent to {n}."), api.friend_request(&me, &n)));
                }
            }
            "unfriend" => {
                if need(self, rest) {
                    let n = target(rest);
                    let me = self.chat.me.clone();
                    self.api_call(move |api| ApiReply::Done(format!("Removed {n} from your friends."), api.friend_remove(&me, &n)));
                }
            }
            "report" => {
                let a = commands::args(rest, 2);
                if a[0].is_empty() {
                    return self.prompt("Report: character, reason", "", PromptKind::Report, false);
                }
                if a.get(1).map(|r| r.is_empty()).unwrap_or(true) {
                    return self.prompt(
                        &format!("Report {}: describe the problem", a[0]),
                        &format!("{}, ", a[0]),
                        PromptKind::Report,
                        false,
                    );
                }
                self.submit_report(&a[0], &a[1]);
            }
            "showads" => {
                let c = self.chat.active_conv_mut();
                c.show_ads = !c.show_ads;
                let (show, key) = (c.show_ads, c.key());
                self.char_state.ads_hidden.retain(|k| *k != key);
                if !show {
                    self.char_state.ads_hidden.push(key);
                }
                let _ = self.char_state.save(&self.character);
                self.info(if show { "Ads are now shown in this channel." } else { "Ads are now hidden in this channel." });
            }
            "mute" => {
                let c = self.chat.active_conv_mut();
                c.muted = !c.muted;
                let (muted, key) = (c.muted, c.key());
                self.char_state.muted.retain(|k| *k != key);
                if muted {
                    self.char_state.muted.push(key);
                }
                let _ = self.char_state.save(&self.character);
                self.info(if muted { "Notifications muted for this tab." } else { "Notifications enabled for this tab." });
            }
            "pin" => {
                let c = self.chat.active_conv_mut();
                c.pinned = !c.pinned;
                let p = c.pinned;
                self.remember_tabs();
                self.info(if p { "Tab pinned." } else { "Tab unpinned." });
            }
            "savead" => {
                if need(self, rest) {
                    self.char_state.saved_ads.push(rest.to_string());
                    let _ = self.char_state.save(&self.character);
                    self.info("Ad saved. Use /ads to post it.");
                }
            }
            "ads" => {
                if self.char_state.saved_ads.is_empty() {
                    return self.info("No saved ads. Save one with /savead <text>.");
                }
                let items: Vec<MenuItem> = self
                    .char_state
                    .saved_ads
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let mut item =
                            MenuItem::new(&bbcode::strip(a).chars().take(70).collect::<String>(), Action::PostAd(ch.clone(), a.clone()));
                        item.alt = Some(Action::DeleteAd(i));
                        item
                    })
                    .collect();
                self.dialogs.push(Dialog::menu("Post a saved ad (Del removes)", items));
            }
            "makeroom" => {
                if need(self, rest) {
                    self.send(protocol::chan_only("CCR", rest));
                }
            }
            "reconnect" => {
                if self.api.is_none() {
                    return self.info("Not logged in.");
                }
                self.reconnect_attempts = 0;
                self.reconnect();
            }
            "logout" => {
                self.disconnect();
                self.remember_tabs();
                self.character.clear();
                self.chat = crate::state::Chat::new();
                let mut l = super::login::LoginScreen::new(&self.accounts, self.api.as_ref().map(|a| a.account.as_str()), true);
                l.auto_pick = false;
                self.login = Some(l);
                self.api = None;
                self.site_counters = Default::default();
                self.my_kinks = None;
                self.my_kinks_requested = false;
                self.site_poll_at = None;
                self.site_error_shown = false;
            }
            "quit" => self.quit = true,
            "raw" => {
                if need(self, rest) {
                    self.send(rest.to_string());
                }
            }
            "kick" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("CKU", &ch, &target(rest)));
                }
            }
            "ban" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("CBU", &ch, &target(rest)));
                }
            }
            "unban" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("CUB", &ch, &target(rest)));
                }
            }
            "banlist" => self.send(protocol::chan_only("CBL", &ch)),
            "timeout" => {
                let a = commands::args(rest, 2);
                let mins = a.get(1).and_then(|m| m.parse::<i64>().ok());
                match mins {
                    Some(m) if !a[0].is_empty() && (1..=90).contains(&m) => self.send(protocol::ctu(&ch, &a[0], m)),
                    _ => self.info(&format!("Usage: {}", def.usage)),
                }
            }
            "op" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("COA", &ch, &target(rest)));
                }
            }
            "deop" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("COR", &ch, &target(rest)));
                }
            }
            "oplist" => {
                let c = self.chat.active_conv();
                let owner = if c.owner.is_empty() { "none".to_string() } else { c.owner.clone() };
                let ops = c.ops.join(", ");
                self.info(&format!("Owner: {owner}. Operators: {ops}"));
                self.send(protocol::chan_only("COL", &ch));
            }
            "invite" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("CIU", &ch, &target(rest)));
                }
            }
            "openroom" => self.send(protocol::rst(&ch, "public")),
            "closeroom" => self.send(protocol::rst(&ch, "private")),
            "setmode" => match rest {
                "chat" | "ads" | "both" => self.send(protocol::rmo(&ch, rest)),
                _ => self.info("Usage: /setmode <chat|ads|both>"),
            },
            "setowner" => {
                if need(self, rest) {
                    self.send(protocol::chan_char("CSO", &ch, &target(rest)));
                }
            }
            "setdescription" => {
                if need(self, rest) {
                    self.send(protocol::cds(&ch, rest));
                }
            }
            "gkick" => {
                if need(self, rest) {
                    self.send(protocol::char_only("KIK", &target(rest)));
                }
            }
            "gban" => {
                if need(self, rest) {
                    self.send(protocol::char_only("ACB", &target(rest)));
                }
            }
            "gunban" => {
                if need(self, rest) {
                    self.send(protocol::char_only("UNB", &target(rest)));
                }
            }
            "gtimeout" => {
                let a = commands::args(rest, 3);
                let mins = a.get(1).and_then(|m| m.parse::<i64>().ok());
                match (mins, a.get(2)) {
                    (Some(m), Some(reason)) if !a[0].is_empty() && (1..=90).contains(&m) => self.send(protocol::tmo(&a[0], m, reason)),
                    _ => self.info(&format!("Usage: {}", def.usage)),
                }
            }
            "createchannel" => {
                if need(self, rest) {
                    self.send(protocol::chan_only("CRC", rest));
                }
            }
            "killchannel" => {
                let c = if rest.is_empty() { ch.clone() } else { self.resolve_channel(rest) };
                if need(self, &c) {
                    self.send(protocol::chan_only("KIC", &c));
                }
            }
            "reward" => {
                if need(self, rest) {
                    self.send(protocol::char_only("RWD", &target(rest)));
                }
            }
            "broadcast" => {
                if need(self, rest) {
                    self.send(protocol::bro(rest));
                }
            }
            "gop" => {
                if need(self, rest) {
                    self.send(protocol::char_only("AOP", &target(rest)));
                }
            }
            "gdeop" => {
                if need(self, rest) {
                    self.send(protocol::char_only("DOP", &target(rest)));
                }
            }
            "reloadconfig" => self.send(protocol::rld(if rest.is_empty() { "" } else { rest })),
            "altwatch" => {
                if need(self, rest) {
                    self.send(protocol::char_only("AWC", &target(rest)));
                }
            }
            "handlereport" => {
                if need(self, rest) {
                    self.send(protocol::sfc_confirm(rest));
                }
            }
            other => self.info(&format!("/{other} is not implemented.")),
        }
    }

    /// Map a typed channel name/title to a channel id.
    fn resolve_channel(&self, s: &str) -> String {
        let s = s.trim().trim_start_matches('#');
        if s.starts_with("ADH-") {
            return s.to_string();
        }
        if let Some(c) = self.chat.conversations.iter().find(|c| c.channel_id().is_some() && c.title.eq_ignore_ascii_case(s)) {
            return c.key();
        }
        self.chat
            .public_channels
            .iter()
            .chain(self.chat.private_rooms.iter())
            .find(|c| c.title.eq_ignore_ascii_case(s) || c.name.eq_ignore_ascii_case(s))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| s.to_string())
    }

    pub fn join_channel(&mut self, id: &str) {
        if let Some(i) = self.chat.find_channel(id) {
            if self.chat.conversations[i].joined {
                self.switch_to(i);
                return;
            }
        }
        self.send(protocol::jch(id));
    }

    pub fn open_status_dialog(&mut self) {
        let recent = self.char_state.recent_statuses.clone();
        self.dialogs.push(Dialog::Status(dialogs::StatusState::new(self.my_status, &self.my_status_msg, recent)));
    }

    pub fn open_profile(&mut self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.ensure_mapping();
        self.request_image(&crate::images::avatar_url(&name));
        self.dialogs.push(Dialog::Profile(Box::new(super::profile::ProfileState::new(&name))));
        let n = name.clone();
        self.api_call(move |api| ApiReply::Profile(n.clone(), api.character_data(&n)));
        // Your own kinks, for the kink match (once per login).
        let me = self.chat.me.clone();
        if !self.my_kinks_requested && !me.is_empty() && !me.eq_ignore_ascii_case(&name) {
            self.my_kinks_requested = true;
            self.api_call(move |api| ApiReply::Profile(me.clone(), api.character_data(&me)));
        }
    }

    pub fn open_memo(&mut self, name: &str) {
        let n = name.to_string();
        self.flash(&format!("Loading memo for {name}…"));
        self.api_call(move |api| ApiReply::Memo(n.clone(), api.memo_get(&n)));
    }

    pub fn show_image(&mut self, title: &str, url: &str) {
        if self.settings.active_image_mode() == ImageMode::Off {
            self.info("Images are turned off. Use /images blocks (or ascii/native) to enable them.");
            return;
        }
        self.request_image(url);
        self.dialogs.push(Dialog::Image { title: title.to_string(), url: url.to_string() });
    }

    fn open_links(&mut self) {
        let conv = self.chat.active_conv();
        let mut items: Vec<MenuItem> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for m in conv.messages.iter().rev().take(300) {
            for l in bbcode::links(&m.text) {
                let (label, action) = match &l {
                    bbcode::Link::Url(u) => (u.clone(), Action::OpenUrl(u.clone())),
                    bbcode::Link::User(u) => (format!("@{u}"), Action::Profile(u.clone())),
                    bbcode::Link::Session { title, id } => (format!("#{title} ({id})"), Action::Join(id.clone())),
                    bbcode::Link::Channel(c) => (format!("#{c}"), Action::Join(c.clone())),
                };
                if seen.insert(label.clone()) {
                    items.push(MenuItem::new(&label, action));
                }
            }
            for e in bbcode::embeds(&m.text) {
                let (label, action) = match e {
                    bbcode::Embed::Eicon(n) => {
                        (format!(":{n}: (eicon)"), Action::ShowImage(format!("Eicon {n}"), crate::images::eicon_url(&n)))
                    }
                    bbcode::Embed::Icon(n) => {
                        (format!("[{n}] (avatar)"), Action::ShowImage(format!("Avatar of {n}"), crate::images::avatar_url(&n)))
                    }
                };
                if seen.insert(label.clone()) {
                    items.push(MenuItem::new(&label, action));
                }
            }
        }
        if items.is_empty() {
            self.info("No links in this tab.");
        } else {
            self.dialogs.push(Dialog::menu("Links (newest first)", items));
        }
    }

    /// Action menu for a character.
    pub fn open_user_menu(&mut self, name: &str) {
        let n = name.to_string();
        let mut items = vec![
            MenuItem::new("Private message", Action::OpenPrivate(n.clone())),
            MenuItem::new("View profile", Action::Profile(n.clone())),
            MenuItem::new("Send a note…", Action::Site(super::site::SiteAction::ComposeNote(n.clone()))),
            MenuItem::new("Show avatar", Action::ShowImage(format!("Avatar of {n}"), crate::images::avatar_url(&n))),
            MenuItem::new("Edit memo", Action::Memo(n.clone())),
            MenuItem::new(if self.chat.is_bookmarked(&n) { "Remove bookmark" } else { "Bookmark" }, Action::ToggleBookmark(n.clone())),
            MenuItem::new(
                if self.chat.is_friend(&n) { "Remove friend" } else { "Send friend request" },
                if self.chat.is_friend(&n) { Action::Unfriend(n.clone()) } else { Action::FriendRequest(n.clone()) },
            ),
            MenuItem::new(if self.chat.is_ignored(&n) { "Unignore" } else { "Ignore" }, Action::ToggleIgnore(n.clone())),
            MenuItem::new("Insert name into message", Action::InsertText(n.clone())),
            MenuItem::new("Report", Action::Report(n.clone())),
        ];
        if let Some(ch) = self.chat.active_conv().channel_id().map(String::from) {
            if self.can(Perm::ChanOp) {
                items.push(MenuItem::new("Kick from channel", Action::Kick(ch.clone(), n.clone())));
                items.push(MenuItem::new("Ban from channel", Action::Ban(ch.clone(), n.clone())));
                items.push(MenuItem::new("Timeout from channel…", Action::Timeout(ch.clone(), n.clone())));
            }
            if self.can(Perm::ChanOwner) {
                if self.chat.active_conv().is_op(&n) {
                    items.push(MenuItem::new("Remove channel operator", Action::Deop(ch.clone(), n.clone())));
                } else {
                    items.push(MenuItem::new("Make channel operator", Action::Op(ch, n.clone())));
                }
            }
        }
        let header = match self.chat.character(&n) {
            Some(c) if !c.status_msg.is_empty() => format!("{} — {} — {}", n, c.status.label(), bbcode::strip(&c.status_msg)),
            Some(c) => format!("{} — {}", n, c.status.label()),
            None => format!("{n} — offline"),
        };
        self.dialogs.push(Dialog::menu(&header, items));
    }

    /// The user menu of a friend of one of your characters: removing the
    /// friend ends that character's friendship.
    pub fn open_friend_menu(&mut self, theirs: &str, mine: &str) {
        self.open_user_menu(theirs);
        if let Some(Dialog::Menu { items, .. }) = self.dialogs.last_mut() {
            for item in items.iter_mut() {
                if matches!(item.action, Action::Unfriend(_) | Action::FriendRequest(_)) {
                    *item = MenuItem::new(&format!("Remove friend (of {mine})"), Action::UnfriendAs(mine.to_string(), theirs.to_string()));
                }
            }
        }
    }

    pub fn prompt(&mut self, title: &str, initial: &str, kind: PromptKind, multiline: bool) {
        self.dialogs.push(Dialog::prompt(title, initial, kind, multiline));
    }

    pub fn prompt_character(&mut self, title: &str, kind: PromptKind) {
        self.prompt(title, "", kind, false);
    }

    pub fn on_prompt(&mut self, kind: PromptKind, value: String, extra: String) {
        let v = value.trim().to_string();
        match kind {
            PromptKind::Profile => self.open_profile(&v),
            PromptKind::PrivateChat => {
                if !v.is_empty() {
                    self.open_private(&v, true);
                }
            }
            PromptKind::Join => {
                if !v.is_empty() {
                    let id = self.resolve_channel(&v);
                    self.join_channel(&id);
                }
            }
            PromptKind::Memo => {
                let name = extra;
                let note = value.clone();
                self.api_call(move |api| ApiReply::Done(format!("Memo for {name} saved."), api.memo_save(&name, &note)));
            }
            PromptKind::Timeout => {
                let (ch, name) = extra.split_once('\n').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
                match v.parse::<i64>() {
                    Ok(m) if (1..=90).contains(&m) => self.send(protocol::ctu(&ch, &name, m)),
                    _ => self.info("Timeout must be 1-90 minutes."),
                }
            }
            PromptKind::Report => {
                let a = commands::args(&v, 2);
                if a.len() < 2 || a[0].is_empty() || a[1].is_empty() {
                    self.info("Report needs a character name and a reason, separated by a comma.");
                } else {
                    self.submit_report(&a[0], &a[1]);
                }
            }
            PromptKind::Find => self.find_start(&v),
            PromptKind::DialogFind => self.dialog_find_start(&v),
            PromptKind::Site(k) => self.on_site_prompt(k, value, extra),
            PromptKind::Command => {
                if !v.is_empty() {
                    self.submit(&v);
                }
            }
            PromptKind::SaveSetting => {
                let key = extra;
                match self.settings.set(&key, &value) {
                    Ok(()) => {
                        self.save_settings();
                        self.settings_changed();
                    }
                    Err(e) => self.flash(&e),
                }
            }
        }
    }

    fn submit_report(&mut self, character: &str, reason: &str) {
        let conv = self.chat.active_conv();
        let channel = match &conv.kind {
            ConvKind::Channel(id) => id.clone(),
            ConvKind::Private(n) => format!("Private chat with {n}"),
            ConvKind::Console => "Console".into(),
        };
        let msgs: Vec<Message> = conv.messages.iter().rev().take(200).rev().cloned().collect();
        let header = format!("Report by {} in {} at {}\n\n", self.chat.me, channel, crate::timeutil::fmt_datetime(crate::timeutil::now()));
        let log = format!("{header}{}", logs::report_log(&msgs));
        let me = self.chat.me.clone();
        let (c, r) = (character.to_string(), reason.to_string());
        self.info("Uploading report…");
        self.api_call(move |api| {
            let result = api.report_submit(&me, &channel, &c, &r, &log);
            ApiReply::Report { character: c, reason: r, result }
        });
    }

    pub fn run_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Site(a) => self.run_site_action(a),
            Action::Join(id) => self.join_channel(&id),
            Action::OpenPrivate(n) => {
                self.dialogs.clear();
                self.open_private(&n, true);
                self.focus = Focus::Input;
            }
            Action::Profile(n) => self.open_profile(&n),
            Action::Memo(n) => self.open_memo(&n),
            Action::ToggleBookmark(n) => {
                let cmd = if self.chat.is_bookmarked(&n) { "unbookmark" } else { "bookmark" };
                self.run_command(&format!("/{cmd} {n}"));
            }
            Action::ToggleIgnore(n) => {
                let cmd = if self.chat.is_ignored(&n) { "unignore" } else { "ignore" };
                self.run_command(&format!("/{cmd} {n}"));
            }
            Action::FriendRequest(n) => self.run_command(&format!("/friend {n}")),
            Action::Unfriend(n) => self.run_command(&format!("/unfriend {n}")),
            Action::Kick(ch, n) => self.send(protocol::chan_char("CKU", &ch, &n)),
            Action::Ban(ch, n) => self.send(protocol::chan_char("CBU", &ch, &n)),
            Action::Op(ch, n) => self.send(protocol::chan_char("COA", &ch, &n)),
            Action::Deop(ch, n) => self.send(protocol::chan_char("COR", &ch, &n)),
            Action::Timeout(ch, n) => {
                let mut d = Dialog::prompt(&format!("Timeout {n} for how many minutes (1-90)?"), "10", PromptKind::Timeout, false);
                if let Dialog::Prompt(p) = &mut d {
                    p.extra = format!("{ch}\n{n}");
                }
                self.dialogs.push(d);
            }
            Action::Report(n) => self.prompt(&format!("Report {n}: describe the problem"), &format!("{n}, "), PromptKind::Report, false),
            Action::OpenUrl(u) => {
                if let Err(e) = crate::notify::open_url(&u) {
                    self.flash(&e);
                }
            }
            Action::ShowImage(t, u) => self.show_image(&t, &u),
            Action::PostAd(ch, text) => {
                let ch = if ch.is_empty() { self.chat.active_conv().channel_id().unwrap_or("").to_string() } else { ch };
                if let Some(i) = self.chat.find_channel(&ch) {
                    self.switch_to(i);
                    self.send_text(&text, MsgKind::Ad);
                }
            }
            Action::DeleteAd(i) => {
                if i < self.char_state.saved_ads.len() {
                    self.char_state.saved_ads.remove(i);
                    let _ = self.char_state.save(&self.character);
                }
            }
            Action::InsertText(t) => {
                self.dialogs.clear();
                self.input.insert_str(&t);
                self.focus = Focus::Input;
            }
            Action::FriendOf(theirs, mine) => self.open_friend_menu(&theirs, &mine),
            Action::UnfriendAs(mine, theirs) => {
                let (Some(api), tx) = (self.api.clone(), self.tx.clone()) else { return self.flash("Not logged in.") };
                std::thread::spawn(move || {
                    let r = api.friend_remove(&mine, &theirs);
                    let ok = r.is_ok();
                    let _ = tx.send(super::AppEvent::Api(ApiReply::Done(format!("{mine} and {theirs} are no longer friends."), r)));
                    if ok {
                        let _ = tx.send(super::AppEvent::Api(ApiReply::FriendLists(api.friend_lists())));
                    }
                });
            }
            Action::FriendRequestAction(action, id) => {
                let (a, i) = (action.clone(), id.clone());
                self.api_call(move |api| ApiReply::Done(format!("Friend request: {a} done."), api.request_action(&a, &i)));
            }
        }
    }

    /// Apply side effects of changed settings.
    pub fn settings_changed(&mut self) {
        self.images.clear_render_cache();
        self.full_redraw = true;
        let mut out = std::io::stdout();
        if self.settings.mouse {
            let _ = crossterm::execute!(out, crossterm::event::EnableMouseCapture);
        } else {
            let _ = crossterm::execute!(out, crossterm::event::DisableMouseCapture);
        }
    }

    pub(super) fn on_api(&mut self, r: ApiReply) {
        match r {
            ApiReply::Site(r) => self.on_site(r),
            ApiReply::Eicons(r) => self.on_eicons(r),
            ApiReply::Profile(name, data) => {
                if name.eq_ignore_ascii_case(&self.chat.me) {
                    match &data {
                        Ok(v) => self.my_kinks = Some(std::sync::Arc::new(super::profile::my_kinks_from(v))),
                        Err(_) => self.my_kinks_requested = false,
                    }
                }
                if let (Ok(v), Some(api)) = (&data, &self.api) {
                    api.remember_id(&name, crate::protocol::vi(v, "id"));
                }
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Profile(p) = d {
                        if p.name.eq_ignore_ascii_case(&name) {
                            p.set_data(data.clone());
                        }
                    }
                }
            }
            ApiReply::Mapping(m) => match m {
                Ok(m) => self.mapping = Some(std::sync::Arc::new(m)),
                Err(e) => self.flash(&format!("Could not load kink/info lists: {e}")),
            },
            ApiReply::FriendLists(r) => {
                if let Ok(l) = &r {
                    let me = self.chat.me.to_lowercase();
                    self.chat.bookmarks = l.bookmarks.iter().map(|b| b.to_lowercase()).collect();
                    self.chat.friends = l.friends.iter().filter(|f| f.1.to_lowercase() == me).map(|f| f.0.to_lowercase()).collect();
                }
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Friends(f) = d {
                        f.data = Some(r.clone());
                    }
                }
            }
            ApiReply::Memo(name, r) => match r {
                Ok(note) => {
                    let mut d = Dialog::prompt(&format!("Memo for {name} (Ctrl+S saves)"), &note, PromptKind::Memo, true);
                    if let Dialog::Prompt(p) = &mut d {
                        p.extra = name;
                    }
                    self.dialogs.push(d);
                }
                Err(e) => self.flash(&format!("Memo: {e}")),
            },
            ApiReply::Guestbook(name, page, r) => {
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Profile(p) = d {
                        if p.name.eq_ignore_ascii_case(&name) {
                            p.guestbook = Some((page, r.clone()));
                        }
                    }
                }
            }
            ApiReply::CharCard(name, r) => {
                self.char_cards.insert(name.to_lowercase(), Some(r.map(std::sync::Arc::new)));
            }
            ApiReply::CharFriends(name, r) => {
                for d in self.dialogs.iter_mut() {
                    if let Dialog::Profile(p) = d {
                        if p.name.eq_ignore_ascii_case(&name) {
                            p.friends = Some(r.clone());
                        }
                    }
                }
            }
            ApiReply::SearchFields(r) => match r {
                Ok(v) => {
                    self.search_fields = Some(v.clone());
                    for d in self.dialogs.iter_mut() {
                        if let Dialog::Search(s) = d {
                            s.load_fields(&v);
                        }
                    }
                }
                Err(e) => {
                    for d in self.dialogs.iter_mut() {
                        if let Dialog::Search(s) = d {
                            s.status = format!("Could not load search fields: {e}");
                        }
                    }
                }
            },
            ApiReply::Report { character, reason, result } => match result {
                Ok(logid) => {
                    self.send(protocol::sfc_report(&character, &reason, Some(logid)));
                    self.info("Report sent to the moderators.");
                }
                Err(e) => {
                    // Still alert the moderators, without the log.
                    self.send(protocol::sfc_report(&character, &reason, None));
                    self.info(&format!("Log upload failed ({e}); the moderators were alerted without a log."));
                }
            },
            ApiReply::Done(what, r) => match r {
                Ok(()) => {
                    self.flash(&what);
                    self.info(&what);
                    if what.starts_with("Friend request") || what.contains("friend") {
                        for d in &self.dialogs {
                            if matches!(d, Dialog::Friends(_)) {
                                self.api_call(|api| ApiReply::FriendLists(api.friend_lists()));
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    self.flash(&format!("Failed: {e}"));
                    self.info(&format!("Failed: {e}"));
                }
            },
        }
    }
}

fn strip_me(wire: &str) -> String {
    let rest = &wire[3..];
    if rest.starts_with('\'') {
        rest.to_string()
    } else {
        rest.trim_start().to_string()
    }
}

impl App {
    /// Start an in-tab search and jump to the newest match.
    pub(super) fn find_start(&mut self, text: &str) {
        let text = text.trim();
        self.chat.active_conv_mut().find_hit = None;
        if text.is_empty() {
            self.find = None;
            return;
        }
        self.find = Some(text.to_string());
        self.find_step(true);
    }

    /// Jump to the next older (or newer) message containing the search text.
    pub(super) fn find_step(&mut self, older: bool) {
        let Some(q) = self.find.as_ref().map(|q| q.to_lowercase()) else { return };
        let conv = self.chat.active_conv();
        let hit = |i: &usize| {
            let m = &conv.messages[*i];
            m.sender.to_lowercase().contains(&q) || crate::bbcode::strip(&m.text).to_lowercase().contains(&q)
        };
        let n = conv.messages.len();
        let found = if older { (0..conv.find_hit.unwrap_or(n)).rev().find(hit) } else { conv.find_hit.and_then(|h| (h + 1..n).find(hit)) };
        match found {
            Some(i) => {
                self.chat.active_conv_mut().find_hit = Some(i);
                self.scroll_to_message(i);
            }
            None if conv.find_hit.is_none() => self.flash("Not found in this tab (older messages are in the logs, F7)."),
            None if older => self.flash("No older matches."),
            None => {
                // Past the newest match: back to the bottom.
                let c = self.chat.active_conv_mut();
                c.find_hit = None;
                c.scroll = 0;
                self.flash("No newer matches.");
            }
        }
    }
}
