//! Direct connections (NativeTerm's docs/REMOTE.md, R4): a device in a
//! session (on the LAN or through the relay) offers a WebRTC data
//! channel; this desktop answers with its own candidates (its addresses,
//! and the public one the person's relay sees, by STUN), and once the
//! channel opens the device resumes a session over it, the handshake and
//! all, as over a WebSocket. The device then closes the connection it
//! offered on.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use native_term_p2p::{Peer, Received};

use crate::server::{handshake, not_paired, session, Got, Link, NotPaired, Shared, State};

/// How long the device has to open the channel after the answer.
const OPEN_WITHIN: Duration = Duration::from_secs(20);
/// The relay's STUN port (`nativeterm-server --stun`).
const STUN_PORT: u16 = 3478;

/// A data channel as a connection to a device.
struct Direct {
    peer: Peer,
    every: Duration,
}

impl Link for Direct {
    fn receive(&mut self) -> anyhow::Result<Got> {
        Ok(match self.peer.receive(self.every)? {
            Received::Message(m) => Got::Message(m),
            Received::Nothing => Got::Nothing,
            Received::Closed => Got::Closed,
        })
    }

    fn send(&mut self, message: Vec<u8>) -> anyhow::Result<()> {
        self.peer.send(&message)
    }

    /// A data channel has no close codes: the device, its channel gone,
    /// comes back over a WebSocket, where "not paired" and "disconnected"
    /// are said with theirs.
    fn close(&mut self, _code: u16, _reason: &str) {
        self.peer.close();
    }

    fn wait(&mut self, every: Duration) -> std::io::Result<()> {
        self.every = every;
        Ok(())
    }
}

/// Answers a device's offer, away from its session: the answer's SDP
/// comes on the receiver (empty when there is none to give); the channel
/// is then waited for, and the device's session served on it.
pub(crate) fn answer(shared: &Arc<Shared>, offer: String) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::clone(shared);
    let spawned = std::thread::Builder::new()
        .name("nativeterm-direct".into())
        .spawn(move || {
            let stun = stun_servers(&shared.state());
            let addresses = native_term_p2p::own_addresses();
            match native_term_p2p::answer(&offer, &addresses, &stun) {
                Ok((peer, sdp)) => {
                    let _ = tx.send(sdp);
                    if let Err(e) = serve(&shared, peer) {
                        log::debug!("nativeterm remote control, direct: {e:#}");
                    }
                }
                Err(e) => {
                    log::debug!("nativeterm remote control, no direct answer: {e:#}");
                    let _ = tx.send(String::new());
                }
            }
        });
    if let Err(e) = spawned {
        log::debug!("nativeterm remote control, direct: {e}");
    }
    rx
}

fn serve(shared: &Arc<Shared>, mut peer: Peer) -> anyhow::Result<()> {
    peer.open(OPEN_WITHIN)?;
    let mut link = Direct {
        peer,
        every: Duration::from_secs(1),
    };
    // a device that is not paired yet pairs over the WebSocket
    let (channel, device) = match handshake(shared, &mut link, false) {
        Ok(done) => done,
        Err(e) if e.is::<NotPaired>() => return not_paired(link),
        Err(e) => return Err(e),
    };
    match session(shared, link, channel, device, false, "direct") {
        Err(e) if e.is::<NotPaired>() => Ok(()),
        other => other,
    }
}

/// The STUN servers to ask: the person's relay's, when there is one.
fn stun_servers(state: &State) -> Vec<SocketAddr> {
    let Some(host) = state.relay.as_ref().and_then(|r| relay_host(&r.url)) else {
        return Vec::new();
    };
    (host.as_str(), STUN_PORT)
        .to_socket_addrs()
        .map(|a| a.collect())
        .unwrap_or_default()
}

/// The host in `wss://host[:port][/...]` (an IPv6 one without brackets).
fn relay_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
mod tests {
    use super::relay_host;

    #[test]
    fn the_relay_host() {
        assert_eq!(
            relay_host("wss://relay.example.com").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(
            relay_host("ws://10.0.0.2:8443/x").as_deref(),
            Some("10.0.0.2")
        );
        assert_eq!(
            relay_host("wss://[2001:db8::1]:443").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(relay_host("wss://"), None);
    }
}
