//! The desktop's side of remote control on the LAN (NativeTerm's R1):
//! one port, serving the web client's files over HTTP and devices over a
//! WebSocket, every frame sealed by the Noise channel of
//! `native_term_remote::noise`. Read-only in R1: input is not taken.
//!
//! NativeTerm owns what is decided (`config.nativeterm_remote_dir`, a
//! folder of NativeTerm's data):
//! - `desktop.key`: the desktop's key pair (64 bytes, private then public),
//!   made by NativeTerm, only the person may read it;
//! - `state.json`: the port, the panes open to remote control, the pairing
//!   asked for (a one-time secret and when it expires), the web client's
//!   folder; read again whenever it changes;
//! - `devices/<public key>.json`: written here when a device pairs;
//!   NativeTerm lists them, and a device whose file is gone is refused.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use mux::pane::PaneId;
use mux::{Mux, MuxNotification};
use native_term_remote::noise::{Channel, Handshake, Keys};
use native_term_remote::proto::{
    ended, frame::Body, Ended, Frame, Heartbeat, SessionInfo, Sessions,
};
use native_term_remote::screen::Since;
use native_term_remote::wire;
use serde::{Deserialize, Serialize};
use tungstenite::{Message, WebSocket};

use crate::{PaneMirror, PaneSource};

/// How often a connection looks for changes, and sends a heartbeat.
const TICK: Duration = Duration::from_millis(40);
const HEARTBEAT: Duration = Duration::from_secs(30);
/// A connection that says nothing for this long is gone.
const SILENCE: Duration = Duration::from_secs(90);

/// What NativeTerm decided, as `state.json` has it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct State {
    #[serde(default)]
    pub port: u16,
    /// Pane ids open to remote control.
    #[serde(default)]
    pub sessions: Vec<PaneId>,
    #[serde(default)]
    pub pairing: Option<Pairing>,
    /// The web client's files.
    #[serde(default)]
    pub web: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Pairing {
    /// The one-time secret, in hex (64 digits).
    pub secret: String,
    /// When it expires (seconds since the epoch).
    pub expires: u64,
}

/// What is written for a paired device.
#[derive(Serialize)]
struct Paired<'a> {
    name: &'a str,
    paired: u64,
}

struct Shared {
    dir: PathBuf,
    keys: Keys,
    /// Bumped on every output of a pane: connections look again.
    generation: AtomicU64,
    mirrors: Mutex<HashMap<PaneId, PaneMirror>>,
    state: Mutex<(Option<SystemTime>, State)>,
}

impl Shared {
    /// `state.json` as it is now (read again when it changed).
    fn state(&self) -> State {
        let path = self.dir.join("state.json");
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let mut cached = self.state.lock().unwrap();
        if cached.0 != modified {
            let read = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            *cached = (modified, read.unwrap_or_default());
        }
        cached.1.clone()
    }

    fn paired(&self, key: &[u8; 32]) -> bool {
        self.dir
            .join("devices")
            .join(format!("{}.json", hex(key)))
            .is_file()
    }

    /// The pane's mirror brought up to date; what a device that applied
    /// `from` needs; `None` when the pane is gone.
    fn since(&self, pane: PaneId, from: u64) -> Option<Since> {
        let pane_ref = Mux::try_get()?.get_pane(pane)?;
        let mut mirrors = self.mirrors.lock().unwrap();
        let mirror = mirrors.entry(pane).or_default();
        mirror.refresh(&mut PaneSource(&*pane_ref));
        Some(mirror.mirror().since(&pane.to_string(), from))
    }
}

/// Start serving, when NativeTerm asked for it (`dir` holds its state).
/// Returns at once; the work is on threads of its own.
pub fn start(dir: PathBuf) {
    std::thread::Builder::new()
        .name("nativeterm-remote".into())
        .spawn(move || {
            if let Err(e) = serve(dir) {
                log::error!("nativeterm remote control: {e:#}");
            }
        })
        .ok();
}

fn serve(dir: PathBuf) -> anyhow::Result<()> {
    let keys = read_keys(&dir.join("desktop.key"))?;
    let shared = Arc::new(Shared {
        dir,
        keys,
        generation: AtomicU64::new(0),
        mirrors: Mutex::new(HashMap::new()),
        state: Mutex::new((None, State::default())),
    });
    // NativeTerm writes the port first; wait for it
    let port = loop {
        let port = shared.state().port;
        if port != 0 {
            break port;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    let watcher = Arc::clone(&shared);
    if let Some(mux) = Mux::try_get() {
        mux.subscribe(move |n| {
            if let MuxNotification::PaneOutput(_) | MuxNotification::PaneRemoved(_) = n {
                watcher.generation.fetch_add(1, Ordering::Relaxed);
            }
            true
        });
    }
    // both families: a dual-stack socket where the system makes one, else
    // the IPv4 one beside it
    let mut listeners = Vec::new();
    for address in [format!("[::]:{port}"), format!("0.0.0.0:{port}")] {
        match TcpListener::bind(&address) {
            Ok(l) => listeners.push(l),
            Err(e) => log::debug!("nativeterm remote control: {address}: {e}"),
        }
    }
    anyhow::ensure!(!listeners.is_empty(), "nothing could listen on port {port}");
    let mut threads = Vec::new();
    for listener in listeners {
        let shared = Arc::clone(&shared);
        threads.push(std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    if let Err(e) = connection(&shared, stream) {
                        log::debug!("nativeterm remote control: {e:#}");
                    }
                });
            }
        }));
    }
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}

fn read_keys(path: &Path) -> anyhow::Result<Keys> {
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(bytes.len() == 64, "{} is not a key pair", path.display());
    let private: [u8; 32] = bytes[..32].try_into()?;
    let public: [u8; 32] = bytes[32..].try_into()?;
    Ok(Keys::from_private(private, public))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    text.len().is_multiple_of(2)
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
                .collect()
        })
        .flatten()
}

/// One connection: a file of the web client, or a device.
fn connection(shared: &Shared, stream: TcpStream) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let (path, websocket) = peek_request(&stream)?;
    if !websocket {
        return serve_file(shared, stream, &path);
    }
    let pairing = match path.as_str() {
        "/ws/pair" => true,
        "/ws/resume" => false,
        _ => anyhow::bail!("no such place: {path}"),
    };
    let mut ws = tungstenite::accept(stream)?;
    let (channel, device) = handshake(shared, &mut ws, pairing)?;
    ws.get_ref().set_read_timeout(Some(TICK))?;
    session(shared, ws, channel, device, pairing)
}

/// The request's path and whether it asks for a WebSocket, read without
/// taking it from the stream (tungstenite reads it again).
fn peek_request(stream: &TcpStream) -> anyhow::Result<(String, bool)> {
    let mut buf = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let n = stream.peek(&mut buf)?;
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        if let httparse::Status::Complete(_) = request.parse(&buf[..n])? {
            let path = request.path.unwrap_or("/").to_string();
            let websocket = request.headers.iter().any(|h| {
                h.name.eq_ignore_ascii_case("upgrade") && h.value.eq_ignore_ascii_case(b"websocket")
            });
            return Ok((path, websocket));
        }
        anyhow::ensure!(n < buf.len() && Instant::now() < deadline, "no request");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A file of the web client (nothing outside its folder).
fn serve_file(shared: &Shared, mut stream: TcpStream, path: &str) -> anyhow::Result<()> {
    // take the request off the stream
    let mut buf = [0u8; 8192];
    let _ = stream.read(&mut buf);
    let file = shared.state().web.and_then(|root| web_file(&root, path));
    let (status, kind, body) = match file.and_then(|f| std::fs::read(&f).ok().map(|b| (f, b))) {
        Some((f, body)) => ("200 OK", content_type(&f), body),
        None => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found".to_vec(),
        ),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    Ok(())
}

/// The file of the web client a request's path names; `None` for a path
/// that would leave its folder.
fn web_file(root: &Path, path: &str) -> Option<PathBuf> {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let relative = path.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let ok = relative
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':']));
    ok.then(|| root.join(relative))
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ttf" | "otf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

type Ws = WebSocket<TcpStream>;

fn binary(ws: &mut Ws) -> anyhow::Result<Vec<u8>> {
    loop {
        match ws.read()? {
            Message::Binary(b) => return Ok(b.to_vec()),
            Message::Close(_) => anyhow::bail!("closed"),
            _ => {}
        }
    }
}

/// Pairing (with the one-time secret NativeTerm put in the QR code) or
/// resuming (a paired device); the channel and the device's key.
fn handshake(shared: &Shared, ws: &mut Ws, pairing: bool) -> anyhow::Result<(Channel, [u8; 32])> {
    let mut hs = if pairing {
        let state = shared.state();
        let pairing = state
            .pairing
            .ok_or_else(|| anyhow::anyhow!("no pairing asked for"))?;
        anyhow::ensure!(pairing.expires > now(), "the pairing expired");
        let secret: [u8; 32] = unhex(&pairing.secret)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| anyhow::anyhow!("the pairing's secret is not one"))?;
        Handshake::pair_desktop(&shared.keys, &secret)?
    } else {
        Handshake::resume_desktop(&shared.keys)?
    };
    while !hs.finished() {
        if hs.my_turn() {
            ws.send(Message::Binary(hs.write()?.into()))?;
        } else {
            hs.read(&binary(ws)?)?;
            // resuming: who it is is known after the first message
            if let (false, Some(key)) = (pairing, hs.remote_key()) {
                anyhow::ensure!(shared.paired(&key), "not a paired device");
            }
        }
    }
    let (channel, device) = hs.channel()?;
    if pairing {
        record(shared, &device, "")?;
    }
    Ok((channel, device))
}

/// A paired device, for NativeTerm's list.
fn record(shared: &Shared, device: &[u8; 32], name: &str) -> anyhow::Result<()> {
    let dir = shared.dir.join("devices");
    std::fs::create_dir_all(&dir)?;
    let paired = serde_json::to_vec(&Paired {
        name,
        paired: now(),
    })?;
    std::fs::write(dir.join(format!("{}.json", hex(device))), paired)?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn send(ws: &mut Ws, channel: &mut Channel, frame: &Frame) -> anyhow::Result<()> {
    for message in channel.seal(&wire::encode(frame))? {
        ws.send(Message::Binary(message.into()))?;
    }
    Ok(())
}

fn sessions(state: &State) -> Frame {
    let mux = Mux::try_get();
    let sessions = state
        .sessions
        .iter()
        .filter_map(|id| {
            let pane = mux.as_ref()?.get_pane(*id)?;
            let dims = pane.get_dimensions();
            Some(SessionInfo {
                id: id.to_string(),
                title: pane.get_title(),
                cols: dims.cols as u32,
                lines: dims.viewport_rows as u32,
                read_only: true,
            })
        })
        .collect();
    wire::frame(Body::Sessions(Sessions { sessions }))
}

/// A device after the handshake: what it asks, and what changes.
fn session(
    shared: &Shared,
    mut ws: Ws,
    mut channel: Channel,
    device: [u8; 32],
    paired_now: bool,
) -> anyhow::Result<()> {
    // subscribed panes, and the screen number each was sent up to
    let mut watching: HashMap<PaneId, u64> = HashMap::new();
    let mut seen_generation = u64::MAX;
    let mut open: HashSet<PaneId> = HashSet::new();
    let mut last_heard = Instant::now();
    let mut last_sent = Instant::now();
    let mut welcomed = false;
    loop {
        match ws.read() {
            Ok(Message::Binary(b)) => {
                last_heard = Instant::now();
                if let Some(frame) = channel.open(&b)?.and_then(|bytes| wire::decode(&bytes)) {
                    match frame.body {
                        Some(Body::Hello(hello)) => {
                            let answer = wire::answer(&hello);
                            let refused = matches!(answer.body, Some(Body::Refused(_)));
                            send(&mut ws, &mut channel, &answer)?;
                            anyhow::ensure!(!refused, "no common version");
                            welcomed = true;
                            // a device just paired: its name, for NativeTerm's list
                            if paired_now {
                                record(shared, &device, &hello.device_name)?;
                            }
                            let state = shared.state();
                            open = state.sessions.iter().copied().collect();
                            send(&mut ws, &mut channel, &sessions(&state))?;
                            last_sent = Instant::now();
                        }
                        Some(Body::Subscribe(s)) if welcomed => {
                            if let Some(pane) = s
                                .session
                                .parse::<PaneId>()
                                .ok()
                                .filter(|p| open.contains(p))
                            {
                                watching.insert(pane, s.since);
                                seen_generation = u64::MAX;
                            }
                        }
                        // read-only for now (R2 takes input)
                        _ => {}
                    }
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.into()),
        }
        anyhow::ensure!(last_heard.elapsed() < SILENCE, "silent too long");
        // a device whose pairing was taken away goes at once
        anyhow::ensure!(shared.paired(&device), "the pairing was revoked");
        if !welcomed {
            continue;
        }
        // the sessions open now; one closed (or turned off) ends
        let state = shared.state();
        let now_open: HashSet<PaneId> = state.sessions.iter().copied().collect();
        if now_open != open {
            for gone in open.difference(&now_open) {
                watching.remove(gone);
                let ended = Ended {
                    session: gone.to_string(),
                    reason: ended::Reason::TurnedOff as i32,
                };
                send(&mut ws, &mut channel, &wire::frame(Body::Ended(ended)))?;
            }
            open = now_open;
            send(&mut ws, &mut channel, &sessions(&state))?;
            last_sent = Instant::now();
        }
        let generation = shared.generation.load(Ordering::Relaxed);
        if generation != seen_generation {
            seen_generation = generation;
            let panes: Vec<PaneId> = watching.keys().copied().collect();
            for pane in panes {
                match shared.since(pane, watching[&pane]) {
                    None => {
                        watching.remove(&pane);
                        let ended = Ended {
                            session: pane.to_string(),
                            reason: ended::Reason::TabClosed as i32,
                        };
                        send(&mut ws, &mut channel, &wire::frame(Body::Ended(ended)))?;
                    }
                    Some(Since::Nothing) => {}
                    Some(Since::Rows(rows)) => {
                        watching.insert(pane, rows.seq);
                        send(&mut ws, &mut channel, &wire::frame(Body::Rows(rows)))?;
                    }
                    Some(Since::Snapshot(chunks)) => {
                        let seq = chunks.first().map_or(0, |c| c.seq);
                        for chunk in chunks {
                            send(&mut ws, &mut channel, &wire::frame(Body::Snapshot(chunk)))?;
                        }
                        watching.insert(pane, seq);
                    }
                }
                last_sent = Instant::now();
            }
        }
        if last_sent.elapsed() > HEARTBEAT {
            send(
                &mut ws,
                &mut channel,
                &wire::frame(Body::Heartbeat(Heartbeat {})),
            )?;
            last_sent = Instant::now();
        }
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;

    #[test]
    fn hex_both_ways_and_state_read() {
        let key = [0x00u8, 0x7f, 0xff, 0x10];
        assert_eq!(hex(&key), "007fff10");
        assert_eq!(unhex("007fff10").unwrap(), key);
        assert!(unhex("0g").is_none() && unhex("123").is_none());
        let state: State = serde_json::from_str(
            r#"{"port": 47100, "sessions": [3, 7], "pairing": {"secret": "ab", "expires": 9}, "web": "C:/x"}"#,
        )
        .unwrap();
        assert_eq!(
            (state.port, state.sessions.as_slice()),
            (47100, &[3usize, 7][..])
        );
        assert_eq!(state.pairing.unwrap().expires, 9);
        assert_eq!(
            serde_json::from_str::<State>("{}").unwrap(),
            State::default(),
            "all optional"
        );
    }

    #[test]
    fn web_files_stay_in_their_folder() {
        let root = Path::new("/web");
        assert_eq!(web_file(root, "/"), Some(root.join("index.html")));
        assert_eq!(
            web_file(root, "/pkg/core.js?v=2"),
            Some(root.join("pkg/core.js"))
        );
        for bad in [
            "/../secret",
            "/pkg/../../x",
            "/a//b",
            "/c:/windows",
            "/a\\..\\b",
            "/./x",
        ] {
            assert_eq!(web_file(root, bad), None, "{bad}");
        }
        assert_eq!(
            content_type(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("pkg/core_bg.wasm")),
            "application/wasm"
        );
    }
}
