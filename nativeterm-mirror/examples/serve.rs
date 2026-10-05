//! Remote control's LAN service alone (no terminal, no sessions), for
//! trying clients against it: the web client's files, pairing, the
//! session, a direct connection. Prints the pairing link, then serves
//! until stopped; `connections.json` in the folder lists who is connected
//! and how.
//!
//!   serve <folder> <port> <web client's dist folder>

use std::time::{SystemTime, UNIX_EPOCH};

use native_term_remote::hex;
use native_term_remote::noise::{Keys, Ticket};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, port, web] = args.as_slice() else {
        anyhow::bail!("serve <folder> <port> <web client's dist folder>");
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let keys = Keys::generate()?;
    let mut key = keys.private().to_vec();
    key.extend_from_slice(&keys.public);
    std::fs::write(dir.join("desktop.key"), key)?;
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret)?;
    let expires = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 600;
    let state = serde_json::json!({
        "port": port.parse::<u16>()?,
        "sessions": [],
        "pairing": { "secret": hex(&secret), "expires": expires },
        "web": web,
    });
    std::fs::write(dir.join("state.json"), state.to_string())?;
    let ticket = Ticket {
        desktop: keys.public,
        secret,
    };
    println!("http://127.0.0.1:{port}/#pair={}", ticket.encode());
    nativeterm_mirror::server::start(dir);
    loop {
        std::thread::park();
    }
}
