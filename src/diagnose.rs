//! `--diagnose`: check each step of connecting to F-List on its own and print
//! the result, to find out why logging in fails on a particular system.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

fn step<T, E: std::fmt::Debug>(name: &str, r: Result<T, E>) -> Option<T> {
    match r {
        Ok(v) => {
            println!("[ok]   {name}");
            Some(v)
        }
        Err(e) => {
            println!("[FAIL] {name}: {e:?}");
            None
        }
    }
}

pub fn run() {
    println!("F-Term {} connection check (TLS backend: {})", env!("CARGO_PKG_VERSION"), crate::tls::backend_name());
    println!();

    // Certificates are only valid within a time range, so the clock must be right.
    println!("[info] system clock (UTC): {}", crate::timeutil::fmt_datetime(crate::timeutil::now()));

    let mut buf = [0u8; 32];
    step("random numbers for TLS", crate::tls::fill_random(&mut buf));

    let host = crate::config::DEFAULT_API.trim_start_matches("https://").trim_end_matches('/');
    let Some(addrs) = step(&format!("DNS lookup of {host}"), (host, 443).to_socket_addrs()) else {
        return;
    };
    let addrs: Vec<_> = addrs.collect();
    println!("[info] addresses: {addrs:?}");

    let mut sock = None;
    for a in &addrs {
        if let Some(s) = step(&format!("TCP connect to {a}"), TcpStream::connect_timeout(a, Duration::from_secs(15))) {
            sock = Some(s);
            break;
        }
    }
    let Some(sock) = sock else { return };
    let _ = sock.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(30)));

    let Some(mut tls) = step("TLS setup", crate::tls::wrap(host, sock)) else { return };
    let req = format!("HEAD / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if step("TLS handshake and HTTPS request", tls.write_all(req.as_bytes()).and_then(|_| tls.flush())).is_none() {
        return;
    }
    let mut reply = Vec::new();
    let mut chunk = [0u8; 512];
    let r = loop {
        match tls.read(&mut chunk) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                reply.extend_from_slice(&chunk[..n]);
                if reply.len() > 200 {
                    break Ok(());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    if step("HTTPS reply", r).is_some() {
        let first = String::from_utf8_lossy(&reply);
        println!("[info] reply: {}", first.lines().next().unwrap_or(""));
    }

    println!();
    println!("All steps passed if no line says FAIL.");
}
