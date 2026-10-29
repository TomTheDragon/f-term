//! Handling of commands received from the chat server.

use serde_json::Value;

use super::{App, ConnState, Dialog};
use crate::protocol::{self, vi, vs, ServerCmd};
use crate::state::{ConvKind, Message, MsgKind, Status, Typing};
use crate::timeutil;

impl App {
    pub(super) fn on_server(&mut self, cmd: ServerCmd) {
        match cmd {
            ServerCmd::Hlo { message } => self.sys(&message),
            ServerCmd::Idn { character } => {
                self.conn_state = ConnState::Online;
                self.reconnect_attempts = 0;
                self.no_reconnect = false;
                self.chat.me = character.clone();
                self.sys(&format!("Identified as {character}."));
                // Restore our status.
                if self.my_status != Status::Online || !self.my_status_msg.is_empty() {
                    let (st, msg) = (self.my_status, self.my_status_msg.clone());
                    self.set_status(st, &msg, false);
                }
                self.rejoin();
            }
            ServerCmd::Con { count } => {
                self.chat.user_count = count;
                self.sys(&format!("{count} users connected."));
            }
            ServerCmd::Var { variable, value } => {
                let num = value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
                let v = &mut self.chat.vars;
                match variable.as_str() {
                    "chat_max" => v.chat_max = num as usize,
                    "priv_max" => v.priv_max = num as usize,
                    "lfrp_max" => v.lfrp_max = num as usize,
                    "lfrp_flood" => v.lfrp_flood = num,
                    "msg_flood" => v.msg_flood = num,
                    "permissions" => v.permissions = num as i64,
                    "icon_blacklist" => {
                        v.icon_blacklist = value
                            .as_array()
                            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect())
                            .unwrap_or_default()
                    }
                    _ => {}
                }
            }
            ServerCmd::Adl { ops } => {
                self.chat.chatops = ops.iter().map(|o| o.to_lowercase()).collect();
            }
            ServerCmd::Aop { character } => {
                self.chat.chatops.insert(character.to_lowercase());
                self.sys(&format!("{character} is now a chat operator."));
            }
            ServerCmd::Dop { character } => {
                self.chat.chatops.remove(&character.to_lowercase());
                self.sys(&format!("{character} is no longer a chat operator."));
            }
            ServerCmd::Lis { characters } => {
                for [name, gender, status, msg] in characters {
                    self.chat.set_character(&name, &gender, &status, &msg);
                }
            }
            ServerCmd::Frl { characters } => {
                // Friends and bookmarks combined; keep them as "watched".
                for c in characters {
                    let lc = c.to_lowercase();
                    if !self.chat.friends.contains(&lc) {
                        self.chat.bookmarks.insert(lc);
                    }
                }
            }
            ServerCmd::Ign { action, characters, character } => match action.as_str() {
                "init" | "list" => {
                    self.chat.ignored = characters.iter().map(|c| c.to_lowercase()).collect();
                    if action == "list" {
                        self.show_ignore_list();
                    }
                }
                "add" => {
                    self.chat.ignored.insert(character.to_lowercase());
                    self.info(&format!("{character} is now ignored."));
                }
                "delete" => {
                    self.chat.ignored.remove(&character.to_lowercase());
                    self.info(&format!("{character} is no longer ignored."));
                }
                _ => {}
            },
            ServerCmd::Nln { identity, gender, status } => {
                self.chat.set_character(&identity, &gender, &status, "");
                if identity.eq_ignore_ascii_case(&self.chat.me) {
                    self.rejoin();
                } else {
                    if self.settings.show_friend_online && (self.chat.is_friend(&identity) || self.chat.is_bookmarked(&identity)) {
                        self.sys(&format!("[user]{identity}[/user] is now online."));
                    }
                    self.watch_toast(&identity, true);
                }
                if let Some(i) = self.chat.find_private(&identity) {
                    let max = self.settings.max_messages;
                    if self.settings.show_join_leave {
                        self.chat.conversations[i].push(Message::new(MsgKind::Event, "", &format!("{identity} is now online.")), max);
                    }
                }
            }
            ServerCmd::Fln { character } => {
                let lc = character.to_lowercase();
                self.chat.characters.remove(&lc);
                let max = self.settings.max_messages;
                let show = self.settings.show_join_leave;
                for c in &mut self.chat.conversations {
                    let was_member = c.members.iter().any(|m| m.eq_ignore_ascii_case(&character));
                    if was_member {
                        c.members.retain(|m| !m.eq_ignore_ascii_case(&character));
                        if show {
                            c.push(Message::new(MsgKind::Event, "", &format!("{character} disconnected.")), max);
                        }
                    }
                    if c.partner().map(|p| p.eq_ignore_ascii_case(&character)).unwrap_or(false) {
                        c.typing = Typing::Clear;
                        c.push(Message::new(MsgKind::Event, "", &format!("{character} is now offline.")), max);
                    }
                }
                if self.settings.show_friend_online && (self.chat.is_friend(&character) || self.chat.is_bookmarked(&character)) {
                    self.sys(&format!("[user]{character}[/user] is now offline."));
                }
                self.watch_toast(&character, false);
            }
            ServerCmd::Sta { character, status, statusmsg } => {
                let lc = character.to_lowercase();
                if let Some(c) = self.chat.characters.get_mut(&lc) {
                    c.status = Status::parse(&status);
                    c.status_msg = statusmsg.clone();
                }
                if character.eq_ignore_ascii_case(&self.chat.me) {
                    self.my_status = Status::parse(&status);
                    self.my_status_msg = statusmsg.clone();
                } else {
                    let text = if statusmsg.is_empty() {
                        format!("{character} is now {}.", Status::parse(&status).label())
                    } else {
                        format!("{character} is now {}: {statusmsg}", Status::parse(&status).label())
                    };
                    if self.settings.show_status_changes && (self.chat.is_friend(&character) || self.chat.is_bookmarked(&character)) {
                        self.sys(&text);
                    }
                    if let Some(i) = self.chat.find_private(&character) {
                        let max = self.settings.max_messages;
                        self.chat.conversations[i].push(Message::new(MsgKind::Event, "", &text), max);
                    }
                }
            }
            ServerCmd::Cha { channels } => {
                self.chat.public_channels = channels;
            }
            ServerCmd::Ors { channels } => {
                self.chat.private_rooms = channels;
            }
            ServerCmd::Jch { channel, character, title } => {
                if character.eq_ignore_ascii_case(&self.chat.me) {
                    let existed = self.chat.find_channel(&channel).is_some();
                    let i = self.chat.ensure(ConvKind::Channel(channel.clone()), &title);
                    let conv = &mut self.chat.conversations[i];
                    if !title.is_empty() {
                        conv.title = title.clone();
                    }
                    conv.joined = true;
                    conv.members.clear();
                    let key = conv.key();
                    conv.muted = self.char_state.muted.contains(&key);
                    conv.show_ads = self.settings.show_ads && !self.char_state.ads_hidden.contains(&key);
                    if !existed {
                        self.switch_to(i);
                    }
                    self.load_history(i);
                    let max = self.settings.max_messages;
                    self.chat.conversations[i].push(Message::new(MsgKind::Event, "", &format!("You joined {title}.")), max);
                    self.remember_tabs();
                } else if let Some(i) = self.chat.find_channel(&channel) {
                    let conv = &mut self.chat.conversations[i];
                    conv.members.insert(character.clone());
                    if self.settings.show_join_leave {
                        let max = self.settings.max_messages;
                        conv.push(Message::new(MsgKind::Event, "", &format!("{character} joined.")), max);
                    }
                }
            }
            ServerCmd::Lch { channel, character } => {
                let Some(i) = self.chat.find_channel(&channel) else { return };
                if character.eq_ignore_ascii_case(&self.chat.me) {
                    self.chat.remove(i);
                    let d = self.chat.active_conv().draft.clone();
                    self.input.set(&d);
                    self.remember_tabs();
                } else {
                    let conv = &mut self.chat.conversations[i];
                    conv.members.retain(|m| !m.eq_ignore_ascii_case(&character));
                    if self.settings.show_join_leave {
                        let max = self.settings.max_messages;
                        conv.push(Message::new(MsgKind::Event, "", &format!("{character} left.")), max);
                    }
                }
            }
            ServerCmd::Ich { channel, users, mode } => {
                let Some(i) = self.chat.find_channel(&channel) else {
                    // Not a channel we asked for; leave it.
                    self.send(protocol::lch(&channel));
                    return;
                };
                let conv = &mut self.chat.conversations[i];
                conv.members = users.into_iter().collect();
                conv.mode = mode;
                conv.joined = true;
            }
            ServerCmd::Cds { channel, description } => {
                if let Some(i) = self.chat.find_channel(&channel) {
                    let changed =
                        !self.chat.conversations[i].description.is_empty() && self.chat.conversations[i].description != description;
                    self.chat.conversations[i].description = description;
                    if changed {
                        let max = self.settings.max_messages;
                        self.chat.conversations[i].push(Message::system("The channel description was changed."), max);
                    }
                }
            }
            ServerCmd::Col { channel, oplist } => {
                if let Some(i) = self.chat.find_channel(&channel) {
                    let conv = &mut self.chat.conversations[i];
                    conv.owner = oplist.first().cloned().unwrap_or_default();
                    conv.ops = oplist.into_iter().filter(|o| !o.is_empty()).collect();
                }
            }
            ServerCmd::Coa { character, channel } => {
                if let Some(i) = self.chat.find_channel(&channel) {
                    let conv = &mut self.chat.conversations[i];
                    if !conv.is_op(&character) {
                        conv.ops.push(character.clone());
                    }
                    let max = self.settings.max_messages;
                    conv.push(Message::system(&format!("{character} is now a channel operator.")), max);
                }
            }
            ServerCmd::Cor { character, channel } => {
                if let Some(i) = self.chat.find_channel(&channel) {
                    let conv = &mut self.chat.conversations[i];
                    conv.ops.retain(|o| !o.eq_ignore_ascii_case(&character));
                    let max = self.settings.max_messages;
                    conv.push(Message::system(&format!("{character} is no longer a channel operator.")), max);
                }
            }
            ServerCmd::Cso { character, channel } => {
                if let Some(i) = self.chat.find_channel(&channel) {
                    let conv = &mut self.chat.conversations[i];
                    conv.owner = character.clone();
                    if !conv.is_op(&character) {
                        conv.ops.insert(0, character.clone());
                    }
                    let max = self.settings.max_messages;
                    conv.push(Message::system(&format!("{character} is now the channel owner.")), max);
                }
            }
            ServerCmd::Rmo { mode, channel } => {
                if let Some(i) = self.chat.find_channel(&channel) {
                    let conv = &mut self.chat.conversations[i];
                    conv.mode = mode.clone();
                    let what = match mode.as_str() {
                        "chat" => "only chat messages",
                        "ads" => "only ads",
                        _ => "chat messages and ads",
                    };
                    let max = self.settings.max_messages;
                    conv.push(Message::system(&format!("The channel now allows {what}.")), max);
                }
            }
            ServerCmd::Cku { operator, channel, character } => self.channel_removal(&channel, &character, &operator, "kicked", None),
            ServerCmd::Cbu { operator, channel, character } => self.channel_removal(&channel, &character, &operator, "banned", None),
            ServerCmd::Ctu { operator, channel, length, character } => {
                self.channel_removal(&channel, &character, &operator, "timed out", Some(length))
            }
            ServerCmd::Ciu { sender, title, name } => {
                if self.chat.is_ignored(&sender) {
                    return;
                }
                self.sys(&format!("[user]{sender}[/user] has invited you to join [session={title}]{name}[/session]. Use /join {name}"));
                let preview = format!("{sender} invited you to {title}");
                crate::notify::notify(&self.settings, "Channel invite", &preview);
                self.dialogs.push(Dialog::confirm(
                    &format!("{sender} invited you to join \"{title}\". Join now?"),
                    super::actions::Action::Join(name),
                ));
            }
            ServerCmd::Msg { character, message, channel } => {
                if self.chat.is_ignored(&character) {
                    return;
                }
                let Some(i) = self.chat.find_channel(&channel) else { return };
                let kind = if let Some(rest) = message.strip_prefix("/me") {
                    let _ = rest;
                    MsgKind::Action
                } else if message.starts_with("/warn ") && (self.chat.conversations[i].is_op(&character) || self.chat.is_chatop(&character))
                {
                    MsgKind::Warn
                } else {
                    MsgKind::Chat
                };
                let text = strip_command(&message, kind);
                self.add_message(i, Message::new(kind, &character, &text));
            }
            ServerCmd::Lrp { character, message, channel } => {
                if self.chat.is_ignored(&character) {
                    return;
                }
                let Some(i) = self.chat.find_channel(&channel) else { return };
                self.add_message(i, Message::new(MsgKind::Ad, &character, &message));
            }
            ServerCmd::Pri { character, message } => {
                if self.chat.is_ignored(&character) {
                    self.send(protocol::ign("notify", Some(&character)));
                    return;
                }
                let i = self.open_private(&character, false);
                self.chat.conversations[i].typing = Typing::Clear;
                let kind = if message.starts_with("/me") { MsgKind::Action } else { MsgKind::Chat };
                let text = strip_command(&message, kind);
                self.add_message(i, Message::new(kind, &character, &text));
            }
            ServerCmd::Tpn { character, status } => {
                if let Some(i) = self.chat.find_private(&character) {
                    self.chat.conversations[i].typing = match status.as_str() {
                        "typing" => Typing::Typing,
                        "paused" => Typing::Paused,
                        _ => Typing::Clear,
                    };
                }
            }
            ServerCmd::Rll { channel, character, message, kind, target, recipient } => {
                if self.chat.is_ignored(&character) {
                    return;
                }
                let _ = (kind, target);
                let idx = if !channel.is_empty() {
                    self.chat.find_channel(&channel)
                } else {
                    // Private roll: shown in the conversation with the other person.
                    let other = if character.eq_ignore_ascii_case(&self.chat.me) { recipient } else { character.clone() };
                    if other.is_empty() {
                        None
                    } else {
                        Some(self.open_private(&other, false))
                    }
                };
                if let Some(i) = idx {
                    self.add_message(i, Message::new(MsgKind::Roll, &character, &message));
                }
            }
            ServerCmd::Bro { message, character } => {
                let text = if character.is_empty() { message } else { format!("{character}: {message}") };
                let n = self.chat.conversations.len();
                for i in 0..n {
                    if i == 0 || self.chat.conversations[i].channel_id().is_some() || i == self.chat.active {
                        let max = self.settings.max_messages;
                        self.chat.conversations[i].push(Message::new(MsgKind::Broadcast, "", &text), max);
                    }
                }
                crate::notify::notify(&self.settings, "Broadcast", &text);
            }
            ServerCmd::Sys { message, channel } => {
                let idx = if channel.is_empty() { None } else { self.chat.find_channel(&channel) };
                let max = self.settings.max_messages;
                match idx {
                    Some(i) => self.chat.conversations[i].push(Message::system(&message), max),
                    None => {
                        self.sys(&message);
                        if self.chat.active != 0 {
                            self.info(&message);
                        }
                    }
                }
            }
            ServerCmd::Err { number, message } => {
                if protocol::is_fatal_error(number) {
                    self.no_reconnect = true;
                }
                if number == 4 {
                    // Identification failed: the ticket was probably stale.
                    if let Some(api) = &self.api {
                        api.invalidate_ticket();
                    }
                }
                if number == 18 || number == 72 || number == 50 || number == 61 {
                    // Search errors
                    if let Some(Dialog::Search(s)) = self.dialogs.last_mut() {
                        s.status = message.clone();
                        return;
                    }
                }
                self.sys_err(&format!("Error {number}: {message}"));
            }
            ServerCmd::Upt { time, starttime, startstring, accepted, channels, users, maxusers } => {
                let up = timeutil::fmt_duration(time - starttime);
                let text = format!(
                    "Server up for {up} (since {startstring}). {users} users online (peak {maxusers}), {channels} channels, {accepted} connections accepted."
                );
                self.info(&text);
            }
            ServerCmd::Kid { kind, message, key, value } => match kind.as_str() {
                "start" => {
                    self.kinks_dump.clear();
                    self.kinks_dump.push(message);
                }
                "custom" => self.kinks_dump.push(format!("[b]{key}[/b]: {value}")),
                _ => {
                    self.kinks_dump.push(message);
                    let lines = std::mem::take(&mut self.kinks_dump);
                    self.dialogs.push(Dialog::text_bbcode("Custom kinks", &lines.join("\n")));
                }
            },
            ServerCmd::Prd { kind, message, key, value } => match kind.as_str() {
                "start" => {
                    self.info_dump.clear();
                    self.info_dump.push(message);
                }
                "info" | "select" => self.info_dump.push(format!("[b]{key}[/b]: {value}")),
                _ => {
                    self.info_dump.push(message);
                    let lines = std::mem::take(&mut self.info_dump);
                    self.dialogs.push(Dialog::text_bbcode("Profile info", &lines.join("\n")));
                }
            },
            ServerCmd::Fks { characters } => {
                if let Some(Dialog::Search(s)) = self.dialogs.last_mut() {
                    let mut c = characters;
                    c.sort_by_key(|a| a.to_lowercase());
                    s.status = format!("{} results", c.len());
                    s.results = c;
                    s.focus = super::dialogs::SearchFocus::Results;
                    s.result_sel = 0;
                } else {
                    self.sys(&format!("Search results: {}", characters.join(", ")));
                }
            }
            ServerCmd::Rtb { kind, data } => self.on_rtb(&kind, &data),
            ServerCmd::Sfc { data } => {
                let action = vs(&data, "action");
                if action == "report" {
                    let text = format!(
                        "[b]Moderator alert[/b] (call {}) from [user]{}[/user]: {} {}",
                        vs(&data, "callid"),
                        vs(&data, "character"),
                        vs(&data, "report"),
                        match vi(&data, "logid") {
                            0 => String::new(),
                            id => format!("[url=https://www.f-list.net/fchat/getLog.php?log={id}]log[/url]"),
                        }
                    );
                    self.sys(&text);
                    crate::notify::notify(&self.settings, "Moderator alert", &vs(&data, "report"));
                } else if action == "confirm" {
                    self.sys(&format!("{} is handling the report of {}.", vs(&data, "moderator"), vs(&data, "character")));
                }
            }
            ServerCmd::Pin => {}
            ServerCmd::Unknown { code, data } => {
                crate::net::log::debug(&format!("unknown command {code} {data}"));
            }
        }
    }

    fn channel_removal(&mut self, channel: &str, character: &str, operator: &str, what: &str, minutes: Option<i64>) {
        let Some(i) = self.chat.find_channel(channel) else { return };
        let title = self.chat.conversations[i].title.clone();
        let extra = minutes.map(|m| format!(" for {m} minutes")).unwrap_or_default();
        let text = format!("{operator} has {what} {character} from {title}{extra}.");
        if character.eq_ignore_ascii_case(&self.chat.me) {
            self.chat.conversations[i].joined = false;
            self.chat.conversations[i].members.clear();
            self.sys_err(&text);
            let max = self.settings.max_messages;
            self.chat.conversations[i].push(Message::new(MsgKind::Error, "", &text), max);
        } else {
            let conv = &mut self.chat.conversations[i];
            conv.members.retain(|m| !m.eq_ignore_ascii_case(character));
            let max = self.settings.max_messages;
            conv.push(Message::system(&text), max);
        }
    }

    fn on_rtb(&mut self, kind: &str, data: &Value) {
        let name = vs(data, "name");
        let text = match kind {
            "note" => format!(
                "New note from [user]{}[/user]: [url=https://www.f-list.net/view_note.php?note_id={}]{}[/url]",
                vs(data, "sender"),
                vs(data, "id"),
                vs(data, "subject")
            ),
            "comment" => format!("[user]{}[/user] commented on your {} \"{}\".", name, vs(data, "target_type"), vs(data, "target")),
            "grouprequest" => format!("[user]{name}[/user] requested to join a group: {}", vs(data, "title")),
            "bugreport" => format!("[user]{name}[/user] submitted a bug report: {}", vs(data, "title")),
            "helpdeskticket" => format!("[user]{name}[/user] submitted a helpdesk ticket: {}", vs(data, "title")),
            "helpdeskreply" => format!("[user]{name}[/user] replied to a helpdesk ticket."),
            "featurerequest" => format!("[user]{name}[/user] submitted a feature request: {}", vs(data, "title")),
            "trackadd" => {
                self.chat.bookmarks.insert(name.to_lowercase());
                format!("[user]{name}[/user] was added to your bookmarks.")
            }
            "trackrem" => {
                self.chat.bookmarks.remove(&name.to_lowercase());
                format!("[user]{name}[/user] was removed from your bookmarks.")
            }
            "friendadd" => {
                self.chat.friends.insert(name.to_lowercase());
                format!("[user]{name}[/user] is now your friend.")
            }
            "friendremove" => {
                self.chat.friends.remove(&name.to_lowercase());
                format!("[user]{name}[/user] is no longer your friend.")
            }
            "friendrequest" => format!("[user]{name}[/user] sent you a friend request. Press F3 to answer it."),
            other => format!("Site notification ({other}): {data}"),
        };
        self.sys(&text);
        if matches!(kind, "note" | "friendrequest") {
            crate::notify::notify(&self.settings, "F-List", &text);
        }
    }

    /// Rejoin channels after login (once per connection).
    fn rejoin(&mut self) {
        if self.joined_after_login {
            return;
        }
        self.joined_after_login = true;
        let ids: Vec<String> = self.chat.conversations.iter().filter_map(|c| c.channel_id().map(String::from)).collect();
        for id in ids {
            self.send(protocol::jch(&id));
        }
    }

    fn show_ignore_list(&mut self) {
        let list: Vec<String> = self.chat.ignored.iter().cloned().collect();
        if list.is_empty() {
            self.info("Your ignore list is empty.");
        } else {
            self.info(&format!("Ignored: {}", list.join(", ")));
        }
    }
}

/// Remove the "/me" or "/warn" prefix from a message.
fn strip_command(message: &str, kind: MsgKind) -> String {
    match kind {
        MsgKind::Action => {
            let rest = &message[3..];
            // "/me's" keeps the apostrophe attached to the name.
            if rest.starts_with('\'') {
                rest.to_string()
            } else {
                rest.trim_start().to_string()
            }
        }
        MsgKind::Warn => message.trim_start_matches("/warn").trim_start().to_string(),
        _ => message.to_string(),
    }
}
