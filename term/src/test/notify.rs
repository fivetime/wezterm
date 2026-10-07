//! What a pane tells of itself: a command done (OSC 133 `C`, then `D`)
//! and kitty's notifications (OSC 99).
use super::*;
use crate::terminal::{Alert, AlertHandler};
use std::sync::Mutex;

struct Collect(Arc<Mutex<Vec<Alert>>>);

impl AlertHandler for Collect {
    fn alert(&mut self, alert: Alert) {
        self.0.lock().unwrap().push(alert);
    }
}

fn term_with_alerts() -> (TestTerm, Arc<Mutex<Vec<Alert>>>) {
    let mut term = TestTerm::new(5, 20, 0);
    let alerts = Arc::new(Mutex::new(Vec::new()));
    term.term
        .set_notification_handler(Box::new(Collect(Arc::clone(&alerts))));
    (term, alerts)
}

fn toasts(alerts: &Mutex<Vec<Alert>>) -> Vec<(Option<String>, String)> {
    alerts
        .lock()
        .unwrap()
        .iter()
        .filter_map(|a| match a {
            Alert::ToastNotification { title, body, .. } => Some((title.clone(), body.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_command_done_after_it_began() {
    let (mut term, alerts) = term_with_alerts();
    // a prompt that ran nothing: no command
    term.print("\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;D;0\x07");
    term.print("\x1b]133;A\x07$ \x1b]133;B\x07make\r\n\x1b]133;C\x07output\r\n\x1b]133;D;2\x07");
    let done: Vec<i32> = alerts
        .lock()
        .unwrap()
        .iter()
        .filter_map(|a| match a {
            Alert::CommandFinished { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    std::assert_eq!(done, vec![2]);
}

#[test]
fn kitty_notifications() {
    let (mut term, alerts) = term_with_alerts();
    // a title alone
    term.print("\x1b]99;;Hello world\x1b\\");
    // in chunks, by identifier, the text with a `;` in it
    term.print("\x1b]99;i=1:d=0;Build\x1b\\");
    term.print("\x1b]99;i=1:d=0:p=body;done; 3 \x1b\\");
    term.print("\x1b]99;i=1:p=body;warnings\x1b\\");
    // base64; a query and an icon are not shown
    term.print("\x1b]99;e=1:p=body;aGkgdGhlcmU=\x1b\\");
    term.print("\x1b]99;i=2:p=?;\x1b\\");
    term.print("\x1b]99;i=3:p=icon;x\x1b\\");
    std::assert_eq!(
        toasts(&alerts),
        vec![
            (None, "Hello world".to_string()),
            (Some("Build".to_string()), "done; 3 warnings".to_string()),
            (None, "hi there".to_string()),
        ]
    );
}
