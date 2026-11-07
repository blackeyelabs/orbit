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
    /// Where the note was scrolled.
    pub scroll: Scroll,
}

/// Where a note was scrolled, in the view that showed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scroll {
    /// Source mode: pixels from the top.
    Source(f32),
    /// Live Preview, whose blocks out of view aren't measured: the block
    /// starting at source offset `top` was at the top of the view,
    /// scrolled `within` pixels into it.
    LivePreview { top: usize, within: f32 },
}

impl Default for Scroll {
    fn default() -> Self {
        Scroll::Source(0.)
    }
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
    /// Source mode's scroll, in pixels.
    #[serde(default)]
    scroll: f32,
    /// Live Preview's, when it showed the note: a session from before it
    /// has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    live: Option<LiveFile>,
}

/// Live Preview's scroll as stored: see [`Scroll::LivePreview`].
#[derive(Serialize, Deserialize)]
struct LiveFile {
    top: usize,
    within: f32,
}

/// A stored number of pixels: never negative, and 0 for nonsense.
fn pixels(value: f32) -> f32 {
    if value.is_finite() { value.max(0.) } else { 0. }
}

impl SessionFile {
    /// The session to store, keeping only the tabs whose notes `keep` accepts.
    pub(crate) fn of(session: &Session, keep: impl Fn(&VaultPath) -> bool) -> Self {
        let tabs = session.tabs.iter().map(|tab| {
            keep(&tab.path).then(|| {
                // Only the view that showed the note has a scroll to keep.
                let (scroll, live) = match tab.scroll {
                    Scroll::Source(scroll) => (scroll, None),
                    Scroll::LivePreview { top, within } => (0., Some(LiveFile { top, within })),
                };
                TabFile {
                    path: tab.path.as_str().to_string(),
                    scroll,
                    live,
                }
            })
        });
        let (tabs, active) = kept(tabs, session.active);
        SessionFile { tabs, active }
    }

    /// The stored session, keeping only the tabs whose notes `keep` accepts.
    pub(crate) fn session(&self, keep: impl Fn(&VaultPath) -> bool) -> Session {
        let tabs = self.tabs.iter().map(|tab| {
            let path = VaultPath::new(&tab.path).ok().filter(|p| keep(p))?;
            let scroll = match &tab.live {
                Some(live) => Scroll::LivePreview {
                    top: live.top,
                    within: pixels(live.within),
                },
                None => Scroll::Source(pixels(tab.scroll)),
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
