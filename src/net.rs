//! Websocket connection to the chat server, running on its own thread.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use tungstenite::Message;

use crate::protocol::{self, ServerCmd};

#[derive(Debug)]
pub enum NetEvent {
    Connected,
    Command(ServerCmd),
    /// Raw line that failed to parse.
    BadLine(String, String),
    Disconnected(String),
}

/// Handle used by the app to talk to the connection thread.
pub struct Connection {
    out: Sender<String>,
}

impl Connection {
    pub fn send(&self, line: String) {
        let _ = self.out.send(line);
    }
    pub fn close(&self) {
        let _ = self.out.send(String::new());
    }
}

/// Adapter that turns read timeouts into WouldBlock, which tungstenite handles
/// as "no data yet" (Windows reports timeouts as TimedOut instead).
struct Sock(TcpStream);

impl Read for Sock {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.kind() == ErrorKind::TimedOut => Err(ErrorKind::WouldBlock.into()),
            r => r,
        }
    }
}

impl Write for Sock {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

enum Stream {
    Plain(Sock),
    Tls(Box<crate::tls::TlsStream<Sock>>),
}

impl Stream {
    fn sock(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => &s.0,
            Stream::Tls(t) => &t.sock.0,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// Start connecting. `idn` is the identify command, sent right after the handshake.
/// Events are passed through `wrap` so the app can tag them with the connection id.
pub fn connect<E: Send + 'static>(url: String, idn: String, id: u64, events: Sender<E>, wrap: fn(u64, NetEvent) -> E) -> Connection {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("net".into())
        .spawn(move || {
            let send = |ev: NetEvent| events.send(wrap(id, ev)).is_ok();
            let reason = match run(&url, &idn, &rx, &send) {
                Ok(()) => "Disconnected.".to_string(),
                Err(e) => e,
            };
            send(NetEvent::Disconnected(reason));
        })
        .expect("spawn net thread");
    Connection { out: tx }
}

fn open(url: &str) -> Result<tungstenite::WebSocket<Stream>, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("Bad server URL: {e}"))?;
    let secure = match parsed.scheme() {
        "wss" => true,
        "ws" => false,
        s => return Err(format!("Unsupported URL scheme '{s}'")),
    };
    let host = parsed.host_str().ok_or("Server URL has no host")?.to_string();
    let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });

    let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("Cannot resolve {host}: {e}"))?.collect();
    let mut last_err = String::from("no addresses");
    let mut tcp = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(15)) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let tcp = tcp.ok_or_else(|| format!("Cannot connect to {host}:{port}: {last_err}"))?;
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(Duration::from_secs(20))).ok();

    let stream = if secure {
        Stream::Tls(Box::new(crate::tls::wrap(&host, Sock(tcp)).map_err(|e| format!("TLS error: {e}"))?))
    } else {
        Stream::Plain(Sock(tcp))
    };

    let (ws, _resp) = tungstenite::client(url, stream).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => format!("Websocket handshake failed: {e}"),
        tungstenite::HandshakeError::Interrupted(_) => "Websocket handshake timed out".to_string(),
    })?;
    Ok(ws)
}

fn run(url: &str, idn: &str, rx: &Receiver<String>, send: &dyn Fn(NetEvent) -> bool) -> Result<(), String> {
    let mut ws = open(url)?;
    ws.get_ref().sock().set_read_timeout(Some(Duration::from_millis(40))).ok();
    ws.send(Message::Text(idn.to_string())).map_err(|e| format!("Send failed: {e}"))?;
    if !send(NetEvent::Connected) {
        return Ok(());
    }

    loop {
        // Outgoing commands.
        loop {
            match rx.try_recv() {
                Ok(line) if line.is_empty() => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return Ok(());
                }
                Ok(line) => {
                    log::debug(&format!(">> {line}"));
                    ws.send(Message::Text(line)).map_err(|e| format!("Send failed: {e}"))?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Incoming.
        match ws.read() {
            Ok(Message::Text(text)) => {
                if text.starts_with("PIN") {
                    ws.send(Message::Text(protocol::pin())).map_err(|e| format!("Send failed: {e}"))?;
                    continue;
                }
                log::debug(&format!("<< {}", truncate(&text, 400)));
                let ev = match protocol::parse(&text) {
                    Ok(cmd) => NetEvent::Command(cmd),
                    Err(e) => NetEvent::BadLine(text.to_string(), e),
                };
                if !send(ev) {
                    return Ok(());
                }
            }
            Ok(Message::Close(frame)) => {
                return Err(match frame {
                    Some(f) if !f.reason.is_empty() => format!("Server closed the connection: {}", f.reason),
                    _ => "Server closed the connection.".into(),
                })
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => return Err("Connection closed.".into()),
            Err(e) => return Err(format!("Connection error: {e}")),
        }
    }
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Tiny debug logger, enabled with the `--debug` flag.
pub mod log {
    use std::io::Write;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

    pub fn enable(path: &std::path::Path) {
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = FILE.set(Mutex::new(f));
        }
    }

    pub fn debug(msg: &str) {
        if let Some(f) = FILE.get() {
            if let Ok(mut f) = f.lock() {
                let _ = writeln!(f, "[{}] {}", crate::timeutil::fmt_datetime(crate::timeutil::now()), msg);
            }
        }
    }
}
