//! Session state: what a vault had open when its window closed, kept in
//! `.vault/session.json` so the next open picks up there.

use serde::{Deserialize, Serialize};

use crate::VaultPath;

/// The open tabs, in order, and which one is active.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionTab {
    pub path: VaultPath,
    /// How far the note's source was scrolled, in pixels from the top.
    pub scroll: f32,
}

/// `.vault/session.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SessionFile {
    tabs: Vec<TabFile>,
    active: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct TabFile {
    path: String,
    #[serde(default)]
    scroll: f32,
}

impl SessionFile {
    /// The session to store, keeping only the tabs whose notes `keep` accepts.
    pub(crate) fn of(session: &Session, keep: impl Fn(&VaultPath) -> bool) -> Self {
        let tabs = session.tabs.iter().map(|tab| {
            keep(&tab.path).then(|| TabFile {
                path: tab.path.as_str().to_string(),
                scroll: tab.scroll,
            })
        });
        let (tabs, active) = kept(tabs, session.active);
        SessionFile { tabs, active }
    }

    /// The stored session, keeping only the tabs whose notes `keep` accepts.
    pub(crate) fn session(&self, keep: impl Fn(&VaultPath) -> bool) -> Session {
        let tabs = self.tabs.iter().map(|tab| {
            let path = VaultPath::new(&tab.path).ok().filter(|p| keep(p))?;
            let scroll = if tab.scroll.is_finite() {
                tab.scroll.max(0.0)
            } else {
                0.0
            };
            Some(SessionTab { path, scroll })
        });
        let (tabs, active) = kept(tabs, self.active);
        Session { tabs, active }
    }
}

/// The tabs that are `Some`. The active tab stays active; if it's dropped,
/// the tab that took its place is, or else the last one.
fn kept<T>(
    tabs: impl Iterator<Item = Option<T>>,
    active: Option<usize>,
) -> (Vec<T>, Option<usize>) {
    let mut kept = Vec::new();
    let mut active_kept = None;
    for (ix, tab) in tabs.enumerate() {
        if Some(ix) == active {
            active_kept = Some(kept.len());
        }
        kept.extend(tab);
    }
    let active = active_kept
        .filter(|_| !kept.is_empty())
        .map(|a| a.min(kept.len() - 1));
    (kept, active)
}
