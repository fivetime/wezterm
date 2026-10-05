//! The LAN service end to end over loopback: the web client's files,
//! pairing with the QR code's secret, resuming, a stranger refused, a
//! revoked device cut off. (No mux here: no panes, an empty session list.)

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

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

fn wait_for(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the service did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

fn ws(port: u16, path: &str) -> Ws {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}{path}"), stream).unwrap();
    ws
}

fn binary(ws: &mut Ws) -> Option<Vec<u8>> {
    loop {
        match ws.read().ok()? {
            Message::Binary(b) => return Some(b.to_vec()),
            Message::Close(_) => return None,
            _ => {}
        }
    }
}

/// The device's side of a handshake; `None` when the desktop hangs up.
fn shake(ws: &mut Ws, mut hs: Handshake) -> Option<Channel> {
    while !hs.finished() {
        if hs.my_turn() {
            ws.send(Message::Binary(hs.write().ok()?.into())).ok()?;
        } else {
            hs.read(&binary(ws)?).ok()?;
        }
    }
    hs.channel().ok().map(|(c, _)| c)
}

fn send(ws: &mut Ws, channel: &mut Channel, frame: &native_term_remote::proto::Frame) {
    for m in channel.seal(&wire::encode(frame)).unwrap() {
        ws.send(Message::Binary(m.into())).unwrap();
    }
}

fn receive(ws: &mut Ws, channel: &mut Channel) -> Option<Body> {
    loop {
        if let Some(bytes) = channel.open(&binary(ws)?).ok()? {
            return wire::decode(&bytes).and_then(|f| f.body);
        }
    }
}

fn write_state(dir: &Path, port: u16, secret: &[u8; 32], expires: u64) {
    let web = dir.join("web").display().to_string().replace('\\', "/");
    let state = format!(
        r#"{{"port": {port}, "sessions": [], "pairing": {{"secret": "{}", "expires": {expires}}}, "web": "{web}"}}"#,
        hex(secret)
    );
    std::fs::write(dir.join("state.json"), state).unwrap();
}

#[test]
fn pairing_resuming_and_revoking_over_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let desktop = Keys::generate().unwrap();
    let mut key = desktop.private().to_vec();
    key.extend_from_slice(&desktop.public);
    std::fs::write(dir.join("desktop.key"), key).unwrap();
    std::fs::create_dir_all(dir.join("web")).unwrap();
    std::fs::write(dir.join("web/index.html"), "<p>web client</p>").unwrap();
    let port = free_port();
    let secret = [9u8; 32];
    let later = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    write_state(dir, port, &secret, later);
    start(dir.to_path_buf());
    wait_for(port);

    // the web client's files, nothing outside them
    let page = get(port, "/");
    assert!(
        page.starts_with("HTTP/1.1 200") && page.ends_with("<p>web client</p>"),
        "{page}"
    );
    assert!(get(port, "/../state.json").starts_with("HTTP/1.1 404"));

    // pairing: the code's secret, the desktop's key
    let phone = Keys::generate().unwrap();
    let ticket = Ticket {
        desktop: desktop.public,
        secret,
    };
    let mut socket = ws(port, "/ws/pair");
    let mut channel = shake(
        &mut socket,
        Handshake::pair_device(&phone, &ticket).unwrap(),
    )
    .expect("paired");
    send(&mut socket, &mut channel, &wire::hello("my phone"));
    assert!(matches!(
        receive(&mut socket, &mut channel),
        Some(Body::Welcome(_))
    ));
    assert!(
        matches!(receive(&mut socket, &mut channel), Some(Body::Sessions(s)) if s.sessions.is_empty())
    );
    let record = dir
        .join("devices")
        .join(format!("{}.json", hex(&phone.public)));
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(text.contains("\"my phone\""), "{text}");
    drop(socket);

    // the code pairs one device only
    let mut socket = ws(port, "/ws/pair");
    let second = Keys::generate().unwrap();
    if let Some(mut channel) = shake(
        &mut socket,
        Handshake::pair_device(&second, &ticket).unwrap(),
    ) {
        let _ = channel
            .seal(&wire::encode(&wire::hello("second")))
            .map(|ms| {
                for m in ms {
                    let _ = socket.send(Message::Binary(m.into()));
                }
            });
        assert!(receive(&mut socket, &mut channel).is_none(), "no Welcome");
    }
    let second_record = dir
        .join("devices")
        .join(format!("{}.json", hex(&second.public)));
    assert!(!second_record.exists());

    // a wrong secret does not pair. The secret is checked in the device's
    // last handshake message, so the device finishes its side; the desktop
    // refuses that message: no Welcome comes, and nothing is recorded.
    let wrong = Ticket {
        secret: [1u8; 32],
        ..ticket.clone()
    };
    let intruder = Keys::generate().unwrap();
    let mut socket = ws(port, "/ws/pair");
    if let Some(mut channel) = shake(
        &mut socket,
        Handshake::pair_device(&intruder, &wrong).unwrap(),
    ) {
        let _ = channel
            .seal(&wire::encode(&wire::hello("intruder")))
            .map(|ms| {
                for m in ms {
                    let _ = socket.send(Message::Binary(m.into()));
                }
            });
        assert!(receive(&mut socket, &mut channel).is_none(), "no Welcome");
    }
    let intruder_record = dir
        .join("devices")
        .join(format!("{}.json", hex(&intruder.public)));
    assert!(!intruder_record.exists());

    // resuming: the paired phone yes, a stranger no
    let mut socket = ws(port, "/ws/resume");
    let mut channel = shake(
        &mut socket,
        Handshake::resume_device(&phone, &desktop.public).unwrap(),
    )
    .expect("resumed");
    send(&mut socket, &mut channel, &wire::hello("my phone"));
    assert!(matches!(
        receive(&mut socket, &mut channel),
        Some(Body::Welcome(_))
    ));
    let mut other = ws(port, "/ws/resume");
    let stranger = Keys::generate().unwrap();
    assert!(shake(
        &mut other,
        Handshake::resume_device(&stranger, &desktop.public).unwrap()
    )
    .is_none());

    // revoked (NativeTerm removes the file): the open connection ends
    let _ = receive(&mut socket, &mut channel); // the sessions list
    std::fs::remove_file(&record).unwrap();
    let start = Instant::now();
    assert!(receive(&mut socket, &mut channel).is_none(), "cut off");
    assert!(start.elapsed() < Duration::from_secs(3));

    // an expired pairing does not pair
    write_state(dir, port, &secret, 1);
    std::thread::sleep(Duration::from_millis(50));
    let mut socket = ws(port, "/ws/pair");
    assert!(shake(
        &mut socket,
        Handshake::pair_device(&Keys::generate().unwrap(), &ticket).unwrap()
    )
    .is_none());
}
