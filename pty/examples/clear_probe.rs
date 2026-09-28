//! What a pty that keeps a picture of the screen of its own (ConPTY)
//! writes to the terminal when it is told the screen was cleared
//! (`MasterPty::clear_screen`), and where what the program writes next
//! goes: a shell is started, a command typed, the screen cleared, another
//! command typed; everything the pty wrote is printed, the escape
//! sequences spelled out.
//!
//!   cargo run -p portable-pty --example clear_probe [shell]
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

fn spelled(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in String::from_utf8_lossy(bytes).chars() {
        match c {
            '\x1b' => out.push_str("<ESC>"),
            '\r' => out.push_str("<CR>"),
            '\n' => out.push_str("<LF>\n      "),
            '\x0c' => out.push_str("<FF>"),
            c if c.is_control() => out.push_str(&format!("<{:02x}>", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Everything written until nothing more comes for `wait` milliseconds.
/// A terminal answers what ConPTY asks at its start (where the cursor
/// is: at the screen's start).
fn written(rx: &Receiver<Vec<u8>>, writer: &mut dyn Write, title: &str, wait: u64) {
    let mut all = Vec::new();
    while let Ok(bytes) = rx.recv_timeout(Duration::from_millis(wait)) {
        if bytes.windows(4).any(|w| w == b"\x1b[6n") {
            let _ = writer.write_all(b"\x1b[1;1R");
            let _ = writer.flush();
        }
        // (and what kind of terminal this is)
        if bytes.windows(3).any(|w| w == b"\x1b[c") {
            let _ = writer.write_all(b"\x1b[?61;4;6;7;14;21;22;23;24;28;32;42c");
            let _ = writer.flush();
        }
        all.extend(bytes);
    }
    println!("--- {title}\n      {}", spelled(&all));
}

fn main() {
    let shell = std::env::args().nth(1).unwrap_or_else(|| {
        if cfg!(windows) {
            "cmd.exe".to_string()
        } else {
            "sh".to_string()
        }
    });
    let pair = NativePtySystem::default()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut child = pair
        .slave
        .spawn_command(CommandBuilder::new(shell))
        .unwrap();
    drop(pair.slave);

    let (tx, rx) = channel::<Vec<u8>>();
    let mut reader = pair.master.try_clone_reader().unwrap();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 || tx.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut writer = pair.master.take_writer().unwrap();
    written(&rx, &mut writer, "the shell starts", 1500);
    writer.write_all(b"echo before\r").unwrap();
    writer.flush().unwrap();
    written(&rx, &mut writer, "a command", 1000);
    match std::env::var("NT_PROBE_TYPE") {
        // (something typed instead: the shell's own way to clear)
        Ok(typed) => {
            let typed = typed
                .replace("<CR>", &char::from(13u8).to_string())
                .replace("<FF>", &char::from(12u8).to_string());
            writer.write_all(typed.as_bytes()).unwrap();
            writer.flush().unwrap();
            written(
                &rx,
                &mut writer,
                "what was typed instead, and what came of it",
                1500,
            );
        }
        Err(_) => {
            println!("--- the screen cleared: {:?}", pair.master.clear_screen());
            written(&rx, &mut writer, "what the pty wrote for it", 1000);
        }
    }
    writer.write_all(b"echo after\r").unwrap();
    writer.flush().unwrap();
    written(&rx, &mut writer, "a command after it", 1000);
    child.kill().ok();
}
