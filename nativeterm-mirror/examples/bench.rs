//! Remote control measured from a device's side (docs/REMOTE.md, R1's
//! measurements): connecting (TCP, WebSocket, the Noise handshake), the
//! first screen (its size and time), and typing: a character sent until
//! the row that echoes it comes back, with the bytes that cost. The
//! session watched should be an idle shell: each character is typed and
//! then taken back with Backspace.
//!
//!   bench pair <ws-base> '<link>' <key file> [count]
//!   bench resume <ws-base> <desktop key hex> <key file> [count]
//!
//! `ws-base` is where `pair` / `resume` are appended: `ws://host:47100/ws/`
//! on the desktop's network, `ws://relay:8443/device/<room>/` through the
//! relay.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use native_term_remote::device::Device;
use native_term_remote::noise::{Channel, Handshake, Keys, Ticket};
use native_term_remote::proto::frame::Body;
use native_term_remote::{unhex, wire};
use tungstenite::{Message, WebSocket};

type Ws = WebSocket<TcpStream>;

/// What came in: plain frames, and the bytes on the wire for them.
struct Link {
    ws: Ws,
    channel: Channel,
    received: usize,
    sent: usize,
}

impl Link {
    fn send(&mut self, frame: &native_term_remote::proto::Frame) -> anyhow::Result<()> {
        for m in self.channel.seal(&wire::encode(frame))? {
            self.sent += m.len();
            self.ws.send(Message::Binary(m.into()))?;
        }
        Ok(())
    }

    fn frame(&mut self) -> anyhow::Result<native_term_remote::proto::Frame> {
        loop {
            if let Message::Binary(b) = self.ws.read()? {
                self.received += b.len();
                if let Some(plain) = self.channel.open(&b)? {
                    if let Some(frame) = wire::decode(&plain) {
                        return Ok(frame);
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
            Message::Close(f) => anyhow::bail!("closed: {f:?}"),
            _ => {}
        }
    }
}

fn stats(name: &str, mut values: Vec<f64>, unit: &str) {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name}: median {:.1} {unit}, p90 {:.1}, min {:.1}, max {:.1} ({} samples)",
        at(0.5),
        at(0.9),
        values[0],
        values[values.len() - 1],
        values.len()
    );
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() >= 4,
        "bench pair|resume <ws-base> <link|desktop key> <key file> [count]"
    );
    let (mode, base, what, key_file) = (&args[0], &args[1], &args[2], &args[3]);
    let count: usize = args.get(4).and_then(|c| c.parse().ok()).unwrap_or(30);
    let (keys, desktop, ticket) = if mode == "pair" {
        let fragment = what.split_once("#pair=").map_or(what.as_str(), |(_, f)| f);
        let ticket = Ticket::decode(fragment.split('&').next().unwrap_or(""))
            .ok_or_else(|| anyhow::anyhow!("not a ticket"))?;
        let keys = Keys::generate()?;
        let mut stored = keys.private().to_vec();
        stored.extend_from_slice(&keys.public);
        std::fs::write(key_file, stored)?;
        (keys, ticket.desktop, Some(ticket))
    } else {
        let bytes = std::fs::read(key_file)?;
        let keys = Keys::from_private(bytes[..32].try_into()?, bytes[32..].try_into()?);
        let desktop: [u8; 32] = unhex(what)
            .and_then(|k| k.try_into().ok())
            .ok_or_else(|| anyhow::anyhow!("not a key"))?;
        (keys, desktop, None)
    };

    // connecting
    let started = Instant::now();
    let url = format!("{base}{}", if ticket.is_some() { "pair" } else { "resume" });
    let request = tungstenite::client::IntoClientRequest::into_client_request(url.as_str())?;
    let host = request
        .uri()
        .host()
        .unwrap_or("")
        .trim_matches(['[', ']'])
        .to_string();
    let port = request.uri().port_u16().unwrap_or(80);
    let stream = TcpStream::connect((host.as_str(), port))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let tcp = started.elapsed();
    let (mut ws, _) = tungstenite::client(request, stream).map_err(|e| anyhow::anyhow!("{e}"))?;
    let websocket = started.elapsed();
    let mut hs = match &ticket {
        Some(t) => Handshake::pair_device(&keys, t)?,
        None => Handshake::resume_device(&keys, &desktop)?,
    };
    while !hs.finished() {
        if hs.my_turn() {
            ws.send(Message::Binary(hs.write()?.into()))?;
        } else {
            hs.read(&binary(&mut ws)?)?;
        }
    }
    let (channel, _) = hs.channel()?;
    let mut link = Link {
        ws,
        channel,
        received: 0,
        sent: 0,
    };
    let mut device = Device::default();
    for f in device.connected("bench") {
        link.send(&f)?;
    }
    let session = loop {
        let frame = link.frame()?;
        if let Some(Body::Sessions(s)) = &frame.body {
            if let Some(first) = s.sessions.first() {
                break first.id.clone();
            }
        }
        device.receive(frame);
    };
    let welcomed = started.elapsed();
    println!(
        "connect: TCP {:.1} ms, WebSocket {:.1} ms, welcomed with sessions {:.1} ms",
        tcp.as_secs_f64() * 1e3,
        websocket.as_secs_f64() * 1e3,
        welcomed.as_secs_f64() * 1e3
    );

    // the first screen
    let before = link.received;
    let asked = Instant::now();
    if let Some(f) = device.watch(&session) {
        link.send(&f)?;
    }
    loop {
        let frame = link.frame()?;
        device.receive(frame);
        if device.screen(&session).is_some_and(|c| c.lines > 0) {
            break;
        }
    }
    let copy = device.screen(&session).unwrap();
    println!(
        "first screen: {}x{} with {} rows kept, {:.1} ms, {} bytes on the wire",
        copy.cols,
        copy.lines,
        copy.row_count(),
        asked.elapsed().as_secs_f64() * 1e3,
        link.received - before
    );

    // typing: a character until the row echoing it comes back
    let mut latencies = Vec::new();
    let mut bytes_in = Vec::new();
    let mut bytes_out = Vec::new();
    for i in 0..count {
        let seq = device.screen(&session).map_or(0, |c| c.seq);
        let (r0, s0) = (link.received, link.sent);
        let t = Instant::now();
        let c = char::from(b'a' + (i % 26) as u8).to_string();
        link.send(
            &device
                .text(&session, &c)
                .map_err(|_| anyhow::anyhow!("not connected"))?,
        )?;
        loop {
            let frame = link.frame()?;
            device.receive(frame);
            if device.screen(&session).is_some_and(|c| c.seq != seq) {
                break;
            }
        }
        latencies.push(t.elapsed().as_secs_f64() * 1e3);
        bytes_in.push((link.received - r0) as f64);
        bytes_out.push((link.sent - s0) as f64);
        // taken back, and its echo waited for
        let seq = device.screen(&session).map_or(0, |c| c.seq);
        link.send(
            &device
                .key(&session, "Backspace", 0)
                .map_err(|_| anyhow::anyhow!("not connected"))?,
        )?;
        loop {
            let frame = link.frame()?;
            device.receive(frame);
            if device.screen(&session).is_some_and(|c| c.seq != seq) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stats("keystroke to echo", latencies, "ms");
    stats("bytes in per keystroke (echo, input result)", bytes_in, "B");
    stats("bytes out per keystroke", bytes_out, "B");
    Ok(())
}
