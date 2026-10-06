use super::confirm;
use crate::TermWindow;
use mux::pane::PaneId;
use mux::tab::TabId;
use mux::termwiztermtab::TermWizTerminal;
use mux::window::WindowId;
use mux::Mux;

pub fn confirm_close_pane(
    pane_id: PaneId,
    mut term: TermWizTerminal,
    mux_window_id: WindowId,
    window: ::window::Window,
) -> anyhow::Result<()> {
    if confirm::run_confirmation("🛑 Really kill this pane?", &mut term)? {
        promise::spawn::spawn_into_main_thread(async move {
            let mux = Mux::get();
            let tab = match mux.get_active_tab_for_window(mux_window_id) {
                Some(tab) => tab,
                None => return,
            };
            tab.kill_pane(pane_id);
        })
        .detach();
    }
    TermWindow::schedule_cancel_overlay_for_pane(window, pane_id);

    Ok(())
}

pub fn confirm_close_tab(
    tab_id: TabId,
    mut term: TermWizTerminal,
    _mux_window_id: WindowId,
    window: ::window::Window,
) -> anyhow::Result<()> {
    if confirm::run_confirmation(
        "🛑 Really kill this tab and all contained panes?",
        &mut term,
    )? {
        promise::spawn::spawn_into_main_thread(async move {
            let mux = Mux::get();
            mux.remove_tab(tab_id);
        })
        .detach();
    }
    TermWindow::schedule_cancel_overlay(window, tab_id, None);

    Ok(())
}

/// Closing a tab that remote control is on for (NativeTerm), in a GUI
/// whose panes live in the mux server: detach it (its panes go on in the
/// server, the device keeps them) or close it (the device is
/// disconnected). Detach is the default.
pub fn confirm_detach_or_close_tab(
    tab_id: TabId,
    kept: bool,
    mut term: TermWizTerminal,
    window: ::window::Window,
) -> anyhow::Result<()> {
    let message = if kept {
        "📱 A device may be using this tab (remote control is on).\n\n\
         Detach: the tab leaves this window and its connection goes on; the \
         device keeps it, and NativeTerm can bring it back.\n\
         Close: the connection ends and the device is disconnected; the \
         programs stay in tmux on the server, to be reopened."
    } else {
        "📱 A device may be using this tab (remote control is on).\n\n\
         Detach: the tab leaves this window and its programs go on; the \
         device keeps it, and NativeTerm can bring it back.\n\
         Close: its programs end and the device is disconnected."
    };
    let choice = confirm::run_choice(
        message,
        &[
            ('d', "[D]etach"),
            ('c', "[C]lose"),
            ('\u{1b}', "Cancel (Esc)"),
        ],
        &mut term,
    )?;
    match choice {
        // (the moves are the main thread's, and their futures not Send)
        Some(0) => promise::spawn::spawn_into_main_thread(async move {
            promise::spawn::spawn(async move {
                if let Err(e) = crate::detach_tab(tab_id).await {
                    log::error!("detaching tab {tab_id}: {e:#}");
                }
            })
            .detach();
        })
        .detach(),
        Some(1) => promise::spawn::spawn_into_main_thread(async move {
            Mux::get().remove_tab(tab_id);
        })
        .detach(),
        _ => {}
    }
    TermWindow::schedule_cancel_overlay(window, tab_id, None);
    Ok(())
}

/// Closing a tab that remote control is on for, which can't be detached
/// (its panes are this GUI's own): the device is disconnected.
pub fn confirm_close_remote_tab(
    tab_id: TabId,
    kept: bool,
    mut term: TermWizTerminal,
    window: ::window::Window,
) -> anyhow::Result<()> {
    let message = if kept {
        "📱 A device may be using this tab (remote control is on). Closing it \
         disconnects the device; the programs stay in tmux on the server, to \
         be reopened. Close it?"
    } else {
        "📱 A device may be using this tab (remote control is on). Closing it \
         ends its programs and disconnects the device. Close it?"
    };
    if confirm::run_confirmation(message, &mut term)? {
        promise::spawn::spawn_into_main_thread(async move {
            Mux::get().remove_tab(tab_id);
        })
        .detach();
    }
    TermWindow::schedule_cancel_overlay(window, tab_id, None);
    Ok(())
}

pub fn confirm_close_window(
    mut term: TermWizTerminal,
    mux_window_id: WindowId,
    window: ::window::Window,
    tab_id: TabId,
) -> anyhow::Result<()> {
    if confirm::run_confirmation(
        "🛑 Really kill this window and all contained tabs and panes?",
        &mut term,
    )? {
        promise::spawn::spawn_into_main_thread(async move {
            let mux = Mux::get();
            mux.kill_window(mux_window_id);
        })
        .detach();
    }
    TermWindow::schedule_cancel_overlay(window, tab_id, None);

    Ok(())
}

pub fn confirm_quit_program(
    mut term: TermWizTerminal,
    window: ::window::Window,
    tab_id: TabId,
) -> anyhow::Result<()> {
    if confirm::run_confirmation("🛑 Really Quit WezTerm?", &mut term)? {
        promise::spawn::spawn_into_main_thread(async move {
            use ::window::{Connection, ConnectionOps};
            let con = Connection::get().expect("call on gui thread");
            con.terminate_message_loop();
        })
        .detach();
    }
    TermWindow::schedule_cancel_overlay(window, tab_id, None);

    Ok(())
}
