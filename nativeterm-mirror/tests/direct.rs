//! A direct connection end to end on this machine: a device paired over
//! the LAN's WebSocket offers a data channel in its session, the desktop
//! answers, and the device resumes over the channel (listed as "direct").

use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use native_term_p2p::Received;
use native_term_remote::noise::{Channel, Handshake, Keys, Ticket};
use native_term_remote::proto::frame::Body;
use native_term_remote::wire;
use nativeterm_mirror::server::{hex, start};
use tungstenite::{Message, WebSocket};

type Ws = WebSocket<TcpStream>;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn ws(port: u16, path: &str) -> Ws {
    let deadline = Instant::now() + Duration::from_secs(10);
    let stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("the service did not start: {e}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    tungstenite::client(format!("ws://127.0.0.1:{port}{path}"), stream)
        .unwrap()
        .0
}

/// One end of a connection, whatever carries it.
trait Carrier {
    fn put(&mut self, message: Vec<u8>);
    fn take(&mut self) -> Option<Vec<u8>>;
}

impl Carrier for Ws {
    fn put(&mut self, message: Vec<u8>) {
        self.send(Message::Binary(message.into())).unwrap();
    }
    fn take(&mut self) -> Option<Vec<u8>> {
        loop {
            match self.read().ok()? {
                Message::Binary(b) => return Some(b.to_vec()),
                Message::Close(_) => return None,
                _ => {}
            }
        }
    }
}

impl Carrier for native_term_p2p::Peer {
    fn put(&mut self, message: Vec<u8>) {
        self.send(&message).unwrap();
    }
    fn take(&mut self) -> Option<Vec<u8>> {
        match self.receive(Duration::from_secs(10)).unwrap() {
            Received::Message(m) => Some(m),
            _ => None,
        }
    }
}

fn shake(c: &mut impl Carrier, mut hs: Handshake) -> Channel {
    while !hs.finished() {
        if hs.my_turn() {
            c.put(hs.write().unwrap());
        } else {
            hs.read(&c.take().expect("the handshake")).unwrap();
        }
    }
    hs.channel().unwrap().0
}

fn send(c: &mut impl Carrier, channel: &mut Channel, frame: &native_term_remote::proto::Frame) {
    for m in channel.seal(&wire::encode(frame)).unwrap() {
        c.put(m);
    }
}

fn receive(c: &mut impl Carrier, channel: &mut Channel) -> Body {
    loop {
        if let Some(bytes) = channel.open(&c.take().expect("a frame")).unwrap() {
            return wire::decode(&bytes).and_then(|f| f.body).unwrap();
        }
    }
}

/// `connections.json`, without its spaces.
fn connections(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("connections.json"))
        .unwrap()
        .split_whitespace()
        .collect()
}

#[test]
fn a_device_moves_onto_a_data_channel() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let desktop = Keys::generate().unwrap();
    let mut key = desktop.private().to_vec();
    key.extend_from_slice(&desktop.public);
    std::fs::write(dir.join("desktop.key"), key).unwrap();
    let port = free_port();
    let secret = [5u8; 32];
    let later = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    std::fs::write(
        dir.join("state.json"),
        format!(
            r#"{{"port": {port}, "listen": "127.0.0.1", "sessions": [], "pairing": {{"secret": "{}", "expires": {later}}}}}"#,
            hex(&secret)
        ),
    )
    .unwrap();
    start(dir.to_path_buf());

    // paired on the LAN
    let phone = Keys::generate().unwrap();
    let ticket = Ticket {
        desktop: desktop.public,
        secret,
    };
    let mut socket = ws(port, "/ws/pair");
    let mut channel = shake(
        &mut socket,
        Handshake::pair_device(&phone, &ticket).unwrap(),
    );
    send(&mut socket, &mut channel, &wire::hello("my phone"));
    let Body::Welcome(welcome) = receive(&mut socket, &mut channel) else {
        panic!("no welcome")
    };
    assert!(welcome.capabilities.iter().any(|c| c == wire::DIRECT));
    assert!(matches!(
        receive(&mut socket, &mut channel),
        Body::Sessions(_)
    ));

    // the offer, in the session; the answer back in it
    let started = Instant::now();
    let addresses = ["127.0.0.1".parse().unwrap()];
    let (offering, sdp) = native_term_p2p::offer(&addresses, &[], &[]).unwrap();
    send(
        &mut socket,
        &mut channel,
        &wire::frame(Body::DirectOffer(native_term_remote::proto::DirectOffer {
            sdp,
        })),
    );
    let answer = loop {
        if let Body::DirectAnswer(a) = receive(&mut socket, &mut channel) {
            break a.sdp;
        }
    };
    assert!(!answer.is_empty(), "the desktop answered");
    let mut peer = offering.connect(&answer).unwrap();
    peer.open(Duration::from_secs(10)).unwrap();
    let opened = started.elapsed();

    // resumed over the channel
    let mut direct = shake(
        &mut peer,
        Handshake::resume_device(&phone, &desktop.public).unwrap(),
    );
    send(&mut peer, &mut direct, &wire::hello("my phone"));
    assert!(matches!(receive(&mut peer, &mut direct), Body::Welcome(_)));
    assert!(matches!(receive(&mut peer, &mut direct), Body::Sessions(_)));
    eprintln!(
        "channel open in {opened:?}, session over it in {:?}",
        started.elapsed()
    );
    let listed = connections(dir);
    assert!(listed.contains("\"via\":\"direct\""), "{listed}");
    assert!(listed.contains("\"via\":\"lan\""), "{listed}");

    // the WebSocket closed: the channel's session goes on
    drop(socket);
    send(
        &mut peer,
        &mut direct,
        &wire::frame(Body::Heartbeat(Default::default())),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let listed = connections(dir);
        if !listed.contains("\"via\":\"lan\"") {
            assert!(listed.contains("\"via\":\"direct\""), "{listed}");
            break;
        }
        assert!(Instant::now() < deadline, "the LAN connection still listed");
        std::thread::sleep(Duration::from_millis(100));
    }
}
