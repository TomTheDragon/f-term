//! Slash commands: definitions used for execution, help and autocompletion.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    Nothing,
    /// Free text.
    Text,
    /// Any online character.
    Character,
    /// A member of the current channel.
    Member,
    /// Channel name or id (joined or listed).
    Channel,
    Status,
    Mode,
    Setting,
    Dice,
    Number,
    OpenConversation,
    LogConversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Any,
    InChannel,
    InChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    User,
    ChanOp,
    ChanOwner,
    ChatOp,
    Admin,
}

pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub args: &'static [Arg],
    pub usage: &'static str,
    pub help: &'static str,
    pub context: Context,
    pub perm: Perm,
}

macro_rules! c {
    ($name:expr, [$($alias:expr),*], [$($arg:expr),*], $usage:expr, $help:expr, $ctx:expr, $perm:expr) => {
        CommandDef { name: $name, aliases: &[$($alias),*], args: &[$($arg),*], usage: $usage, help: $help, context: $ctx, perm: $perm }
    };
}

use Arg::*;
use Context::*;
use Perm::*;

pub const COMMANDS: &[CommandDef] = &[
    // General
    c!("help", ["?"], [Text], "/help [command]", "Show help for commands and keys", Any, User),
    c!("join", ["j"], [Channel], "/join <channel>", "Join a channel or private room", Any, User),
    c!("close", ["part", "leave"], [OpenConversation], "/close [tab]", "Leave the current channel or close a private chat", Any, User),
    c!(
        "priv",
        ["query", "pm", "msg"],
        [Character, Text],
        "/priv <character>[, message]",
        "Open a private chat (optionally sending a message)",
        Any,
        User
    ),
    c!("me", [], [Text], "/me <action>", "Send an action (emote)", InChat, User),
    c!("ad", ["lrp"], [Text], "/ad <text>", "Post a roleplay ad in the current channel", InChannel, User),
    c!("roll", ["dice"], [Dice], "/roll <dice>", "Roll dice, e.g. 1d20, 2d6+3", InChat, User),
    c!("bottle", [], [], "/bottle", "Spin the bottle", InChat, User),
    c!(
        "status",
        ["sta"],
        [Status, Text],
        "/status <online|looking|busy|dnd|away|idle> [message]",
        "Set your status and status message",
        Any,
        User
    ),
    c!("away", [], [Text], "/away [message]", "Set your status to away", Any, User),
    c!("back", [], [], "/back", "Set your status back to online", Any, User),
    c!("clear", [], [], "/clear", "Clear the messages of the current tab", Any, User),
    c!("clearall", [], [], "/clearall", "Clear the messages of all tabs", Any, User),
    c!("ignore", [], [Character], "/ignore <character>", "Ignore a character", Any, User),
    c!("unignore", [], [Character], "/unignore <character>", "Stop ignoring a character", Any, User),
    c!("ignorelist", [], [], "/ignorelist", "Show your ignore list", Any, User),
    c!("profile", ["p", "whois"], [Character], "/profile [character]", "View a character profile", Any, User),
    c!("memo", [], [Character], "/memo [character]", "Edit your private memo about a character", Any, User),
    c!("kinks", [], [Character], "/kinks <character>", "Show a character's custom kinks (server KIN)", Any, User),
    c!("info", ["pro"], [Character], "/info <character>", "Show profile info via the server (PRO)", Any, User),
    c!("code", [], [], "/code", "Show the BBCode link for the current channel", InChannel, User),
    c!("getdescription", ["topic"], [], "/getdescription", "Show the channel description as BBCode", InChannel, User),
    c!("uptime", [], [], "/uptime", "Show server uptime and statistics", Any, User),
    c!("whoami", [], [], "/whoami", "Show who you are logged in as", Any, User),
    c!("users", ["names", "who"], [], "/users", "List members of the current channel", InChannel, User),
    c!("channels", ["list"], [Text], "/channels [filter]", "Open the channel browser", Any, User),
    c!("rooms", [], [Text], "/rooms [filter]", "Open the channel browser on private rooms", Any, User),
    c!("friends", ["bookmarks"], [], "/friends", "Show friends, bookmarks and friend requests", Any, User),
    c!("search", ["fks"], [], "/search", "Search for characters by kinks and profile fields", Any, User),
    c!("recent", [], [], "/recent", "Reopen a recent conversation", Any, User),
    c!("logs", [], [LogConversation], "/logs [conversation]", "Browse chat logs", Any, User),
    c!("settings", ["options", "config"], [], "/settings", "Open the settings", Any, User),
    c!("set", [], [Setting, Text], "/set <setting> [value]", "Show or change a setting", Any, User),
    c!("images", [], [Text], "/images <off|ascii|colorascii|blocks|native>", "Change how images are shown", Any, User),
    c!("avatar", ["icon"], [Character], "/avatar [character]", "Show a character's avatar", Any, User),
    c!("eicon", [], [Text], "/eicon <name>", "Preview an eicon", Any, User),
    c!("image", ["img", "view"], [Text], "/image <url>", "View an image from a URL", Any, User),
    c!("links", ["urls"], [], "/links", "List links in the current tab", Any, User),
    c!("open", [], [Text], "/open <url>", "Open a link in the browser", Any, User),
    c!("bookmark", [], [Character], "/bookmark <character>", "Bookmark a character", Any, User),
    c!("unbookmark", [], [Character], "/unbookmark <character>", "Remove a bookmark", Any, User),
    c!("friend", ["addfriend"], [Character], "/friend <character>", "Send a friend request", Any, User),
    c!("unfriend", [], [Character], "/unfriend <character>", "Remove a friend", Any, User),
    c!(
        "report",
        ["alert"],
        [Character, Text],
        "/report <character>, <reason>",
        "Report a character to the moderators (uploads a log of this tab)",
        Any,
        User
    ),
    c!("showads", [], [], "/showads", "Toggle showing ads in this channel", InChannel, User),
    c!("mute", [], [], "/mute", "Toggle notifications for this tab", Any, User),
    c!("pin", [], [], "/pin", "Pin/unpin this tab so it reopens on login", Any, User),
    c!("savead", [], [Text], "/savead <text>", "Save a roleplay ad for later (see /ads)", Any, User),
    c!("ads", [], [], "/ads", "Pick a saved ad to post", InChannel, User),
    c!("makeroom", ["createroom"], [Text], "/makeroom <title>", "Create a private room", Any, User),
    c!("reconnect", [], [], "/reconnect", "Reconnect to the server", Any, User),
    c!("logout", ["disconnect"], [], "/logout", "Disconnect and return to the login screen", Any, User),
    c!("quit", ["exit", "q"], [], "/quit", "Quit F-Term", Any, User),
    c!("raw", [], [Text], "/raw <CMD {json}>", "Send a raw protocol command (for testing)", Any, User),
    // Channel moderation
    c!("warn", [], [Text], "/warn <message>", "Send a highlighted warning to the channel", InChannel, ChanOp),
    c!("kick", [], [Member], "/kick <character>", "Kick a character from the channel", InChannel, ChanOp),
    c!("ban", [], [Member], "/ban <character>", "Ban a character from the channel", InChannel, ChanOp),
    c!("unban", [], [Character], "/unban <character>", "Unban a character from the channel", InChannel, ChanOp),
    c!("banlist", [], [], "/banlist", "Show the channel ban list", InChannel, ChanOp),
    c!(
        "timeout",
        [],
        [Member, Number],
        "/timeout <character>, <minutes>",
        "Time a character out of the channel (1-90 minutes)",
        InChannel,
        ChanOp
    ),
    c!("op", [], [Member], "/op <character>", "Make a character channel operator", InChannel, ChanOwner),
    c!("deop", [], [Member], "/deop <character>", "Remove channel operator status", InChannel, ChanOwner),
    c!("oplist", [], [], "/oplist", "List the channel operators", InChannel, User),
    c!("invite", [], [Character], "/invite <character>", "Invite a character to this private room", InChannel, ChanOp),
    c!("openroom", [], [], "/openroom", "Make this private room public (listed)", InChannel, ChanOp),
    c!("closeroom", [], [], "/closeroom", "Make this private room invite-only", InChannel, ChanOp),
    c!("setmode", [], [Mode], "/setmode <chat|ads|both>", "Set which messages the channel allows", InChannel, ChanOp),
    c!("setowner", [], [Member], "/setowner <character>", "Give channel ownership to a character", InChannel, ChanOwner),
    c!("setdescription", ["setdesc"], [Text], "/setdescription <text>", "Change the channel description", InChannel, ChanOp),
    // Global moderation
    c!("gkick", [], [Character], "/gkick <character>", "Kick a character from the chat", Any, ChatOp),
    c!("gban", ["accountban"], [Character], "/gban <character>", "Ban a character's account from the chat", Any, ChatOp),
    c!("gunban", [], [Character], "/gunban <character>", "Unban a character's account", Any, ChatOp),
    c!(
        "gtimeout",
        [],
        [Character, Number, Text],
        "/gtimeout <character>, <minutes>, <reason>",
        "Time out a character from the chat",
        Any,
        ChatOp
    ),
    c!("createchannel", [], [Text], "/createchannel <name>", "Create an official channel", Any, Admin),
    c!("killchannel", [], [Channel], "/killchannel [channel]", "Destroy a channel", Any, ChatOp),
    c!("reward", [], [Character], "/reward <character>", "Give a character the crown status", Any, Admin),
    c!("broadcast", [], [Text], "/broadcast <message>", "Broadcast a message to everyone", Any, Admin),
    c!("gop", [], [Character], "/gop <character>", "Promote a character to chat operator", Any, Admin),
    c!("gdeop", [], [Character], "/gdeop <character>", "Demote a chat operator", Any, Admin),
    c!("reloadconfig", [], [Text], "/reloadconfig [save]", "Reload the server configuration", Any, Admin),
    c!("altwatch", [], [Character], "/altwatch <character>", "List connected alts of a character (deprecated)", Any, ChatOp),
    c!("handlereport", ["confirmreport"], [Text], "/handlereport <call id>", "Confirm handling of a moderator report", Any, ChatOp),
];

pub fn find(name: &str) -> Option<&'static CommandDef> {
    let n = name.to_ascii_lowercase();
    COMMANDS.iter().find(|c| c.name == n || c.aliases.contains(&n.as_str()))
}

/// Split "/name rest" into (name, rest).
pub fn split(input: &str) -> Option<(&str, &str)> {
    let s = input.strip_prefix('/')?;
    if s.starts_with('/') {
        return None; // "//text" sends a literal slash
    }
    let (name, rest) = match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    };
    Some((name, rest))
}

/// Split comma separated arguments; the last one takes the remainder.
pub fn args(rest: &str, n: usize) -> Vec<String> {
    if n <= 1 {
        return vec![rest.trim().to_string()];
    }
    rest.splitn(n, ',').map(|s| s.trim().to_string()).collect()
}

/// Matching commands for a typed prefix (without the slash).
pub fn matching(prefix: &str) -> Vec<&'static CommandDef> {
    let p = prefix.to_ascii_lowercase();
    let mut v: Vec<&CommandDef> = COMMANDS.iter().filter(|c| c.name.starts_with(&p)).collect();
    for c in COMMANDS {
        if !v.iter().any(|x| x.name == c.name) && c.aliases.iter().any(|a| a.starts_with(&p)) {
            v.push(c);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parsing() {
        assert_eq!(split("/join Frontpage"), Some(("join", "Frontpage")));
        assert_eq!(split("//not a command"), None);
        assert_eq!(args("Some Name, 30", 2), vec!["Some Name", "30"]);
        assert_eq!(args("Name, 5, a, b", 3), vec!["Name", "5", "a, b"]);
        assert_eq!(find("query").unwrap().name, "priv");
        assert!(matching("ba").iter().any(|c| c.name == "ban"));
        // No duplicate names/aliases.
        let mut all: Vec<&str> = COMMANDS.iter().flat_map(|c| std::iter::once(c.name).chain(c.aliases.iter().copied())).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(n, all.len());
    }
}
