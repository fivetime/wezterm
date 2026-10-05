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
    ended, frame::Body, input_result::Reason, Ended, Frame, Heartbeat, InputResult, SessionInfo,
    Sessions,
};
use native_term_remote::screen::Since;
use native_term_remote::wire;
use serde::{Deserialize, Serialize};
use tungstenite::{Message, WebSocket};

use crate::{PaneMirror, PaneSource};

/// How often a connection looks for changes, and sends a heartbeat.
const TICK: Duration = Duration::from_millis(40);
/// How often while something is happening (input came, rows went):
/// an echo is sent within this of the pane drawing it, not within `TICK`.
const QUICK: Duration = Duration::from_millis(4);
/// How long after the last input or output it stays `QUICK`.
const BUSY: Duration = Duration::from_millis(500);

/// The stream under a connection's WebSocket, whose read timeout is how
/// often the connection looks for changes.
pub(crate) trait Wait {
    fn wait(&self, every: Duration) -> std::io::Result<()>;
}

impl Wait for TcpStream {
    fn wait(&self, every: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(every))
    }
}
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
    /// Panes open to be seen only: input to them is refused.
    #[serde(default)]
    pub read_only: Vec<PaneId>,
    /// The person's relay, for devices that cannot reach this desktop
    /// directly.
    #[serde(default)]
    pub relay: Option<RelaySettings>,
    /// Connections (`connections.json`'s ids) the desktop disconnected:
    /// closed with `CLOSE_DISCONNECTED`.
    #[serde(default)]
    pub disconnect: Vec<u64>,
}

/// What a paired device may do: see only, or see and type. Chosen with
/// the pairing code, changed on the desktop at any time (its file in
/// `devices/`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    View,
    #[default]
    Operate,
}

/// A connection now, as `connections.json` lists it for NativeTerm.
#[derive(Clone, Debug, Serialize)]
pub struct Connection {
    /// Unique across this desktop's runs.
    pub id: u64,
    /// The device's key, in hex.
    pub device: String,
    pub name: String,
    /// `lan` or `relay`.
    pub via: &'static str,
    /// Seconds since the epoch.
    pub since: u64,
    /// When it last typed, if it did.
    pub typed: Option<u64>,
    /// The sessions (pane ids) it watches.
    pub watching: Vec<PaneId>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct RelaySettings {
    /// `wss://host[:port]` (or `ws://` behind nothing, for trying).
    pub url: String,
    /// The relay's access token.
    pub token: String,
    /// This desktop's room there (random, in the pairing link).
    pub room: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Pairing {
    /// The one-time secret, in hex (64 digits).
    pub secret: String,
    /// What the device paired with it may do.
    #[serde(default)]
    pub role: Role,
    /// When it expires (seconds since the epoch).
    pub expires: u64,
}

/// What is written for a paired device.
#[derive(Serialize, Deserialize)]
struct Paired<'a> {
    #[serde(borrow, default)]
    name: std::borrow::Cow<'a, str>,
    #[serde(default)]
    paired: u64,
    #[serde(default)]
    role: Role,
}

/// A device's role, and when its file was written (`None`: no file).
type RoleSeen = (Option<SystemTime>, Role);

pub(crate) struct Shared {
    dir: PathBuf,
    keys: Keys,
    /// Bumped on every output of a pane: connections look again.
    generation: AtomicU64,
    mirrors: Mutex<HashMap<PaneId, PaneMirror>>,
    state: Mutex<(Option<SystemTime>, State)>,
    /// Pairing secrets a device paired with: each pairs one device only.
    /// (Claimed when a pairing succeeds, not when one starts: anyone on
    /// the network may start one, and must not use the code up.)
    used: Mutex<HashSet<String>>,
    /// The connections now (after their welcome), for `connections.json`.
    connections: Mutex<HashMap<u64, Connection>>,
    /// A connection's id: this run's start, then a count.
    next_connection: AtomicU64,
    /// Devices' roles, with their files' times.
    roles: Mutex<HashMap<[u8; 32], RoleSeen>>,
    /// Panes held at a device's size, and the connection that asked last.
    holders: Mutex<HashMap<PaneId, u64>>,
}

impl Shared {
    /// `connections.json` written again (whole, then renamed into place).
    fn write_connections(&self) {
        let list: Vec<Connection> = {
            let mut all: Vec<Connection> =
                self.connections.lock().unwrap().values().cloned().collect();
            all.sort_by_key(|c| c.id);
            all
        };
        let path = self.dir.join("connections.json");
        let temp = self.dir.join("connections.json.new");
        let written = serde_json::to_vec_pretty(&list)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&temp, bytes))
            .and_then(|()| std::fs::rename(&temp, &path));
        if let Err(e) = written {
            log::debug!("nativeterm remote control: connections.json: {e}");
        }
    }

    fn connection_changed(&self, id: u64, change: impl FnOnce(&mut Connection)) {
        if let Some(c) = self.connections.lock().unwrap().get_mut(&id) {
            change(c);
        }
        self.write_connections();
    }
    /// `state.json` as it is now (read again when it changed).
    pub(crate) fn state(&self) -> State {
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

    /// The device's role, as its file says now (read again only when
    /// the file changed: this is asked on every look).
    fn role(&self, key: &[u8; 32]) -> Role {
        let path = self.dir.join("devices").join(format!("{}.json", hex(key)));
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let mut roles = self.roles.lock().unwrap();
        if let Some((at, role)) = roles.get(key) {
            if *at == modified {
                return *role;
            }
        }
        let role = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Paired>(&t).ok().map(|p| p.role))
            .unwrap_or_default();
        roles.insert(*key, (modified, role));
        role
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

/// The service, once started (for `is_open`).
static SERVING: std::sync::OnceLock<Arc<Shared>> = std::sync::OnceLock::new();

/// Whether the pane is open to remote control now: its tab carries a
/// mark in the tab strip while it is.
pub fn is_open(pane: PaneId) -> bool {
    SERVING
        .get()
        .is_some_and(|shared| shared.state().sessions.contains(&pane))
}

/// The tabs whose panes were opened or closed to remote control are told
/// to draw their titles again (their mark comes or goes).
fn watch_marks(shared: Arc<Shared>) {
    let mut before: HashSet<PaneId> = HashSet::new();
    loop {
        let now: HashSet<PaneId> = shared.state().sessions.iter().copied().collect();
        if now != before {
            if let Some(mux) = Mux::try_get() {
                for pane in now.symmetric_difference(&before) {
                    if let Some(tab) = mux
                        .resolve_pane_id(*pane)
                        .and_then(|(_, _, tab)| mux.get_tab(tab))
                    {
                        Mux::notify_from_any_thread(MuxNotification::TabTitleChanged {
                            tab_id: tab.tab_id(),
                            title: tab.get_title(),
                        });
                    }
                }
            }
            before = now;
        }
        std::thread::sleep(Duration::from_millis(300));
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
        used: Mutex::new(HashSet::new()),
        connections: Mutex::new(HashMap::new()),
        next_connection: AtomicU64::new(now() * 1000),
        roles: Mutex::new(HashMap::new()),
        holders: Mutex::new(HashMap::new()),
    });
    // none yet (a list left by the run before is not true now)
    shared.write_connections();
    // NativeTerm writes the port first; wait for it
    let port = loop {
        let port = shared.state().port;
        if port != 0 {
            break port;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("nativeterm-relay".into())
            .spawn(move || crate::relay::run(shared))?;
    }
    let _ = SERVING.set(Arc::clone(&shared));
    {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("nativeterm-marks".into())
            .spawn(move || watch_marks(shared))?;
    }
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
    text.len()
        .is_multiple_of(2)
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
    let (channel, device) = match handshake(shared, &mut ws, pairing) {
        Ok(done) => done,
        Err(e) if e.is::<NotPaired>() => return not_paired(ws),
        Err(e) => return Err(e),
    };
    ws.get_ref().set_read_timeout(Some(TICK))?;
    match session(shared, ws, channel, device, pairing, "lan") {
        Err(e) if e.is::<NotPaired>() => Ok(()),
        other => other,
    }
}

/// A device this desktop does not know (never paired, or removed): it is
/// told so (`CLOSE_NOT_PAIRED`), and stops trying.
#[derive(Debug)]
pub(crate) struct NotPaired;

impl std::fmt::Display for NotPaired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not a paired device")
    }
}

impl std::error::Error for NotPaired {}

pub(crate) fn not_paired<S: Read + Write>(mut ws: Ws<S>) -> anyhow::Result<()> {
    close_with(&mut ws, wire::CLOSE_NOT_PAIRED, "not paired");
    Ok(())
}

/// Close with `code`; the device's answer read before the connection
/// goes.
fn close_with<S: Read + Write>(ws: &mut Ws<S>, code: u16, reason: &str) {
    use tungstenite::protocol::frame::coding::CloseCode;
    use tungstenite::protocol::CloseFrame;
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: reason.to_string().into(),
    };
    let _ = ws.close(Some(frame));
    // the close goes out before the socket does, and the device's answer
    // is read: a connection dropped with unread data is reset, and the
    // reset can throw the close (and its code) away on the way
    let _ = ws.flush();
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        match ws.read() {
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
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

pub(crate) type Ws<S = TcpStream> = WebSocket<S>;

fn binary<S: Read + Write>(ws: &mut Ws<S>) -> anyhow::Result<Vec<u8>> {
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
pub(crate) fn handshake<S: Read + Write>(
    shared: &Shared,
    ws: &mut Ws<S>,
    pairing: bool,
) -> anyhow::Result<(Channel, [u8; 32])> {
    let mut secret_hex = None;
    let mut role = Role::Operate;
    let mut hs = if pairing {
        let state = shared.state();
        let pairing = state
            .pairing
            .ok_or_else(|| anyhow::anyhow!("no pairing asked for"))?;
        anyhow::ensure!(pairing.expires > now(), "the pairing expired");
        anyhow::ensure!(
            !shared.used.lock().unwrap().contains(&pairing.secret),
            "the pairing code was used"
        );
        secret_hex = Some(pairing.secret.clone());
        role = pairing.role;
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
                if !shared.paired(&key) {
                    return Err(NotPaired.into());
                }
            }
        }
    }
    let (channel, device) = hs.channel()?;
    if let Some(secret) = secret_hex {
        // one device per code, whichever finished first
        anyhow::ensure!(
            shared.used.lock().unwrap().insert(secret),
            "the pairing code was used"
        );
        record(shared, &device, "", Some(role))?;
    }
    Ok((channel, device))
}

/// A paired device, for NativeTerm's list: its name and role (`None`:
/// the role it has).
fn record(
    shared: &Shared,
    device: &[u8; 32],
    name: &str,
    role: Option<Role>,
) -> anyhow::Result<()> {
    let dir = shared.dir.join("devices");
    std::fs::create_dir_all(&dir)?;
    let role = role.unwrap_or_else(|| shared.role(device));
    let paired = serde_json::to_vec(&serde_json::json!({
        "name": name,
        "paired": now(),
        "role": role,
    }))?;
    std::fs::write(dir.join(format!("{}.json", hex(device))), paired)?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn send<S: Read + Write>(
    ws: &mut Ws<S>,
    channel: &mut Channel,
    frame: &Frame,
) -> anyhow::Result<()> {
    for message in channel.seal(&wire::encode(frame))? {
        ws.send(Message::Binary(message.into()))?;
    }
    Ok(())
}

/// The sessions open, as this device sees them: read only where the tab
/// is, or everywhere when the device may only see.
fn sessions(state: &State, viewer: bool) -> Frame {
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
                read_only: viewer || state.read_only.contains(id),
            })
        })
        .collect();
    wire::frame(Body::Sessions(Sessions { sessions }))
}

/// Hold the pane at `cols` x `lines` (0 columns: give it back to its
/// window), on the GUI's thread, the window told to draw it again.
fn hold_size(pane: PaneId, cols: u32, lines: u32) {
    // (no mux, no GUI thread: the tests' service)
    if Mux::try_get().is_none() {
        return;
    }
    promise::spawn::spawn_into_main_thread(async move {
        let Some(mux) = Mux::try_get() else { return };
        let Some(pane_ref) = mux.get_pane(pane) else {
            return;
        };
        if cols == 0 {
            mux::held_size::release(pane);
        } else {
            mux::held_size::hold(pane, cols as usize, lines as usize);
        }
        // what the window wants, now held or not
        let size = mux::held_size::requested(pane).unwrap_or_else(|| {
            let d = pane_ref.get_dimensions();
            wezterm_term::TerminalSize {
                rows: d.viewport_rows,
                cols: d.cols,
                pixel_width: d.pixel_width,
                pixel_height: d.pixel_height,
                dpi: d.dpi,
            }
        });
        let _ = pane_ref.resize(size);
        if let Some((_, _, tab)) = mux.resolve_pane_id(pane) {
            Mux::notify_from_any_thread(MuxNotification::TabResized(tab));
        }
    })
    .detach();
}

/// A device's input to a session open to remote control (and not read
/// only), as the desktop's keyboard would type it; recorded in
/// `input.log`.
fn apply_input(
    shared: &Shared,
    device: &[u8; 32],
    connection: u64,
    input: &native_term_remote::proto::Input,
) -> Reason {
    let Some(kind) = &input.kind else {
        return Reason::UnknownKey;
    };
    let state = shared.state();
    let pane_id = input.session.parse::<PaneId>().ok();
    let pane = pane_id
        .filter(|p| state.sessions.contains(p))
        .and_then(|p| Mux::try_get()?.get_pane(p));
    let reason = match (pane, pane_id) {
        (None, _) => Reason::NoSession,
        (Some(_), Some(id)) if state.read_only.contains(&id) => Reason::ReadOnly,
        (Some(_), _) if shared.role(device) == Role::View => Reason::ReadOnly,
        (Some(pane), _) => crate::input::apply(&*pane, kind),
    };
    // the device by the start of its key, and which of its connections
    let who = format!("{} #{connection}", hex(&device[..4]));
    crate::input::record(&shared.dir, &who, &input.session, kind, reason);
    reason
}

/// A device after the handshake: what it asks, and what changes.
pub(crate) fn session<S: Read + Write + Wait>(
    shared: &Shared,
    mut ws: Ws<S>,
    mut channel: Channel,
    device: [u8; 32],
    paired_now: bool,
    via: &'static str,
) -> anyhow::Result<()> {
    let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    // listed from its welcome until it ends, however it ends
    struct Listed<'a>(&'a Shared, u64);
    impl Drop for Listed<'_> {
        fn drop(&mut self) {
            if self.0.connections.lock().unwrap().remove(&self.1).is_some() {
                self.0.write_connections();
            }
            // the tabs it held at its size go back to their windows
            let held: Vec<PaneId> = {
                let mut holders = self.0.holders.lock().unwrap();
                let mine: Vec<PaneId> = holders
                    .iter()
                    .filter(|(_, c)| **c == self.1)
                    .map(|(p, _)| *p)
                    .collect();
                for pane in &mine {
                    holders.remove(pane);
                }
                mine
            };
            for pane in held {
                hold_size(pane, 0, 0);
            }
        }
    }
    let _listed = Listed(shared, id);
    // subscribed panes, and the screen number each was sent up to
    let mut watching: HashMap<PaneId, u64> = HashMap::new();
    let mut seen_generation = u64::MAX;
    let mut open: HashSet<PaneId> = HashSet::new();
    let mut last_heard = Instant::now();
    let mut last_sent = Instant::now();
    let mut welcomed = false;
    let mut busy_until = Instant::now();
    let mut every = TICK;
    let mut viewer = shared.role(&device) == Role::View;
    loop {
        let want = if Instant::now() < busy_until {
            QUICK
        } else {
            TICK
        };
        if want != every {
            ws.get_ref().wait(want)?;
            every = want;
        }
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
                                record(shared, &device, &hello.device_name, None)?;
                            }
                            shared.connections.lock().unwrap().insert(
                                id,
                                Connection {
                                    id,
                                    device: hex(&device),
                                    name: hello.device_name.clone(),
                                    via,
                                    since: now(),
                                    typed: None,
                                    watching: Vec::new(),
                                },
                            );
                            shared.write_connections();
                            let state = shared.state();
                            open = state.sessions.iter().copied().collect();
                            send(&mut ws, &mut channel, &sessions(&state, viewer))?;
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
                                let panes: Vec<PaneId> = watching.keys().copied().collect();
                                shared.connection_changed(id, |c| c.watching = panes);
                            }
                        }
                        Some(Body::HoldSize(h)) if welcomed => {
                            if let Some(pane) = h
                                .session
                                .parse::<PaneId>()
                                .ok()
                                .filter(|p| open.contains(p))
                            {
                                let mut holders = shared.holders.lock().unwrap();
                                let give = if h.cols == 0 {
                                    // given back only by the one holding it
                                    holders.get(&pane) == Some(&id)
                                        && holders.remove(&pane).is_some()
                                } else {
                                    holders.insert(pane, id);
                                    true
                                };
                                drop(holders);
                                if give {
                                    hold_size(pane, h.cols, h.lines);
                                }
                            }
                        }
                        Some(Body::Input(input)) if welcomed => {
                            busy_until = Instant::now() + BUSY;
                            let reason = apply_input(shared, &device, id, &input);
                            if reason == Reason::Applied {
                                // (written at most once a second while typing)
                                let at = now();
                                let stale = shared
                                    .connections
                                    .lock()
                                    .unwrap()
                                    .get(&id)
                                    .is_some_and(|c| c.typed != Some(at));
                                if stale {
                                    shared.connection_changed(id, |c| c.typed = Some(at));
                                }
                            }
                            let result = InputResult {
                                session: input.session,
                                id: input.id,
                                reason: reason as i32,
                            };
                            send(
                                &mut ws,
                                &mut channel,
                                &wire::frame(Body::InputResult(result)),
                            )?;
                            last_sent = Instant::now();
                        }
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
        // a device whose pairing was taken away goes at once, told so
        if !shared.paired(&device) {
            not_paired(ws)?;
            return Err(NotPaired.into());
        }
        if !welcomed {
            continue;
        }
        // the sessions open now; one closed (or turned off) ends
        let state = shared.state();
        // the role changed on the desktop: the sessions again, as it sees them
        let now_viewer = shared.role(&device) == Role::View;
        if now_viewer != viewer {
            viewer = now_viewer;
            send(&mut ws, &mut channel, &sessions(&state, viewer))?;
            last_sent = Instant::now();
        }
        // disconnected by the desktop: told so, and not coming back by itself
        if state.disconnect.contains(&id) {
            close_with(
                &mut ws,
                wire::CLOSE_DISCONNECTED,
                "disconnected by the desktop",
            );
            return Ok(());
        }
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
            send(&mut ws, &mut channel, &sessions(&state, viewer))?;
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
                        busy_until = Instant::now() + BUSY;
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
