//! A device on the command line, for trying remote control before the
//! web client: pairs with the link a pairing QR code says (or resumes,
//! with the key kept from pairing), lists the sessions open, watches the
//! first and prints its screen as it changes.
//!
//!   device pair '<link>' <key file> [name]
//!   device resume <host:port> <desktop key hex> <key file> [name]
//!
//! The key file holds the device's key pair (made on pairing).

use std::net::TcpStream;
use std::time::{Duration, Instant};

use native_term_remote::device::Device;
use native_term_remote::noise::{Channel, Handshake, Keys, Ticket};
use native_term_remote::proto::frame::Body;
use native_term_remote::{hex, unhex, wire};
use tungstenite::{Message, WebSocket};

type Ws = WebSocket<TcpStream>;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (address, desktop, keys, path, name, pairing) = match args.first().map(String::as_str) {
        Some("pair") if args.len() >= 3 => {
            let link = &args[1];
            let (base, fragment) = link
                .split_once("#pair=")
                .ok_or_else(|| anyhow::anyhow!("no #pair= in the link"))?;
            let ticket =
                Ticket::decode(fragment).ok_or_else(|| anyhow::anyhow!("not a pairing ticket"))?;
            let address = base
                .trim_start_matches("http://")
                .trim_end_matches('/')
                .to_string();
            let keys = Keys::generate()?;
            let mut stored = keys.private().to_vec();
            stored.extend_from_slice(&keys.public);
            std::fs::write(&args[2], stored)?;
            let name = args.get(3).cloned().unwrap_or_else(|| "device example".into());
            (address, ticket.desktop, keys, "/ws/pair", name, Some(ticket))
        }
        Some("resume") if args.len() >= 4 => {
            let desktop: [u8; 32] = unhex(&args[2])
                .and_then(|k| k.try_into().ok())
                .ok_or_else(|| anyhow::anyhow!("not a key"))?;
            let bytes = std::fs::read(&args[3])?;
            anyhow::ensure!(bytes.len() == 64, "not a key file");
            let keys = Keys::from_private(bytes[..32].try_into()?, bytes[32..].try_into()?);
            let name = args.get(4).cloned().unwrap_or_else(|| "device example".into());
            (args[1].clone(), desktop, keys, "/ws/resume", name, None)
        }
        _ => anyhow::bail!("device pair '<link>' <key file> [name] | device resume <host:port> <desktop key hex> <key file> [name]"),
    };
    let stream = TcpStream::connect(&address)?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    let (mut ws, _) = tungstenite::client(format!("ws://{address}{path}"), stream)?;
    let mut hs = match &pairing {
        Some(ticket) => Handshake::pair_device(&keys, ticket)?,
        None => Handshake::resume_device(&keys, &desktop)?,
    };
    while !hs.finished() {
        if hs.my_turn() {
            ws.send(Message::Binary(hs.write()?.into()))?;
        } else {
            hs.read(&binary(&mut ws)?)?;
        }
    }
    let (mut channel, _) = hs.channel()?;
    println!(
        "{} with desktop {}; this device is {}",
        if pairing.is_some() {
            "paired"
        } else {
            "resumed"
        },
        hex(&desktop),
        hex(&keys.public)
    );
    let mut device = Device::default();
    for frame in device.connected(&name) {
        send(&mut ws, &mut channel, &frame)?;
    }
    let started = Instant::now();
    let mut watching: Option<String> = None;
    let mut shown = 0u64;
    loop {
        let bytes = match binary(&mut ws) {
            Ok(b) => b,
            Err(e) => {
                println!("connection ended: {e}");
                return Ok(());
            }
        };
        let Some(plain) = channel.open(&bytes)? else {
            continue;
        };
        let Some(frame) = wire::decode(&plain) else {
            continue;
        };
        if let Some(Body::Sessions(list)) = &frame.body {
            println!(
                "sessions: {:?}",
                list.sessions
                    .iter()
                    .map(|s| (&s.id, &s.title))
                    .collect::<Vec<_>>()
            );
            if watching.is_none() {
                if let Some(first) = list.sessions.first() {
                    watching = Some(first.id.clone());
                    if let Some(f) = device.watch(&first.id) {
                        send(&mut ws, &mut channel, &f)?;
                    }
                }
            }
        }
        let welcomed = matches!(frame.body, Some(Body::Welcome(_)));
        for reply in device.receive(frame) {
            send(&mut ws, &mut channel, &reply)?;
        }
        if welcomed {
            println!("welcomed, protocol {:?}", device.version());
        }
        for ended in device.take_ended() {
            println!("ended: {ended}");
        }
        if let Some(copy) = watching.as_deref().and_then(|s| device.screen(s)) {
            if copy.seq != shown && copy.lines > 0 {
                shown = copy.seq;
                println!(
                    "--- {:.1}s seq {} {}x{} \"{}\"",
                    started.elapsed().as_secs_f32(),
                    copy.seq,
                    copy.cols,
                    copy.lines,
                    copy.title
                );
                for i in copy.top..copy.top + u64::from(copy.lines) {
                    let text = copy.text(i).unwrap_or_default();
                    if !text.trim().is_empty() {
                        println!("{text}");
                    }
                }
            }
        }
    }
}

fn binary(ws: &mut Ws) -> anyhow::Result<Vec<u8>> {
    loop {
        match ws.read()? {
            Message::Binary(b) => return Ok(b.to_vec()),
            Message::Close(_) => anyhow::bail!("closed"),
            _ => {}
        }
    }
}

fn send(
    ws: &mut Ws,
    channel: &mut Channel,
    frame: &native_term_remote::proto::Frame,
) -> anyhow::Result<()> {
    for m in channel.seal(&wire::encode(frame))? {
        ws.send(Message::Binary(m.into()))?;
    }
    Ok(())
}
