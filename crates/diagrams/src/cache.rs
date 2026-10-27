//! Diagrams drawn in the background, kept by source and theme, so a
//! preview never waits for one and redrawing the same one is free. (They
//! stay SVG: the display rasterizes them at its own scale.)

use std::{
    collections::{HashMap, VecDeque},
    panic,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use crate::{Diagram, DiagramError, DiagramRenderer, DiagramTheme};

/// Drawings kept at most; the oldest go first.
const KEPT: usize = 200;
/// Drawings waiting at most: typing in a diagram asks for a new one at
/// every keystroke, and only the latest few matter.
const WAITING: usize = 4;

type Key = (String, DiagramTheme);

/// A drawing, or why there's none.
pub type Drawn = Arc<Result<Diagram, DiagramError>>;

#[derive(Default)]
struct State {
    /// Each drawing asked for: `None` while it's waiting or being drawn.
    drawn: HashMap<Key, Option<Drawn>>,
    /// Drawn ones, oldest first, for letting go of the oldest.
    order: VecDeque<Key>,
    /// Waiting to be drawn, newest last.
    waiting: VecDeque<Key>,
    /// The cache is gone: the drawing thread ends.
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when something's waiting or the cache closes.
    wake: Condvar,
}

pub struct DiagramCache {
    shared: Arc<Shared>,
}

impl DiagramCache {
    /// A cache drawing with `renderer` on a thread of its own, which calls
    /// `on_ready` (on that thread) after each drawing.
    pub fn new(
        renderer: Arc<dyn DiagramRenderer>,
        on_ready: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared: Arc<Shared> = Arc::default();
        let worker = shared.clone();
        thread::Builder::new()
            .name("diagrams".into())
            .spawn(move || {
                while let Some(key) = next_waiting(&worker) {
                    // A renderer that panics fails this drawing, not the
                    // thread: the others still get drawn.
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        renderer.render(&key.0, key.1)
                    }))
                    .unwrap_or_else(|_| Err(DiagramError::Failed("the renderer crashed".into())));
                    let mut state = lock(&worker.state);
                    state.drawn.insert(key.clone(), Some(Arc::new(result)));
                    state.order.push_back(key);
                    while state.order.len() > KEPT {
                        let old = state.order.pop_front().expect("over the limit");
                        state.drawn.remove(&old);
                    }
                    drop(state);
                    on_ready();
                }
            })
            .expect("a thread for drawing diagrams starts");
        DiagramCache { shared }
    }

    /// The drawing of `source` in `theme`, if it's ready; otherwise it's
    /// drawn in the background, and the cache says when.
    pub fn get(&self, source: &str, theme: DiagramTheme) -> Option<Drawn> {
        let key = (source.to_string(), theme);
        let mut state = lock(&self.shared.state);
        if let Some(drawn) = state.drawn.get(&key) {
            return drawn.clone();
        }
        state.drawn.insert(key.clone(), None);
        state.waiting.push_back(key);
        while state.waiting.len() > WAITING {
            // Asked for again, it waits again.
            let stale = state.waiting.pop_front().expect("over the limit");
            state.drawn.remove(&stale);
        }
        self.shared.wake.notify_one();
        None
    }
}

impl Drop for DiagramCache {
    fn drop(&mut self) {
        lock(&self.shared.state).closed = true;
        self.shared.wake.notify_all();
    }
}

/// The newest drawing waiting, once there is one; `None` once the cache is
/// gone.
fn next_waiting(shared: &Shared) -> Option<Key> {
    let state = lock(&shared.state);
    let mut state = shared
        .wake
        .wait_while(state, |s| s.waiting.is_empty() && !s.closed)
        .unwrap_or_else(|e| e.into_inner());
    if state.closed {
        return None;
    }
    state.waiting.pop_back()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
