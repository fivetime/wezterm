//! Remote control through the person's relay (NativeTerm's
//! `nativeterm-server`, run here in the test): the desktop holds its room
//! there as `state.json` says; a device that never reaches the desktop
//! pairs, is welcomed, resumes; a stranger is told it is not paired.

use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use native_term_remote::noise::{Channel, Handshake, Keys, Ticket};
use native_term_remote::proto::frame::Body;
use native_term_remote::wire;
use native_term_server::relay::{Relay, Stream};
use nativeterm_mirror::server::{hex, start};
use tungstenite::{Message, WebSocket};

type Ws = WebSocket<TcpStream>;
const TOKEN: &str = "a-token-of-sixteen-or-more";
const ROOM: &str = "0123456789abcdef0123456789abcdef";

fn relay() -> u16 {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        rt.block_on(async move {
            let relay = Relay::new(TOKEN.to_string());
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let relay = Arc::clone(&relay);
                tokio::spawn(async move { relay.serve(Box::new(stream) as Box<dyn Stream>).await });
            }
        })
    });
    port
}

fn device(port: u16, path: &str) -> Ws {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (ws, _) = tungstenite::client(
        format!("ws://127.0.0.1:{port}/device/{ROOM}/{path}"),
        stream,
    )
    .unwrap();
    ws
}

fn binary(ws: &mut Ws) -> Result<Vec<u8>, Option<u16>> {
    loop {
        match ws.read() {
            Ok(Message::Binary(b)) => return Ok(b.to_vec()),
            Ok(Message::Close(f)) => return Err(f.map(|f| u16::from(f.code))),
            Ok(_) => {}
            Err(_) => return Err(None),
        }
    }
}

fn shake(ws: &mut Ws, mut hs: Handshake) -> Result<Channel, Option<u16>> {
    while !hs.finished() {
        if hs.my_turn() {
            ws.send(Message::Binary(hs.write().unwrap().into()))
                .unwrap();
        } else {
            hs.read(&binary(ws)?).map_err(|_| None)?;
        }
    }
    Ok(hs.channel().unwrap().0)
}

fn welcomed(ws: &mut Ws, channel: &mut Channel) -> bool {
    for m in channel.seal(&wire::encode(&wire::hello("phone"))).unwrap() {
        ws.send(Message::Binary(m.into())).unwrap();
    }
    let Ok(bytes) = binary(ws) else { return false };
    let frame = channel.open(&bytes).unwrap().unwrap();
    matches!(wire::decode(&frame).unwrap().body, Some(Body::Welcome(_)))
}

#[test]
fn pairing_and_resuming_through_the_relay() {
    let port = relay();
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let desktop = Keys::generate().unwrap();
    let mut key = desktop.private().to_vec();
    key.extend_from_slice(&desktop.public);
    std::fs::write(dir.join("desktop.key"), key).unwrap();
    let secret = [4u8; 32];
    let later = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    // the LAN port is one nothing here uses: the device goes by the relay
    let lan = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let state = format!(
        r#"{{"port": {lan}, "listen": "127.0.0.1", "sessions": [], "pairing": {{"secret": "{}", "expires": {later}}},
            "relay": {{"url": "ws://127.0.0.1:{port}", "token": "{TOKEN}", "room": "{ROOM}"}}}}"#,
        hex(&secret)
    );
    std::fs::write(dir.join("state.json"), state).unwrap();
    start(dir.to_path_buf());

    // the room is held once the desktop has reached the relay
    let phone = Keys::generate().unwrap();
    let ticket = Ticket {
        desktop: desktop.public,
        secret,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut paired, _channel) = loop {
        let mut ws = device(port, "pair");
        match shake(&mut ws, Handshake::pair_device(&phone, &ticket).unwrap()) {
            Ok(mut channel) => {
                assert!(welcomed(&mut ws, &mut channel), "paired through the relay");
                break (ws, channel);
            }
            Err(Some(4404)) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200))
            }
            Err(e) => panic!("{e:?}"),
        }
    };
    let _ = paired.close(None);
    assert!(dir
        .join("devices")
        .join(format!("{}.json", hex(&phone.public)))
        .exists());

    // resuming through it
    let mut ws = device(port, "resume");
    let mut channel = shake(
        &mut ws,
        Handshake::resume_device(&phone, &desktop.public).unwrap(),
    )
    .expect("resumed");
    assert!(welcomed(&mut ws, &mut channel));

    // a stranger: told it is not paired, through the relay
    let mut ws = device(port, "resume");
    let mut hs = Handshake::resume_device(&Keys::generate().unwrap(), &desktop.public).unwrap();
    ws.send(Message::Binary(hs.write().unwrap().into()))
        .unwrap();
    assert_eq!(binary(&mut ws), Err(Some(wire::CLOSE_NOT_PAIRED)));
}
