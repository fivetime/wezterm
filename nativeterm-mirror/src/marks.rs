//! The tab strip's remote-control mark for a GUI whose panes live in the
//! mux server (NativeTerm's R3): the service runs there, with the panes,
//! so the GUI reads `state.json` itself. Its sessions are the server's
//! pane ids; the GUI maps its own panes to them (a client pane knows the
//! server's id of the pane it shows), and writes that map for NativeTerm
//! (`gui-panes.json`: the GUI's pane id to the server's), which knows a
//! tab by the GUI's ids and opens sessions by the server's.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use mux::pane::PaneId;
use mux::tab::TabId;
use mux::{Mux, MuxNotification};

use crate::server::State;

/// The server's pane ids open to remote control, as last read.
static OPEN: Mutex<Option<HashSet<PaneId>>> = Mutex::new(None);
/// The server's pane ids whose programs are kept on the server they are
/// logged into (tmux), as last read.
static KEPT: Mutex<Option<HashSet<PaneId>>> = Mutex::new(None);

/// Whether the server's pane `server_pane` runs in tmux on its server.
pub fn is_kept(server_pane: PaneId) -> bool {
    KEPT.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|kept| kept.contains(&server_pane))
}

/// Whether the server's pane `server_pane` is open to remote control.
pub fn is_open(server_pane: PaneId) -> bool {
    OPEN.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|open| open.contains(&server_pane))
}

/// Reads `dir`'s `state.json` while the GUI runs, and has the tabs whose
/// panes were opened or closed draw their titles again; `tab_of` gives the
/// GUI's tab showing the server's pane.
pub fn watch(
    dir: PathBuf,
    tab_of: impl Fn(PaneId) -> Option<TabId> + Send + 'static,
    pairs: impl Fn() -> Vec<(PaneId, PaneId)> + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("nativeterm-marks".into())
        .spawn(move || {
            let path = dir.join("state.json");
            let mut seen: Option<SystemTime> = None;
            let mut before: HashSet<PaneId> = HashSet::new();
            let mut written: Option<std::collections::BTreeMap<PaneId, PaneId>> = None;
            loop {
                // the GUI's panes and the server's, for NativeTerm
                let map: std::collections::BTreeMap<PaneId, PaneId> = pairs().into_iter().collect();
                if written.as_ref() != Some(&map) && write_map(&dir, &map).is_ok() {
                    written = Some(map);
                }
                let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if modified != seen {
                    seen = modified;
                    let state: State = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or_default();
                    *KEPT.lock().unwrap() = Some(state.kept.iter().copied().collect());
                    let now: HashSet<PaneId> = state.sessions.into_iter().collect();
                    *OPEN.lock().unwrap() = Some(now.clone());
                    if let Some(mux) = Mux::try_get() {
                        for pane in now.symmetric_difference(&before) {
                            if let Some(tab) = tab_of(*pane).and_then(|t| mux.get_tab(t)) {
                                Mux::notify_from_any_thread(MuxNotification::TabTitleChanged {
                                    tab_id: tab.tab_id(),
                                    title: tab.get_title(),
                                });
                            }
                        }
                    }
                    before = now;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
    if let Err(e) = spawned {
        log::error!("nativeterm remote control marks: {e}");
    }
}

/// `gui-panes.json` written whole, then renamed into place.
fn write_map(
    dir: &std::path::Path,
    map: &std::collections::BTreeMap<PaneId, PaneId>,
) -> std::io::Result<()> {
    let text = serde_json::to_string(
        &map.iter()
            .map(|(gui, server)| (gui.to_string(), *server))
            .collect::<std::collections::BTreeMap<String, PaneId>>(),
    )?;
    let path = dir.join("gui-panes.json");
    let new = dir.join("gui-panes.json.new");
    std::fs::write(&new, text)?;
    std::fs::rename(new, path)
}
