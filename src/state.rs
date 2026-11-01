//! In-memory model of the chat: characters, channels, private conversations.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use ratatui::style::Color;

use crate::protocol::ListedChannel;
use crate::timeutil;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Looking,
    Online,
    Busy,
    Dnd,
    Idle,
    Away,
    Crown,
    Offline,
}

impl Status {
    pub fn parse(s: &str) -> Status {
        match s.to_ascii_lowercase().as_str() {
            "looking" => Status::Looking,
            "busy" => Status::Busy,
            "dnd" => Status::Dnd,
            "idle" => Status::Idle,
            "away" => Status::Away,
            "crown" => Status::Crown,
            "offline" => Status::Offline,
            _ => Status::Online,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Looking => "looking",
            Status::Online => "online",
            Status::Busy => "busy",
            Status::Dnd => "dnd",
            Status::Idle => "idle",
            Status::Away => "away",
            Status::Crown => "crown",
            Status::Offline => "offline",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Status::Looking => "Looking",
            Status::Online => "Online",
            Status::Busy => "Busy",
            Status::Dnd => "Do not disturb",
            Status::Idle => "Idle",
            Status::Away => "Away",
            Status::Crown => "Rewarded",
            Status::Offline => "Offline",
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Status::Looking => "+",
            Status::Online => "·",
            Status::Busy => "~",
            Status::Dnd => "!",
            Status::Idle => "z",
            Status::Away => "-",
            Status::Crown => "♛",
            Status::Offline => " ",
        }
    }
    pub fn color(self) -> Color {
        match self {
            Status::Looking => Color::LightGreen,
            Status::Online => Color::Gray,
            Status::Busy => Color::Rgb(0xff, 0xa5, 0x00),
            Status::Dnd => Color::LightRed,
            Status::Idle => Color::DarkGray,
            Status::Away => Color::DarkGray,
            Status::Crown => Color::Yellow,
            Status::Offline => Color::DarkGray,
        }
    }
    /// Statuses a user may set.
    pub const SETTABLE: [Status; 6] = [Status::Online, Status::Looking, Status::Busy, Status::Dnd, Status::Away, Status::Idle];
}

pub fn gender_color(gender: &str) -> Color {
    match gender.to_ascii_lowercase().as_str() {
        "male" => Color::Rgb(0x66, 0x99, 0xff),
        "female" => Color::Rgb(0xff, 0x66, 0x99),
        "herm" => Color::Rgb(0x9b, 0x30, 0xff),
        "male-herm" => Color::Rgb(0x00, 0x7f, 0xff),
        "shemale" => Color::Rgb(0xcc, 0x66, 0xff),
        "cunt-boy" => Color::Rgb(0x00, 0xcc, 0x66),
        "transgender" => Color::Rgb(0xee, 0x88, 0x22),
        "none" => Color::Rgb(0xff, 0xff, 0xbb),
        _ => Color::White,
    }
}

#[derive(Debug, Clone)]
pub struct Character {
    pub name: String,
    pub gender: String,
    pub status: Status,
    pub status_msg: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Chat,
    Action,
    Ad,
    Roll,
    Warn,
    System,
    Error,
    Event,
    Broadcast,
    History,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub ts: i64,
    pub kind: MsgKind,
    pub sender: String,
    pub text: String,
    pub highlight: bool,
}

impl Message {
    pub fn new(kind: MsgKind, sender: &str, text: &str) -> Message {
        Message { ts: timeutil::now(), kind, sender: sender.to_string(), text: text.to_string(), highlight: false }
    }
    pub fn system(text: &str) -> Message {
        Message::new(MsgKind::System, "", text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvKind {
    Console,
    Channel(String),
    Private(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    Clear,
    Paused,
    Typing,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub kind: ConvKind,
    pub title: String,
    pub messages: VecDeque<Message>,
    pub unread: usize,
    pub unread_highlight: bool,
    /// Lines scrolled up from the bottom (0 = following new messages).
    pub scroll: usize,
    pub draft: String,
    // Channel data
    pub members: BTreeSet<String>,
    pub ops: Vec<String>,
    pub owner: String,
    pub mode: String,
    pub description: String,
    pub show_ads: bool,
    pub joined: bool,
    /// When we last posted an ad here (for the server's ad cooldown).
    pub last_ad: Option<std::time::Instant>,
    /// Index of the first message that arrived while the tab was not being
    /// viewed; a "new messages" line is drawn above it.
    pub marker: Option<usize>,
    /// Index of the message found by the in-tab search (Ctrl+F).
    pub find_hit: Option<usize>,
    // Private chat data
    pub typing: Typing,
    pub pinned: bool,
    pub muted: bool,
    pub history_loaded: bool,
}

impl Conversation {
    pub fn new(kind: ConvKind, title: &str) -> Conversation {
        Conversation {
            kind,
            title: title.to_string(),
            messages: VecDeque::new(),
            unread: 0,
            unread_highlight: false,
            scroll: 0,
            draft: String::new(),
            members: BTreeSet::new(),
            ops: Vec::new(),
            owner: String::new(),
            mode: "both".into(),
            description: String::new(),
            show_ads: true,
            joined: false,
            last_ad: None,
            marker: None,
            find_hit: None,
            typing: Typing::Clear,
            pinned: false,
            muted: false,
            history_loaded: false,
        }
    }

    pub fn key(&self) -> String {
        match &self.kind {
            ConvKind::Console => "console".into(),
            ConvKind::Channel(id) => id.clone(),
            ConvKind::Private(name) => name.to_lowercase(),
        }
    }

    pub fn channel_id(&self) -> Option<&str> {
        match &self.kind {
            ConvKind::Channel(id) => Some(id),
            _ => None,
        }
    }

    pub fn partner(&self) -> Option<&str> {
        match &self.kind {
            ConvKind::Private(n) => Some(n),
            _ => None,
        }
    }

    pub fn push(&mut self, msg: Message, max: usize) {
        if self.scroll > 0 {
            // Keep the view steady while scrolled up.
            self.scroll += 1 + msg.text.matches('\n').count();
        }
        self.messages.push_back(msg);
        while self.messages.len() > max.max(50) {
            self.messages.pop_front();
            self.shift_indexes(-1);
        }
    }

    /// Keep the marker and search hit on the same message after messages
    /// were added (+) or removed (-) at the front.
    pub fn shift_indexes(&mut self, delta: isize) {
        let shift = |i: usize| i.checked_add_signed(delta);
        self.marker = self.marker.map(|m| shift(m).unwrap_or(0));
        self.find_hit = self.find_hit.and_then(shift);
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.scroll = 0;
        self.marker = None;
        self.find_hit = None;
    }

    /// Put the "new messages" line above the next message, unless one is already set.
    pub fn mark_unread(&mut self) {
        if self.marker.is_none() {
            self.marker = Some(self.messages.len());
            if self.scroll > 0 {
                self.scroll += 1;
            }
        }
    }

    /// Time left before another ad may be posted here.
    pub fn ad_wait(&self, flood_secs: f64) -> Option<std::time::Duration> {
        let wait = std::time::Duration::from_secs_f64(flood_secs.max(0.0));
        let left = wait.checked_sub(self.last_ad?.elapsed())?;
        (left.as_secs() > 0).then_some(left)
    }

    pub fn is_op(&self, name: &str) -> bool {
        self.ops.iter().any(|o| o.eq_ignore_ascii_case(name))
    }

    pub fn is_owner(&self, name: &str) -> bool {
        !self.owner.is_empty() && self.owner.eq_ignore_ascii_case(name)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ServerVars {
    pub chat_max: usize,
    pub priv_max: usize,
    pub lfrp_max: usize,
    pub lfrp_flood: f64,
    pub msg_flood: f64,
    pub permissions: i64,
    pub icon_blacklist: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Chat {
    pub me: String,
    pub characters: HashMap<String, Character>,
    pub conversations: Vec<Conversation>,
    pub active: usize,
    pub friends: BTreeSet<String>,
    pub bookmarks: BTreeSet<String>,
    pub ignored: BTreeSet<String>,
    pub chatops: HashSet<String>,
    pub vars: ServerVars,
    pub public_channels: Vec<ListedChannel>,
    pub private_rooms: Vec<ListedChannel>,
    pub user_count: i64,
}

impl Chat {
    pub fn new() -> Chat {
        let mut c = Chat {
            vars: ServerVars { chat_max: 4096, priv_max: 50000, lfrp_max: 50000, lfrp_flood: 600.0, msg_flood: 0.5, ..Default::default() },
            ..Default::default()
        };
        c.conversations.push(Conversation::new(ConvKind::Console, "Console"));
        c
    }

    pub fn character(&self, name: &str) -> Option<&Character> {
        self.characters.get(&name.to_lowercase())
    }

    pub fn status_of(&self, name: &str) -> Status {
        self.character(name).map(|c| c.status).unwrap_or(Status::Offline)
    }

    pub fn is_online(&self, name: &str) -> bool {
        self.characters.contains_key(&name.to_lowercase())
    }

    pub fn gender_of(&self, name: &str) -> String {
        self.character(name).map(|c| c.gender.clone()).unwrap_or_default()
    }

    pub fn set_character(&mut self, name: &str, gender: &str, status: &str, msg: &str) {
        self.characters.insert(
            name.to_lowercase(),
            Character { name: name.to_string(), gender: gender.to_string(), status: Status::parse(status), status_msg: msg.to_string() },
        );
    }

    pub fn is_friend(&self, name: &str) -> bool {
        self.friends.contains(&name.to_lowercase())
    }

    pub fn is_bookmarked(&self, name: &str) -> bool {
        self.bookmarks.contains(&name.to_lowercase())
    }

    pub fn is_ignored(&self, name: &str) -> bool {
        self.ignored.contains(&name.to_lowercase())
    }

    pub fn is_chatop(&self, name: &str) -> bool {
        self.chatops.contains(&name.to_lowercase())
    }

    pub fn find(&self, kind: &ConvKind) -> Option<usize> {
        self.conversations.iter().position(|c| match (&c.kind, kind) {
            (ConvKind::Private(a), ConvKind::Private(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a == b,
        })
    }

    pub fn find_channel(&self, id: &str) -> Option<usize> {
        self.find(&ConvKind::Channel(id.to_string()))
    }

    pub fn find_private(&self, name: &str) -> Option<usize> {
        self.find(&ConvKind::Private(name.to_string()))
    }

    /// Find or create a conversation, keeping channels before private chats.
    pub fn ensure(&mut self, kind: ConvKind, title: &str) -> usize {
        if let Some(i) = self.find(&kind) {
            return i;
        }
        let conv = Conversation::new(kind.clone(), title);
        let idx = match kind {
            ConvKind::Channel(_) => {
                self.conversations.iter().rposition(|c| !matches!(c.kind, ConvKind::Private(_))).map(|i| i + 1).unwrap_or(1)
            }
            _ => self.conversations.len(),
        };
        self.conversations.insert(idx, conv);
        if idx <= self.active && self.conversations.len() > 1 {
            self.active += 1;
        }
        idx
    }

    pub fn remove(&mut self, idx: usize) {
        if idx == 0 || idx >= self.conversations.len() {
            return;
        }
        self.conversations.remove(idx);
        if self.active >= idx {
            self.active = self.active.saturating_sub(1);
        }
        self.active = self.active.min(self.conversations.len() - 1);
    }

    pub fn active_conv(&self) -> &Conversation {
        &self.conversations[self.active.min(self.conversations.len() - 1)]
    }

    pub fn active_conv_mut(&mut self) -> &mut Conversation {
        let i = self.active.min(self.conversations.len() - 1);
        &mut self.conversations[i]
    }

    pub fn console(&mut self) -> &mut Conversation {
        &mut self.conversations[0]
    }

    /// Sorted member list for display: owner/ops first, then by status, then name.
    pub fn sorted_members(&self, conv: &Conversation) -> Vec<String> {
        let mut v: Vec<&String> = conv.members.iter().collect();
        v.sort_by_cached_key(|n| {
            let rank = if conv.is_owner(n) {
                0
            } else if conv.is_op(n) || self.is_chatop(n) {
                1
            } else if self.is_friend(n) || self.is_bookmarked(n) {
                2
            } else {
                3
            };
            let st = match self.status_of(n) {
                Status::Looking => 0,
                Status::Online | Status::Crown => 1,
                Status::Busy | Status::Dnd => 2,
                _ => 3,
            };
            (rank, st, n.to_lowercase())
        });
        v.into_iter().cloned().collect()
    }

    /// Names that match a prefix, for tab completion. Members of the given conversation first.
    pub fn complete_names(&self, prefix: &str, conv: Option<&Conversation>) -> Vec<String> {
        let p = prefix.to_lowercase();
        let mut out: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        if let Some(c) = conv {
            if let Some(name) = c.partner() {
                if name.to_lowercase().starts_with(&p) && seen.insert(name.to_lowercase()) {
                    out.push(name.to_string());
                }
            }
            // Recent speakers first.
            for m in c.messages.iter().rev().take(200) {
                if !m.sender.is_empty() && m.sender.to_lowercase().starts_with(&p) && seen.insert(m.sender.to_lowercase()) {
                    out.push(m.sender.clone());
                }
            }
            for n in &c.members {
                if n.to_lowercase().starts_with(&p) && seen.insert(n.to_lowercase()) {
                    out.push(n.clone());
                }
            }
        }
        let mut rest: Vec<&Character> =
            self.characters.values().filter(|c| c.name.to_lowercase().starts_with(&p) && !seen.contains(&c.name.to_lowercase())).collect();
        rest.sort_by_key(|c| (!(self.is_friend(&c.name) || self.is_bookmarked(&c.name)), c.name.to_lowercase()));
        out.extend(rest.into_iter().take(200).map(|c| c.name.clone()));
        out
    }
}
