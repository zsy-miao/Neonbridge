//! A small HTTP/1.1 server on 127.0.0.1: one thread per connection, every
//! response closes its connection. tsproxy and cloudflared reach it locally.
use crate::auth;
use crate::encoder::{Encoder, Kind};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    loopback: bool,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    /// Straight from this PC: tsproxy marks its requests (Tailscale-User-Login,
    /// X-Forwarded-For) and cloudflared stamps Cf-* headers.
    fn local(&self) -> bool {
        self.loopback
            && ["Tailscale-User-Login", "X-Forwarded-For", "Cf-Connecting-Ip", "Cf-Ray", "Forwarded"].iter().all(|h| self.header(h).is_none())
    }
    fn secure(&self) -> bool {
        self.header("X-Forwarded-Proto") == Some("https")
    }
}

pub(crate) fn serve(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    std::thread::Builder::new().name("feed http".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = handle(stream);
            });
        }
    })?;
    Ok(())
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let loopback = stream.peer_addr().is_ok_and(|a| a.ip().is_loopback());
    stream.set_read_timeout(Some(std::time::Duration::from_secs(15))).ok()?;
    let mut reader = BufReader::new(stream.take(64 * 1024));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
        if headers.len() > 100 {
            return None;
        }
    }
    let mut request = Request { method, path, headers, body: Vec::new(), loopback };
    let length: usize = request.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if length > 4096 {
        return None;
    }
    request.body.resize(length, 0);
    reader.read_exact(&mut request.body).ok()?;
    Some(request)
}

fn reply(stream: &mut TcpStream, code: u16, ctype: &str, body: &[u8], extra: &[String]) -> std::io::Result<()> {
    let reason = match code { 200 => "OK", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 429 => "Too Many Requests", 503 => "Service Unavailable", _ => "Error" };
    let mut head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{}",
        body.len(), SECURITY);
    for h in extra {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

const SECURITY: &str = "X-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n";

fn error(stream: &mut TcpStream, code: u16, message: &str) -> std::io::Result<()> {
    let body = format!("{{\"error\":\"{}\"}}", message.replace('"', "'"));
    reply(stream, code, "application/json", body.as_bytes(), &[])
}

fn form_value(body: &[u8], key: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let raw = text.split('&').find_map(|p| p.strip_prefix(key)?.strip_prefix('='))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let Some(request) = read_request(&stream) else { return Ok(()) };
    let route = request.path.split('?').next().unwrap_or("/").to_owned();
    if request.method == "POST" && route == "/auth" {
        let password = form_value(&request.body, "password").unwrap_or_default();
        return match auth::login(&password, request.local()) {
            auth::Login::Ok(token) => reply(&mut stream, 200, "application/json", b"{\"ok\":true}",
                &[format!("Set-Cookie: {}", auth::cookie(&token, request.secure()))]),
            auth::Login::Refused(code, message) => error(&mut stream, code, message),
        };
    }
    if request.method != "GET" && request.method != "HEAD" {
        return error(&mut stream, 400, "bad request");
    }
    if route == "/manifest.json" {
        return reply(&mut stream, 200, "application/manifest+json", crate::page::MANIFEST.as_bytes(), &[]);
    }
    if !auth::authed(request.header("Cookie")) {
        if route == "/" {
            let setup = !auth::password_set();
            let page = crate::page::LOGIN.replace("__SETUP__", if setup { "true" } else { "false" })
                .replace("__LOCAL__", if request.local() { "true" } else { "false" });
            return reply(&mut stream, 200, "text/html; charset=utf-8", page.as_bytes(), &[]);
        }
        return error(&mut stream, 401, "login required");
    }
    match route.as_str() {
        "/" => reply(&mut stream, 200, "text/html; charset=utf-8", crate::page::PAGE.as_bytes(), &[]),
        "/status" => {
            let body = format!("{{\"frames\":{},\"viewers\":{}}}", crate::frames::published(), crate::encoder::viewers());
            reply(&mut stream, 200, "application/json", body.as_bytes(), &[])
        }
        "/live.mp4" => stream_mp4(&mut stream),
        "/live.mjpg" => stream_mjpeg(&mut stream),
        _ => error(&mut stream, 404, "not found"),
    }
}

fn stream_mp4(stream: &mut TcpStream) -> std::io::Result<()> {
    let encoder = Encoder::get(Kind::Mp4);
    let Some((id, init, codec)) = encoder.subscribe() else {
        return error(stream, 503, "no video yet: is the game running and presenting?");
    };
    let result = (|| -> std::io::Result<()> {
        stream.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nX-Codec: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{SECURITY}\r\n",
            codec.unwrap_or_default()).as_bytes())?;
        if let Some(init) = init {
            stream.write_all(&init)?;
        }
        while let Some(frags) = encoder.next(id) {
            for f in frags {
                stream.write_all(&f)?;
            }
        }
        Ok(())
    })();
    encoder.unsubscribe(id);
    result
}

fn stream_mjpeg(stream: &mut TcpStream) -> std::io::Result<()> {
    let encoder = Encoder::get(Kind::Jpeg);
    let Some((id, _, _)) = encoder.subscribe() else {
        return error(stream, 503, "no video yet");
    };
    let result = (|| -> std::io::Result<()> {
        stream.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-store\r\nConnection: close\r\n{SECURITY}\r\n").as_bytes())?;
        while let Some(frames) = encoder.next(id) {
            if let Some(jpeg) = frames.last() {
                stream.write_all(format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", jpeg.len()).as_bytes())?;
                stream.write_all(jpeg)?;
                stream.write_all(b"\r\n")?;
            }
        }
        Ok(())
    })();
    encoder.unsubscribe(id);
    result
}
