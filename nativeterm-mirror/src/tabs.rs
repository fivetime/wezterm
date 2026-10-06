//! Tabs from a device (NativeTerm's docs/REMOTE.md, "Tabs from a
//! device"): a device's `Request`s that only NativeTerm can do (a tab
//! opened for a host, closed, detached, brought back) are handed over in
//! remote control's folder (`requests/<connection>-<id>.json`) and
//! answered there (`results/…`); the lists NativeTerm keeps there
//! (`hosts.json`, `resumable.json`) are sent as they are.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use mux::pane::PaneId;
use native_term_remote::proto::frame::Body;
use native_term_remote::proto::request::Kind;
use native_term_remote::proto::resumable_info;
use native_term_remote::proto::{
    Frame, HostInfo, Hosts, Request, RequestResult, Resumable, ResumableInfo,
};
use native_term_remote::wire;
use serde::{Deserialize, Serialize};

const REQUESTS: &str = "requests";
const RESULTS: &str = "results";
const HOSTS: &str = "hosts.json";
const RESUMABLE: &str = "resumable.json";

/// How long NativeTerm may take to answer (opening a tab waits for it).
const ANSWER_WITHIN: Duration = Duration::from_secs(60);

/// A request as NativeTerm reads it.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct Handed<'a> {
    conn: u64,
    id: u64,
    device: &'a str,
    #[serde(flatten)]
    what: What,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum What {
    Open { host: String },
    Close { session: String, detach: bool },
    // (`tab`: the request has an `id` of its own)
    Resume { tab: String },
}

/// NativeTerm's answer.
#[derive(Debug, Default, Deserialize)]
struct Answer {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    session: String,
    #[serde(default)]
    code: String,
}

#[derive(Debug, Deserialize)]
struct HostEntry {
    alias: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    kept: bool,
}

#[derive(Debug, Deserialize)]
struct ResumableEntry {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    kind: String,
}

/// A connection's requests handed over, and whether its device follows
/// the tabs that can be brought back.
#[derive(Default)]
pub(crate) struct Tabs {
    waiting: Vec<(String, u64, Instant)>,
    /// `resumable.json`'s time when last sent; `None` until asked.
    resumable_sent: Option<Option<SystemTime>>,
}

fn result(id: u64, ok: bool, error: impl Into<String>, session: impl Into<String>) -> Frame {
    wire::frame(Body::RequestResult(RequestResult {
        id,
        ok,
        error: error.into(),
        session: session.into(),
        reason: String::new(),
    }))
}

/// A refusal, with the word clients translate.
fn refused(id: u64, reason: &str, error: impl Into<String>) -> Frame {
    wire::frame(Body::RequestResult(RequestResult {
        id,
        ok: false,
        error: error.into(),
        session: String::new(),
        reason: reason.to_string(),
    }))
}

impl Tabs {
    /// A device's request: the frames to send now (lists, a refusal), the
    /// rest handed to NativeTerm and answered later (`answers`). `open`:
    /// the sessions this device sees; `viewer`: it may only see.
    pub(crate) fn take(
        &mut self,
        dir: &Path,
        conn: u64,
        device: &str,
        request: Request,
        viewer: bool,
        open: &HashSet<PaneId>,
    ) -> Vec<Frame> {
        let id = request.id;
        let what = match request.kind {
            Some(Kind::ListHosts(_)) => return vec![hosts(dir), result(id, true, "", "")],
            Some(Kind::ListResumable(_)) => {
                self.resumable_sent = Some(modified(dir));
                return vec![resumable(dir), result(id, true, "", "")];
            }
            _ if viewer => return vec![refused(id, "view_only", "this device may only see")],
            Some(Kind::OpenTab(o)) if !o.host.is_empty() => What::Open { host: o.host },
            Some(Kind::CloseTab(c)) => {
                // only a session this device sees
                if !c.session.parse::<PaneId>().is_ok_and(|p| open.contains(&p)) {
                    return vec![refused(
                        id,
                        "no_session",
                        format!("no session {}", c.session),
                    )];
                }
                What::Close {
                    session: c.session,
                    detach: c.detach,
                }
            }
            Some(Kind::Resume(r)) if !r.id.is_empty() => What::Resume { tab: r.id },
            _ => {
                return vec![refused(
                    id,
                    "unknown_request",
                    "not a request this desktop knows",
                )]
            }
        };
        let name = format!("{conn}-{id}");
        let handed = Handed {
            conn,
            id,
            device,
            what,
        };
        match hand_over(dir, &name, &handed) {
            Ok(()) => {
                self.waiting.push((name, id, Instant::now()));
                Vec::new()
            }
            Err(e) => vec![refused(
                id,
                "failed",
                format!("not handed to NativeTerm: {e}"),
            )],
        }
    }

    /// NativeTerm's answers come since, and what was not answered in
    /// time; the tabs that can be brought back again, when they changed
    /// and the device follows them.
    pub(crate) fn answers(&mut self, dir: &Path) -> Vec<Frame> {
        let mut frames = Vec::new();
        self.waiting.retain(|(name, id, at)| {
            let path = dir.join(RESULTS).join(format!("{name}.json"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                let _ = std::fs::remove_file(&path);
                let answer: Answer = serde_json::from_str(&text).unwrap_or_default();
                frames.push(wire::frame(Body::RequestResult(RequestResult {
                    id: *id,
                    ok: answer.ok,
                    error: answer.error,
                    session: answer.session,
                    reason: answer.code,
                })));
                return false;
            }
            if at.elapsed() >= ANSWER_WITHIN {
                let _ = std::fs::remove_file(dir.join(REQUESTS).join(format!("{name}.json")));
                frames.push(refused(*id, "not_answered", "NativeTerm did not answer"));
                return false;
            }
            true
        });
        if let Some(sent) = self.resumable_sent {
            let now = modified(dir);
            if now != sent {
                self.resumable_sent = Some(now);
                frames.push(resumable(dir));
            }
        }
        frames
    }
}

fn modified(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join(RESUMABLE))
        .and_then(|m| m.modified())
        .ok()
}

fn hand_over(dir: &Path, name: &str, handed: &Handed) -> std::io::Result<()> {
    let requests = dir.join(REQUESTS);
    std::fs::create_dir_all(&requests)?;
    let temp = requests.join(format!("{name}.json.new"));
    std::fs::write(&temp, serde_json::to_vec(handed)?)?;
    std::fs::rename(temp, requests.join(format!("{name}.json")))
}

fn hosts(dir: &Path) -> Frame {
    let entries: Vec<HostEntry> = std::fs::read_to_string(dir.join(HOSTS))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let hosts = entries
        .into_iter()
        .map(|h| HostInfo {
            alias: h.alias,
            label: h.label,
            folder: h.folder,
            kept: h.kept,
        })
        .collect();
    wire::frame(Body::Hosts(Hosts { hosts }))
}

fn resumable(dir: &Path) -> Frame {
    let entries: Vec<ResumableEntry> = std::fs::read_to_string(dir.join(RESUMABLE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let tabs = entries
        .into_iter()
        .map(|t| ResumableInfo {
            id: t.id,
            title: t.title,
            kind: match t.kind.as_str() {
                "detached" => resumable_info::Kind::Detached,
                "tmux" => resumable_info::Kind::Tmux,
                _ => resumable_info::Kind::Unspecified,
            } as i32,
        })
        .collect();
    wire::frame(Body::Resumable(Resumable { tabs }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_term_remote::proto::{CloseTab, ListHosts, OpenTab};

    fn request(id: u64, kind: Kind) -> Request {
        Request {
            id,
            kind: Some(kind),
        }
    }

    fn results(frames: &[Frame]) -> Vec<(u64, bool, String)> {
        frames
            .iter()
            .filter_map(|f| match &f.body {
                Some(Body::RequestResult(r)) => Some((r.id, r.ok, r.error.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn requests_are_handed_over_and_answered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(HOSTS),
            r#"[{"alias":"web01","label":"Web","folder":"prod","kept":true}]"#,
        )
        .unwrap();
        let open: HashSet<PaneId> = [5].into_iter().collect();
        let mut tabs = Tabs::default();

        let listed = tabs.take(
            dir.path(),
            3,
            "ab",
            request(1, Kind::ListHosts(ListHosts {})),
            false,
            &open,
        );
        match &listed[0].body {
            Some(Body::Hosts(h)) => assert_eq!(
                (h.hosts[0].alias.as_str(), h.hosts[0].kept),
                ("web01", true)
            ),
            other => panic!("{other:?}"),
        }

        let refused = tabs.take(
            dir.path(),
            3,
            "ab",
            request(
                2,
                Kind::OpenTab(OpenTab {
                    host: "web01".into(),
                }),
            ),
            true,
            &open,
        );
        assert_eq!(
            results(&refused),
            vec![(2, false, "this device may only see".to_string())]
        );
        let unseen = CloseTab {
            session: "9".into(),
            detach: false,
        };
        let refused = tabs.take(
            dir.path(),
            3,
            "ab",
            request(3, Kind::CloseTab(unseen)),
            false,
            &open,
        );
        assert!(!results(&refused)[0].1, "a session this device doesn't see");

        let now = tabs.take(
            dir.path(),
            3,
            "ab",
            request(
                4,
                Kind::OpenTab(OpenTab {
                    host: "web01".into(),
                }),
            ),
            false,
            &open,
        );
        assert!(now.is_empty(), "answered later");
        let handed = std::fs::read_to_string(dir.path().join(REQUESTS).join("3-4.json")).unwrap();
        assert_eq!(
            handed,
            r#"{"conn":3,"id":4,"device":"ab","kind":"open","host":"web01"}"#
        );
        assert!(tabs.answers(dir.path()).is_empty(), "not answered yet");

        std::fs::create_dir_all(dir.path().join(RESULTS)).unwrap();
        std::fs::write(
            dir.path().join(RESULTS).join("3-4.json"),
            r#"{"ok":true,"session":"12"}"#,
        )
        .unwrap();
        let answered = tabs.answers(dir.path());
        match &answered[0].body {
            Some(Body::RequestResult(r)) => {
                assert_eq!((r.id, r.ok, r.session.as_str()), (4, true, "12"))
            }
            other => panic!("{other:?}"),
        }
        assert!(!dir.path().join(RESULTS).join("3-4.json").exists(), "taken");

        let resume = native_term_remote::proto::Resume { id: "d:2".into() };
        assert!(tabs
            .take(
                dir.path(),
                3,
                "ab",
                request(5, Kind::Resume(resume)),
                false,
                &open
            )
            .is_empty());
        let handed = std::fs::read_to_string(dir.path().join(REQUESTS).join("3-5.json")).unwrap();
        assert_eq!(
            handed,
            r#"{"conn":3,"id":5,"device":"ab","kind":"resume","tab":"d:2"}"#
        );
    }
}
