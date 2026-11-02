//! Persistent settings, saved logins and per-character state.

use std::path::PathBuf;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::paths;

pub const DEFAULT_SERVER: &str = "wss://chat.f-list.net/chat2";
pub const DEFAULT_API: &str = "https://www.f-list.net";
pub const DEFAULT_STATIC: &str = "https://static.f-list.net";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageMode {
    /// Never show images.
    Off,
    /// Plain ASCII characters (works everywhere).
    Ascii,
    /// Coloured ASCII characters.
    ColorAscii,
    /// Unicode half blocks with colours.
    Blocks,
    /// Unicode Braille dots (2x4 per character), coloured.
    Braille,
    /// Real pixels via the terminal's graphics protocol (kitty, iTerm2 or sixel).
    Native,
}

impl ImageMode {
    pub const ALL: [ImageMode; 6] =
        [ImageMode::Off, ImageMode::Ascii, ImageMode::ColorAscii, ImageMode::Blocks, ImageMode::Braille, ImageMode::Native];
    pub fn name(self) -> &'static str {
        match self {
            ImageMode::Off => "off",
            ImageMode::Ascii => "ascii",
            ImageMode::ColorAscii => "colorascii",
            ImageMode::Blocks => "blocks",
            ImageMode::Braille => "braille",
            ImageMode::Native => "native",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsProtocol {
    Auto,
    Kitty,
    Iterm2,
    Sixel,
}

impl GraphicsProtocol {
    pub const ALL: [GraphicsProtocol; 4] =
        [GraphicsProtocol::Auto, GraphicsProtocol::Kitty, GraphicsProtocol::Iterm2, GraphicsProtocol::Sixel];
}

/// Whose coming online and going offline shows a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchNotify {
    Off,
    Friends,
    Bookmarks,
    Both,
}

impl WatchNotify {
    pub const ALL: [WatchNotify; 4] = [WatchNotify::Off, WatchNotify::Friends, WatchNotify::Bookmarks, WatchNotify::Both];

    pub fn wants(self, friend: bool, bookmark: bool) -> bool {
        match self {
            WatchNotify::Off => false,
            WatchNotify::Friends => friend,
            WatchNotify::Bookmarks => bookmark,
            WatchNotify::Both => friend || bookmark,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // Connection
    pub server: String,
    pub api_base: String,
    pub auto_reconnect: bool,
    pub rejoin_channels: bool,
    pub reopen_private_chats: bool,

    // Display
    pub show_timestamps: bool,
    pub timestamp_seconds: bool,
    pub show_join_leave: bool,
    pub show_status_changes: bool,
    pub show_friend_online: bool,
    pub show_ads: bool,
    pub color_names_by_gender: bool,
    pub show_user_list: bool,
    pub show_online_friends: bool,
    pub show_sidebar: bool,
    pub sidebar_width: u16,
    pub user_list_width: u16,
    pub max_messages: usize,
    pub load_history_lines: usize,
    pub truecolor: bool,
    pub mouse: bool,
    pub show_eicon_names: bool,
    pub ascii_borders: bool,

    // Window of the GUI version (f-term-windows-9x-gui.exe)
    pub gui_font: String,
    pub gui_font_size: u16,
    pub gui_columns: u16,
    pub gui_rows: u16,
    pub gui_inline_images: bool,
    /// How the window shows images (`native` = real pixels).
    pub gui_image_mode: ImageMode,

    // Images
    pub image_mode: ImageMode,
    pub graphics_protocol: GraphicsProtocol,
    pub inline_images: bool,
    pub inline_image_rows: u16,
    pub animate_images: bool,

    // Notifications & highlights
    pub highlight_own_name: bool,
    pub highlight_words: Vec<String>,
    pub notify_private: bool,
    pub notify_highlight: bool,
    pub notify_bell: bool,
    pub notify_osc: bool,
    pub notify_command: String,
    /// A sound file (WAV) to play on notifications.
    pub notify_sound: String,
    pub notify_only_unfocused: bool,
    pub notify_online: WatchNotify,

    // Behaviour
    pub send_typing: bool,
    pub idle_minutes: u32,
    /// Check the website for unread notes/messages every N minutes (0 = off).
    pub site_poll_minutes: u32,
    pub log_messages: bool,
    pub log_ads: bool,
    pub confirm_quit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            server: DEFAULT_SERVER.into(),
            api_base: DEFAULT_API.into(),
            auto_reconnect: true,
            rejoin_channels: true,
            reopen_private_chats: true,
            show_timestamps: true,
            timestamp_seconds: false,
            show_join_leave: true,
            show_status_changes: false,
            show_friend_online: true,
            show_ads: true,
            color_names_by_gender: true,
            show_user_list: true,
            show_online_friends: true,
            show_sidebar: true,
            sidebar_width: 24,
            user_list_width: 24,
            max_messages: 1000,
            load_history_lines: 30,
            truecolor: true,
            mouse: false,
            show_eicon_names: true,
            ascii_borders: false,
            gui_font: String::new(),
            gui_font_size: 14,
            gui_columns: 120,
            gui_rows: 45,
            gui_inline_images: true,
            gui_image_mode: ImageMode::Native,
            image_mode: ImageMode::Blocks,
            graphics_protocol: GraphicsProtocol::Auto,
            inline_images: false,
            inline_image_rows: 4,
            animate_images: true,
            highlight_own_name: true,
            highlight_words: Vec::new(),
            notify_private: true,
            notify_highlight: true,
            notify_bell: true,
            notify_osc: false,
            notify_command: String::new(),
            notify_sound: String::new(),
            notify_only_unfocused: false,
            notify_online: WatchNotify::Off,
            send_typing: true,
            idle_minutes: 15,
            site_poll_minutes: 5,
            log_messages: true,
            log_ads: false,
            confirm_quit: false,
        }
    }
}

/// Description of one setting, used by the settings dialog and `/set`.
pub struct SettingInfo {
    pub key: &'static str,
    pub help: &'static str,
}

pub const SETTINGS: &[SettingInfo] = &[
    SettingInfo { key: "server", help: "Chat server websocket URL" },
    SettingInfo { key: "api_base", help: "F-List website/API base URL" },
    SettingInfo { key: "auto_reconnect", help: "Reconnect automatically when the connection drops" },
    SettingInfo { key: "rejoin_channels", help: "Rejoin your channels after login" },
    SettingInfo { key: "reopen_private_chats", help: "Reopen private chat tabs after login" },
    SettingInfo { key: "show_timestamps", help: "Show message timestamps" },
    SettingInfo { key: "timestamp_seconds", help: "Include seconds in timestamps" },
    SettingInfo { key: "show_join_leave", help: "Show join/leave messages in channels" },
    SettingInfo { key: "show_status_changes", help: "Show status changes of friends/bookmarks in the console" },
    SettingInfo { key: "show_friend_online", help: "Show when friends/bookmarks come online or go offline" },
    SettingInfo { key: "show_ads", help: "Show roleplay ads in channels by default" },
    SettingInfo { key: "color_names_by_gender", help: "Colour character names by gender" },
    SettingInfo { key: "show_user_list", help: "Show the channel member list" },
    SettingInfo { key: "show_online_friends", help: "Show online friends and bookmarks in the console tab" },
    SettingInfo { key: "show_sidebar", help: "Show the conversation list" },
    SettingInfo { key: "sidebar_width", help: "Width of the conversation list" },
    SettingInfo { key: "user_list_width", help: "Width of the member list" },
    SettingInfo { key: "max_messages", help: "Messages kept in memory per tab" },
    SettingInfo { key: "load_history_lines", help: "Lines from the log to show when opening a tab (0 = off)" },
    SettingInfo { key: "truecolor", help: "Use 24-bit colour (turn off for 256-colour terminals)" },
    SettingInfo { key: "mouse", help: "Mouse support: click tabs, lists, key hints, scroll (hold Shift to select text in a terminal)" },
    SettingInfo { key: "show_eicon_names", help: "Show eicon names in text" },
    SettingInfo { key: "ascii_borders", help: "Draw borders with plain ASCII characters" },
    #[cfg(feature = "gui")]
    SettingInfo { key: "gui_font", help: "GUI version: font name (empty = Lucida Console or Courier New; after restart)" },
    #[cfg(feature = "gui")]
    SettingInfo { key: "gui_font_size", help: "GUI version: font height in pixels (after restart)" },
    #[cfg(feature = "gui")]
    SettingInfo { key: "gui_columns", help: "GUI version: window width in columns (remembered)" },
    #[cfg(feature = "gui")]
    SettingInfo { key: "gui_rows", help: "GUI version: window height in rows (remembered)" },
    #[cfg(feature = "gui")]
    SettingInfo { key: "gui_inline_images", help: "GUI version: show icons/eicons as images under messages" },
    #[cfg(feature = "gui")]
    SettingInfo {
        key: "gui_image_mode",
        help: "GUI version: image display: off, ascii, colorascii, blocks, braille, native (= real pixels)",
    },
    SettingInfo { key: "image_mode", help: "Image display: off, ascii, colorascii, blocks, braille, native" },
    SettingInfo { key: "graphics_protocol", help: "Native image protocol: auto, kitty, iterm2, sixel" },
    SettingInfo { key: "inline_images", help: "Show icons/eicons as small images under messages" },
    SettingInfo { key: "inline_image_rows", help: "Height of inline images in rows" },
    SettingInfo { key: "animate_images", help: "Play animated GIFs and eicons (not with native terminal graphics)" },
    SettingInfo { key: "highlight_own_name", help: "Highlight messages that mention your name" },
    SettingInfo { key: "highlight_words", help: "Extra highlight words, comma separated" },
    SettingInfo { key: "notify_private", help: "Notify on private messages" },
    SettingInfo { key: "notify_highlight", help: "Notify on highlights" },
    SettingInfo { key: "notify_bell", help: "Ring the terminal bell on notifications" },
    SettingInfo { key: "notify_osc", help: "Send desktop notifications via terminal escape codes (OSC 9/777)" },
    SettingInfo { key: "notify_command", help: "Run this command on notifications (%t = title, %m = message)" },
    SettingInfo { key: "notify_sound", help: "Play this sound file (WAV) on notifications (empty = off)" },
    SettingInfo { key: "notify_only_unfocused", help: "Only notify when the terminal is not focused" },
    SettingInfo { key: "notify_online", help: "Pop up at the top right when they come online/go offline: off, friends, bookmarks, both" },
    SettingInfo { key: "send_typing", help: "Tell others when you are typing in private chats" },
    SettingInfo { key: "idle_minutes", help: "Set status to idle after this many minutes (0 = off)" },
    SettingInfo { key: "site_poll_minutes", help: "Check the website for unread notes and site messages every N minutes (0 = off)" },
    SettingInfo { key: "log_messages", help: "Write chat logs to disk" },
    SettingInfo { key: "log_ads", help: "Include ads in logs" },
    SettingInfo { key: "confirm_quit", help: "Ask before quitting" },
];

impl Settings {
    /// The image mode in use: the window's own in the GUI version.
    pub fn active_image_mode(&self) -> ImageMode {
        if crate::images::pixel_output().is_some() {
            self.gui_image_mode
        } else {
            self.image_mode
        }
    }

    pub fn set_active_image_mode(&mut self, m: ImageMode) {
        if crate::images::pixel_output().is_some() {
            self.gui_image_mode = m;
        } else {
            self.image_mode = m;
        }
    }

    fn path() -> PathBuf {
        paths::config_dir().join("settings.toml")
    }

    pub fn load() -> Settings {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("settings.toml is invalid ({e}); using defaults");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        paths::write_private(&Self::path(), s.as_bytes())
    }

    fn as_table(&self) -> toml::Table {
        toml::Table::try_from(self).unwrap_or_default()
    }

    /// Current value of a setting as text.
    pub fn get(&self, key: &str) -> Option<String> {
        let t = self.as_table();
        t.get(key).map(|v| match v {
            toml::Value::String(s) => s.clone(),
            toml::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "),
            other => other.to_string(),
        })
    }

    /// Set a setting from text. Returns an error message on failure.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let mut t = self.as_table();
        let old = t.get(key).cloned().ok_or_else(|| format!("Unknown setting '{key}'"))?;
        let value = value.trim();
        let new = match old {
            toml::Value::Boolean(b) => toml::Value::Boolean(match value.to_ascii_lowercase().as_str() {
                "" | "toggle" => !b,
                "true" | "on" | "yes" | "1" => true,
                "false" | "off" | "no" | "0" => false,
                _ => return Err(format!("'{value}' is not on/off")),
            }),
            toml::Value::Integer(_) => toml::Value::Integer(value.parse().map_err(|_| format!("'{value}' is not a number"))?),
            toml::Value::Array(_) => toml::Value::Array(
                value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| toml::Value::String(s.into())).collect(),
            ),
            _ => toml::Value::String(value.into()),
        };
        t.insert(key.into(), new);
        let parsed: Settings = toml::Value::Table(t).try_into().map_err(|e| format!("Invalid value: {e}"))?;
        *self = parsed;
        Ok(())
    }

    pub fn is_bool(&self, key: &str) -> bool {
        matches!(self.as_table().get(key), Some(toml::Value::Boolean(_)))
    }
}

// ---------------------------------------------------------------------------
// Saved logins

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedAccount {
    pub account: String,
    /// Obfuscated password (not encryption!). Empty when not saved.
    pub password: String,
    pub last_character: String,
    pub auto_login: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Accounts {
    pub last_account: String,
    pub accounts: Vec<SavedAccount>,
}

const OBF_KEY: &[u8] = b"f-terminal:not-encryption:keep-this-file-private";

pub fn obfuscate(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    let bytes: Vec<u8> = plain.bytes().enumerate().map(|(i, b)| b ^ OBF_KEY[i % OBF_KEY.len()]).collect();
    format!("x1:{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn deobfuscate(stored: &str) -> String {
    let Some(rest) = stored.strip_prefix("x1:") else {
        return stored.to_string();
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(rest) else {
        return String::new();
    };
    let plain: Vec<u8> = bytes.iter().enumerate().map(|(i, b)| b ^ OBF_KEY[i % OBF_KEY.len()]).collect();
    String::from_utf8(plain).unwrap_or_default()
}

impl Accounts {
    fn path() -> PathBuf {
        paths::config_dir().join("accounts.toml")
    }

    pub fn load() -> Accounts {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        paths::write_private(&Self::path(), s.as_bytes())
    }

    pub fn find(&self, account: &str) -> Option<&SavedAccount> {
        self.accounts.iter().find(|a| a.account.eq_ignore_ascii_case(account))
    }

    pub fn upsert(&mut self, acc: SavedAccount) {
        self.last_account = acc.account.clone();
        if let Some(a) = self.accounts.iter_mut().find(|a| a.account.eq_ignore_ascii_case(&acc.account)) {
            *a = acc;
        } else {
            self.accounts.push(acc);
        }
    }

    pub fn remove(&mut self, account: &str) {
        self.accounts.retain(|a| !a.account.eq_ignore_ascii_case(account));
        if self.last_account.eq_ignore_ascii_case(account) {
            self.last_account.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Per-character state (channels to rejoin, open PMs, recent statuses, ...)

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterState {
    /// Channel ids to rejoin, with their titles.
    pub channels: Vec<(String, String)>,
    pub private_chats: Vec<String>,
    pub pinned: Vec<String>,
    pub recent_statuses: Vec<String>,
    pub last_status: String,
    pub last_status_message: String,
    /// Conversations with notifications muted (channel ids / character names).
    pub muted: Vec<String>,
    /// Conversations that should always notify.
    pub always_notify: Vec<String>,
    /// Draft roleplay ads.
    pub saved_ads: Vec<String>,
    /// Per-channel override for ad visibility.
    pub ads_hidden: Vec<String>,
}

impl CharacterState {
    fn path(character: &str) -> PathBuf {
        paths::config_dir().join("characters").join(format!("{}.toml", paths::sanitize(&character.to_lowercase())))
    }

    pub fn load(character: &str) -> CharacterState {
        std::fs::read_to_string(Self::path(character)).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, character: &str) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        paths::write_private(&Self::path(character), s.as_bytes())
    }

    pub fn push_recent_status(&mut self, msg: &str) {
        if msg.trim().is_empty() {
            return;
        }
        self.recent_statuses.retain(|s| s != msg);
        self.recent_statuses.insert(0, msg.to_string());
        self.recent_statuses.truncate(10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obfuscation_roundtrip() {
        let s = "hunter2 ÄÖÜ";
        assert_eq!(deobfuscate(&obfuscate(s)), s);
        assert_eq!(deobfuscate(""), "");
    }
    #[test]
    fn set_values() {
        let mut s = Settings::default();
        s.set("show_ads", "off").unwrap();
        assert!(!s.show_ads);
        s.set("image_mode", "ascii").unwrap();
        assert_eq!(s.image_mode, ImageMode::Ascii);
        assert!(s.set("image_mode", "bogus").is_err());
        s.set("highlight_words", "foo, bar").unwrap();
        assert_eq!(s.highlight_words, vec!["foo", "bar"]);
        s.set("notify_online", "bookmarks").unwrap();
        assert_eq!(s.notify_online, WatchNotify::Bookmarks);
        assert!(s.set("notify_online", "nobody").is_err());
        s.set("idle_minutes", "5").unwrap();
        assert_eq!(s.idle_minutes, 5);
    }
}
