//! BBCode parsing for F-Chat messages and profiles.
//!
//! Produces a flat list of styled segments that the UI turns into ratatui spans.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Url(String),
    User(String),
    Session { title: String, id: String },
    Channel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Embed {
    /// Character avatar.
    Icon(String),
    /// Custom emoticon.
    Eicon(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seg {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<Color>,
    pub spoiler: bool,
    pub small: bool,
    pub big: bool,
    pub heading: bool,
    pub quote: bool,
    pub link: Option<Link>,
    pub embed: Option<Embed>,
    pub hr: bool,
}

pub fn color_by_name(name: &str) -> Option<Color> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "red" => Color::Rgb(0xff, 0x40, 0x40),
        "blue" => Color::Rgb(0x60, 0x80, 0xff),
        "white" => Color::Rgb(0xff, 0xff, 0xff),
        "yellow" => Color::Rgb(0xff, 0xff, 0x40),
        "pink" => Color::Rgb(0xff, 0xa0, 0xc8),
        "gray" | "grey" => Color::Rgb(0xa0, 0xa0, 0xa0),
        "green" => Color::Rgb(0x40, 0xe0, 0x40),
        "orange" => Color::Rgb(0xff, 0xa5, 0x20),
        "purple" => Color::Rgb(0xc0, 0x60, 0xff),
        "black" => Color::Rgb(0x50, 0x50, 0x50),
        "brown" => Color::Rgb(0xb0, 0x70, 0x30),
        "cyan" => Color::Rgb(0x40, 0xe0, 0xe0),
        _ => return None,
    })
}

const KNOWN: &[&str] = &[
    "b", "i", "u", "s", "color", "url", "user", "icon", "eicon", "noparse", "sub", "sup", "spoiler", "session", "channel", "big", "small",
    "heading", "indent", "collapse", "hr", "quote", "left", "right", "center", "justify", "img",
];

#[derive(Debug, Clone)]
struct Open {
    tag: String,
    arg: String,
    start_seg: usize,
    start_text: String,
}

/// Parse one tag token at `s` (which starts with '['). Returns (closing, name, arg, len).
fn parse_tag(s: &str) -> Option<(bool, String, String, usize)> {
    let end = s.find(']')?;
    let inner = &s[1..end];
    if inner.is_empty() || inner.len() > 300 || inner.contains('[') {
        return None;
    }
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, inner),
    };
    let (name, arg) = match inner.find('=') {
        Some(i) if !closing => (&inner[..i], &inner[i + 1..]),
        _ => (inner, ""),
    };
    let name = name.trim().to_ascii_lowercase();
    if !KNOWN.contains(&name.as_str()) {
        return None;
    }
    Some((closing, name, arg.to_string(), end + 1))
}

/// Parse BBCode into styled segments. Unknown or unbalanced tags are kept as text.
pub fn parse(input: &str) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    let bytes = input.as_bytes();

    // Style for new text given the current open tags.
    fn style_from(stack: &[Open]) -> Seg {
        let mut s = Seg::default();
        for o in stack {
            match o.tag.as_str() {
                "b" => s.bold = true,
                "i" => s.italic = true,
                "u" => s.underline = true,
                "s" => s.strike = true,
                "color" => s.color = color_by_name(&o.arg),
                "spoiler" => s.spoiler = true,
                "small" | "sub" | "sup" => s.small = true,
                "big" => s.big = true,
                "heading" => s.heading = true,
                "quote" => s.quote = true,
                _ => {}
            }
        }
        s
    }

    let flush = |cur: &mut String, segs: &mut Vec<Seg>, stack: &[Open]| {
        if !cur.is_empty() {
            let mut seg = style_from(stack);
            seg.text = std::mem::take(cur);
            segs.push(seg);
        }
    };

    while i < input.len() {
        if bytes[i] == b'[' {
            if let Some((closing, name, arg, len)) = parse_tag(&input[i..]) {
                let raw = &input[i..i + len];
                let in_noparse = stack.last().map(|o| o.tag == "noparse").unwrap_or(false);
                if in_noparse && !(closing && name == "noparse") {
                    cur.push_str(raw);
                    i += len;
                    continue;
                }
                if !closing {
                    if name == "hr" {
                        flush(&mut cur, &mut segs, &stack);
                        segs.push(Seg { text: "\n".into(), hr: true, ..Default::default() });
                        i += len;
                        continue;
                    }
                    flush(&mut cur, &mut segs, &stack);
                    stack.push(Open { tag: name.clone(), arg: arg.clone(), start_seg: segs.len(), start_text: raw.to_string() });
                    if name == "collapse" {
                        // Show the collapse title as a heading line.
                        let title = if arg.is_empty() { "Collapse".to_string() } else { arg };
                        segs.push(Seg { text: format!("▼ {title}\n"), bold: true, ..Default::default() });
                    }
                    if name == "indent" {
                        cur.push_str("    ");
                    }
                    i += len;
                    continue;
                }
                // Closing tag: find matching open tag.
                if let Some(pos) = stack.iter().rposition(|o| o.tag == name) {
                    flush(&mut cur, &mut segs, &stack);
                    // Implicitly close anything opened after it.
                    let open = stack.remove(pos);
                    stack.truncate(pos);
                    finish_tag(&open, &mut segs);
                    i += len;
                    continue;
                }
                cur.push_str(raw);
                i += len;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap();
        cur.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut cur, &mut segs, &stack);
    // Unclosed tags just apply to the rest of the text (like the web client),
    // except link-like ones which are shown raw.
    for open in stack.into_iter().rev() {
        if matches!(open.tag.as_str(), "url" | "user" | "icon" | "eicon" | "session" | "channel") {
            segs.insert(open.start_seg, Seg { text: open.start_text, ..Default::default() });
        }
    }
    segs
}

/// Apply semantics of a closed tag to the segments it wrapped.
fn finish_tag(open: &Open, segs: &mut Vec<Seg>) {
    let inner: String = segs[open.start_seg..].iter().map(|s| s.text.as_str()).collect();
    match open.tag.as_str() {
        "url" => {
            let url = if open.arg.trim().is_empty() { inner.trim().to_string() } else { open.arg.trim().to_string() };
            let label = if open.arg.trim().is_empty() { url.clone() } else { inner.clone() };
            let host = url::Url::parse(&url).ok().and_then(|u| u.host_str().map(String::from));
            let base = segs.get(open.start_seg).cloned().unwrap_or_default();
            segs.truncate(open.start_seg);
            let mut text = label;
            if !open.arg.trim().is_empty() {
                if let Some(h) = host {
                    text = format!("{text} [{h}]");
                }
            }
            segs.push(Seg { text, underline: true, link: Some(Link::Url(url)), ..base });
        }
        "user" | "icon" | "eicon" | "channel" => {
            let name = inner.trim().to_string();
            let base = segs.get(open.start_seg).cloned().unwrap_or_default();
            segs.truncate(open.start_seg);
            let seg = match open.tag.as_str() {
                "user" => Seg { text: name.clone(), bold: true, link: Some(Link::User(name)), ..base },
                "icon" => Seg {
                    text: format!("[{name}]"),
                    bold: true,
                    link: Some(Link::User(name.clone())),
                    embed: Some(Embed::Icon(name)),
                    ..base
                },
                "eicon" => Seg { text: format!(":{name}:"), embed: Some(Embed::Eicon(name)), ..base },
                _ => Seg { text: format!("#{name}"), underline: true, link: Some(Link::Channel(name)), ..base },
            };
            segs.push(seg);
        }
        "session" => {
            let id = inner.trim().to_string();
            let title = if open.arg.is_empty() { id.clone() } else { open.arg.clone() };
            let base = segs.get(open.start_seg).cloned().unwrap_or_default();
            segs.truncate(open.start_seg);
            segs.push(Seg { text: format!("#{title}"), underline: true, link: Some(Link::Session { title, id }), ..base });
        }
        "sup" => {
            if let Some(first) = segs.get_mut(open.start_seg) {
                first.text.insert(0, '^');
            }
        }
        "collapse" | "quote" | "heading" => {
            segs.push(Seg { text: "\n".into(), ..Default::default() });
        }
        _ => {}
    }
}

/// Strip all BBCode, leaving plain text.
pub fn strip(input: &str) -> String {
    parse(input).into_iter().map(|s| s.text).collect()
}

/// All links in a message, in order.
pub fn links(input: &str) -> Vec<Link> {
    let mut out: Vec<Link> = parse(input).into_iter().filter_map(|s| s.link).collect();
    // Bare URLs in plain text.
    for word in strip(input).split_whitespace() {
        let w = word.trim_matches(|c: char| matches!(c, '(' | ')' | '<' | '>' | '"' | '\'' | ',' | '.'));
        if (w.starts_with("http://") || w.starts_with("https://")) && !out.contains(&Link::Url(w.to_string())) {
            out.push(Link::Url(w.to_string()));
        }
    }
    out
}

pub fn embeds(input: &str) -> Vec<Embed> {
    parse(input).into_iter().filter_map(|s| s.embed).collect()
}

pub fn seg_style(s: &Seg, base: Style) -> Style {
    let mut st = base;
    if let Some(c) = s.color {
        st = st.fg(c);
    }
    let mut m = Modifier::empty();
    if s.bold || s.heading || s.big {
        m |= Modifier::BOLD;
    }
    if s.italic || s.quote {
        m |= Modifier::ITALIC;
    }
    if s.underline || s.heading {
        m |= Modifier::UNDERLINED;
    }
    if s.strike {
        m |= Modifier::CROSSED_OUT;
    }
    if s.small {
        m |= Modifier::DIM;
    }
    st = st.add_modifier(m);
    if s.spoiler {
        st = st.fg(Color::DarkGray).bg(Color::DarkGray);
    }
    if matches!(s.link, Some(Link::Url(_)) | Some(Link::Session { .. }) | Some(Link::Channel(_))) {
        st = st.fg(Color::LightBlue);
    }
    if matches!(s.embed, Some(Embed::Eicon(_))) {
        st = st.fg(Color::LightYellow);
    }
    st
}

/// Turn BBCode into styled spans, split into lines on '\n'.
/// `user_style` lets the caller colour [user] / [icon] names.
pub fn to_lines(input: &str, base: Style, user_style: &dyn Fn(&str) -> Style, show_eicon_names: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::default()];
    for seg in parse(input) {
        if seg.hr {
            lines.push(Line::from(Span::styled("─".repeat(30), base.fg(Color::DarkGray))));
            lines.push(Line::default());
            continue;
        }
        let mut st = seg_style(&seg, base);
        if let Some(Link::User(name)) = &seg.link {
            st = user_style(name).add_modifier(st.add_modifier.intersection(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED));
        }
        let text = match &seg.embed {
            Some(Embed::Eicon(_)) if !show_eicon_names => "◆".to_string(),
            _ => seg.text.clone(),
        };
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            if !first.is_empty() {
                lines.last_mut().unwrap().spans.push(Span::styled(first.replace('\t', "    "), st));
            }
        }
        for part in parts {
            let mut l = Line::default();
            if seg.quote {
                l.spans.push(Span::styled("│ ", base.fg(Color::DarkGray)));
            }
            if !part.is_empty() {
                l.spans.push(Span::styled(part.replace('\t', "    "), st));
            }
            lines.push(l);
        }
    }
    // Drop a trailing empty line created by closing block tags.
    while lines.len() > 1 && lines.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let s = parse("hi [b]bold [i]both[/i][/b] end");
        assert_eq!(s.len(), 4);
        assert!(s[1].bold && !s[1].italic);
        assert!(s[2].bold && s[2].italic);
        assert_eq!(strip("[color=red]x[/color]"), "x");
    }

    #[test]
    fn unknown_and_unbalanced() {
        assert_eq!(strip("[foo]x[/foo]"), "[foo]x[/foo]");
        assert_eq!(strip("a [/b] c"), "a [/b] c");
        assert_eq!(strip("[noparse][b]x[/b][/noparse]"), "[b]x[/b]");
    }

    #[test]
    fn links_and_embeds() {
        let l = links("[url=https://example.com/a]Example[/url] and [user]Bob[/user] see https://x.org/");
        assert_eq!(l[0], Link::Url("https://example.com/a".into()));
        assert_eq!(l[1], Link::User("Bob".into()));
        assert_eq!(l[2], Link::Url("https://x.org/".into()));
        let e = embeds("[eicon]wave[/eicon][icon]Bob[/icon]");
        assert_eq!(e, vec![Embed::Eicon("wave".into()), Embed::Icon("Bob".into())]);
        let s = links("[session=My Room]ADH-123[/session]");
        assert_eq!(s[0], Link::Session { title: "My Room".into(), id: "ADH-123".into() });
    }
}
