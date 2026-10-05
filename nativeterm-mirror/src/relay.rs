//! The desktop's side of the person's relay (NativeTerm's
//! `nativeterm-server`), for devices that cannot reach this desktop
//! directly: a control connection held in the desktop's room while
//! `state.json` names a relay; for each device the relay announces, a
//! data connection, over which the same handshake and session run as on
//! the LAN. The relay carries only sealed messages.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::server::{handshake, not_paired, session, NotPaired, RelaySettings, Shared};

/// How long connecting (and the TLS and WebSocket handshakes) may take.
const CONNECT: Duration = Duration::from_secs(10);
/// How often the control connection looks at `state.json` again.
const LOOK: Duration = Duration::from_secs(1);
/// A data connection's read timeout, as the LAN's (`server::TICK`).
const TICK: Duration = Duration::from_millis(40);

type Relayed = WebSocket<MaybeTlsStream<TcpStream>>;

/// Hold the room for as long as there is a relay to hold it at; tried
/// again after 1, 2, 4... up to 30 s when the relay cannot be reached.
pub(crate) fn run(shared: Arc<Shared>) {
    let mut attempt = 0u32;
    loop {
        let Some(relay) = shared.state().relay.filter(|r| !r.url.is_empty()) else {
            attempt = 0;
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        match control(&shared, &relay) {
            Ok(()) => attempt = 0,
            Err(e) => {
                log::info!("nativeterm relay {}: {e:#}", relay.url);
                attempt = (attempt + 1).min(6);
            }
        }
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs((1u64 << (attempt - 1)).min(30)));
        }
    }
}

/// The control connection: `incoming <conn> <path>` for each device; over
/// when the relay goes or `state.json` names another relay (or none).
fn control(shared: &Arc<Shared>, relay: &RelaySettings) -> anyhow::Result<()> {
    let mut ws = open(relay, &format!("/desktop/{}", relay.room))?;
    timeout(&ws, Some(LOOK))?;
    log::info!("nativeterm relay: room held at {}", relay.url);
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let mut parts = text.split(' ');
                if let (Some("incoming"), Some(conn), Some(path)) =
                    (parts.next(), parts.next(), parts.next())
                {
                    let pairing = path == "/ws/pair";
                    let (shared, relay, conn) =
                        (Arc::clone(shared), relay.clone(), conn.to_string());
                    std::thread::spawn(move || {
                        if let Err(e) = device(&shared, &relay, &conn, pairing) {
                            log::debug!("nativeterm relay: a device: {e:#}");
                        }
                    });
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
        if shared.state().relay.as_ref() != Some(relay) {
            let _ = ws.close(None);
            return Ok(());
        }
    }
}

/// One device through the relay: the data connection, then as on the LAN.
fn device(
    shared: &Arc<Shared>,
    relay: &RelaySettings,
    conn: &str,
    pairing: bool,
) -> anyhow::Result<()> {
    let mut ws = open(relay, &format!("/desktop/{}/{conn}", relay.room))?;
    let (channel, device) = match handshake(shared, &mut ws, pairing) {
        Ok(done) => done,
        Err(e) if e.is::<NotPaired>() => return not_paired(ws),
        Err(e) => return Err(e),
    };
    timeout(&ws, Some(TICK))?;
    match session(shared, ws, channel, device, pairing, "relay") {
        Err(e) if e.is::<NotPaired>() => Ok(()),
        other => other,
    }
}

/// A WebSocket to the relay at `path`, with the token.
fn open(relay: &RelaySettings, path: &str) -> anyhow::Result<Relayed> {
    let url = format!("{}{path}", relay.url.trim_end_matches('/'));
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {}", relay.token).parse()?);
    let uri = request.uri().clone();
    let host = uri
        .host()
        .ok_or_else(|| anyhow::anyhow!("{url}: no host"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });
    let at = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("{host}: no address"))?;
    let stream = TcpStream::connect_timeout(&at, CONNECT)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(CONNECT))?;
    let (ws, _) =
        tungstenite::client_tls(request, stream).map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    Ok(ws)
}

impl crate::server::Wait for MaybeTlsStream<TcpStream> {
    fn wait(&self, every: Duration) -> std::io::Result<()> {
        match self {
            MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(every)),
            MaybeTlsStream::NativeTls(s) => s.get_ref().set_read_timeout(Some(every)),
            _ => Ok(()),
        }
    }
}

/// The read timeout of the TCP connection under the WebSocket (and TLS).
fn timeout(ws: &Relayed, t: Option<Duration>) -> std::io::Result<()> {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(t),
        MaybeTlsStream::NativeTls(s) => s.get_ref().set_read_timeout(t),
        _ => Ok(()),
    }
}
