//! The terminal's agent for NativeTerm's remote control (NativeTerm's
//! docs/REMOTE.md, "The control plane and the terminal"): remote control
//! runs in NativeTerm; what only the terminal has is asked of it here, on
//! NativeTerm's pipe (`Role::Terminal`): a pane described, its screen
//! since a number (this mirror numbers it), a device's input typed, a
//! hold at a device's size. It says, unasked, when a pane printed
//! something, and what happened there a person away may want to know of
//! (the bell, a program's notification, a command done). Connected for as long as the terminal runs; NativeTerm not
//! there (not started yet, started again): tried again every 2 seconds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mux::pane::PaneId;
use mux::{Mux, MuxNotification};
use native_term_remote::proto::frame::Body;
use native_term_remote::proto::input_result::Reason;
use native_term_remote::{b64, unb64, wire};
use native_term_session::pipe::{self, PipeConnection};
use native_term_session::protocol::{AgentPane, AppMessage, PaneEvent, Role, ShimMessage};
use wezterm_term::Alert;

use crate::{PaneMirror, PaneSource};

/// Serve NativeTerm's remote control from this terminal's panes, on a
/// thread of its own, for as long as the program runs.
pub fn start() {
    let spawned = std::thread::Builder::new()
        .name("nativeterm-agent".into())
        .spawn(|| loop {
            if let Err(e) = serve() {
                log::debug!("nativeterm agent: {e:#}");
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    if let Err(e) = spawned {
        log::error!("nativeterm agent: {e}");
    }
}

/// One connection to NativeTerm, until it goes.
fn serve() -> anyhow::Result<()> {
    let name = pipe::pipe_name()?;
    let conn = Arc::new(pipe::connect(&name, Duration::from_secs(1))?);
    conn.send(&ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Terminal,
        pid: std::process::id(),
        wt_session: None,
        session: None,
        alias: None,
        terminal_window: None,
    })?;
    log::info!("nativeterm agent: serving NativeTerm's remote control");
    let alive = Arc::new(AtomicBool::new(true));
    // a pane printed something: said at once (an echo waits on it), what
    // came meanwhile said with it; an event in a pane: said as it is
    let (changed, changes) = std::sync::mpsc::channel::<Option<(u64, PaneEvent)>>();
    if let Some(mux) = Mux::try_get() {
        let alive = Arc::clone(&alive);
        let changed = std::sync::Mutex::new(changed);
        mux.subscribe(move |n| {
            match n {
                MuxNotification::PaneOutput(_) | MuxNotification::PaneRemoved(_) => {
                    let _ = changed.lock().unwrap().send(None);
                }
                MuxNotification::Alert { pane_id, alert } => {
                    if let Some(event) = pane_event(alert) {
                        let _ = changed.lock().unwrap().send(Some((pane_id as u64, event)));
                    }
                }
                _ => {}
            }
            alive.load(Ordering::Relaxed)
        });
    }
    {
        let (alive, conn) = (Arc::clone(&alive), Arc::clone(&conn));
        std::thread::Builder::new()
            .name("nativeterm-agent-changed".into())
            .spawn(move || {
                while alive.load(Ordering::Relaxed) {
                    match changes.recv_timeout(Duration::from_secs(1)) {
                        Ok(first) => {
                            // the output said once, the events each
                            let mut printed = first.is_none();
                            let mut events: Vec<(u64, PaneEvent)> = first.into_iter().collect();
                            while let Ok(next) = changes.try_recv() {
                                printed |= next.is_none();
                                events.extend(next);
                            }
                            if printed && conn.send(&ShimMessage::AgentChanged).is_err() {
                                break;
                            }
                            for (pane, event) in events {
                                if conn.send(&ShimMessage::AgentEvent { pane, event }).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
    }
    let mut mirrors: HashMap<PaneId, PaneMirror> = HashMap::new();
    let ended = answer(&conn, &mut mirrors);
    alive.store(false, Ordering::Relaxed);
    conn.close();
    ended
}

/// What NativeTerm's remote control is told of an alert in a pane (the
/// rest of them are the terminal's own business).
fn pane_event(alert: Alert) -> Option<PaneEvent> {
    match alert {
        Alert::Bell => Some(PaneEvent::Bell),
        Alert::ToastNotification { title, body, .. } => Some(PaneEvent::Notification {
            title: title.unwrap_or_default(),
            body,
        }),
        Alert::CommandFinished { status, millis } => {
            Some(PaneEvent::CommandDone { status, millis })
        }
        _ => None,
    }
}

/// NativeTerm's questions, answered one by one until it goes.
fn answer(conn: &PipeConnection, mirrors: &mut HashMap<PaneId, PaneMirror>) -> anyhow::Result<()> {
    loop {
        let Some(message) = conn.recv::<AppMessage>(Duration::from_secs(60))? else {
            continue;
        };
        match message {
            AppMessage::AgentPane { req, pane } => {
                conn.send(&ShimMessage::AgentPane {
                    req,
                    pane: describe(pane),
                })?;
            }
            AppMessage::AgentScreen { req, pane, from } => {
                let since = Mux::try_get()
                    .and_then(|mux| mux.get_pane(pane as PaneId))
                    .map(|p| {
                        let mirror = mirrors.entry(pane as PaneId).or_default();
                        mirror.refresh(&mut PaneSource(&*p));
                        mirror.mirror().since(&pane.to_string(), from)
                    });
                let gone = since.is_none();
                if gone {
                    mirrors.remove(&(pane as PaneId));
                }
                let frames = since
                    .map(|s| {
                        s.into_frames()
                            .iter()
                            .map(|f| b64(&wire::encode(f)))
                            .collect()
                    })
                    .unwrap_or_default();
                conn.send(&ShimMessage::AgentScreen { req, gone, frames })?;
            }
            AppMessage::AgentInput { req, pane, input } => {
                let kind =
                    unb64(&input)
                        .and_then(|b| wire::decode(&b))
                        .and_then(|f| match f.body {
                            Some(Body::Input(i)) => i.kind,
                            _ => None,
                        });
                let reason = match (
                    kind,
                    Mux::try_get().and_then(|mux| mux.get_pane(pane as PaneId)),
                ) {
                    (Some(kind), Some(p)) => crate::input::apply(&*p, &kind),
                    (None, _) => Reason::UnknownKey,
                    (_, None) => Reason::NoSession,
                };
                conn.send(&ShimMessage::AgentInput {
                    req,
                    reason: reason as i32,
                })?;
            }
            AppMessage::AgentHold { pane, cols, lines } => hold_size(pane as PaneId, cols, lines),
            _ => {}
        }
    }
}

/// The pane as NativeTerm's list shows it: the tab's title where one was
/// set (NativeTerm's label), else what the program in it says.
fn describe(id: u64) -> Option<AgentPane> {
    let mux = Mux::try_get()?;
    let pane = mux.get_pane(id as PaneId)?;
    let dims = pane.get_dimensions();
    let tab_title = mux
        .resolve_pane_id(id as PaneId)
        .and_then(|(_, _, tab)| mux.get_tab(tab))
        .map(|tab| tab.get_title())
        .filter(|t| !t.is_empty());
    Some(AgentPane {
        id,
        title: tab_title.unwrap_or_else(|| pane.get_title()),
        cols: dims.cols as u32,
        lines: dims.viewport_rows as u32,
    })
}

/// Hold the pane at `cols` x `lines` (0 columns: give it back to its
/// window), on the GUI's thread, the window told to draw it again.
fn hold_size(pane: PaneId, cols: u32, lines: u32) {
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
