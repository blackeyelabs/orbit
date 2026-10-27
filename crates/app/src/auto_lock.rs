//! Protected folders lock themselves when you walk away: after a while
//! without input (Settings' `auto_lock_minutes`), when the computer sleeps
//! and when the screen locks. On Linux the last two come over D-Bus, from
//! logind (`PrepareForSleep`, a session's `Lock`) and the desktop's
//! ScreenSaver, freedesktop's or GNOME's (`ActiveChanged`); where they don't
//! come, only idle time locks. Before sleep, Orbit holds sleep back (a
//! logind "delay" inhibitor, taken only while locking on suspend is on)
//! until the folders are locked. Other systems lock on idle time alone.

use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::appearance::AppSettings;
use crate::vaults;

/// How often idle time is checked.
const IDLE_CHECK: Duration = Duration::from_secs(15);

/// When there was last input, and whether folders locked for idling since.
struct Activity {
    last: Instant,
    idle_locked: bool,
}

impl Global for Activity {}

/// A system event folders lock for, if Settings say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Suspend,
    ScreenLock,
}

/// A system event to lock for, with how to say it's done.
struct Event {
    reason: Reason,
    /// Told once the folders are locked: sleep waits for it.
    done: Option<std::sync::mpsc::Sender<()>>,
}

pub fn init(cx: &mut App) {
    cx.set_global(Activity {
        last: Instant::now(),
        idle_locked: false,
    });
    cx.observe_keystrokes(|_, _, cx| active(cx)).detach();
    // Idle time.
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(IDLE_CHECK).await;
            cx.update(|cx| {
                let Some(minutes) = AppSettings::get(cx).auto_lock_minutes else {
                    return;
                };
                let activity = cx.global_mut::<Activity>();
                let idle = activity.last.elapsed();
                if !activity.idle_locked && idle >= Duration::from_secs(u64::from(minutes) * 60) {
                    activity.idle_locked = true;
                    vaults::lock_everywhere(cx);
                }
            });
        }
    })
    .detach();
    // Sleep and screen lock.
    let (events, received) = async_channel::unbounded();
    #[cfg(target_os = "linux")]
    {
        let lock_on_suspend = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            AppSettings::get(cx).lock_on_suspend,
        ));
        let flag = lock_on_suspend.clone();
        cx.observe_global::<AppSettings>(move |cx| {
            flag.store(
                AppSettings::get(cx).lock_on_suspend,
                std::sync::atomic::Ordering::Relaxed,
            );
        })
        .detach();
        system::listen(events, lock_on_suspend);
    }
    #[cfg(not(target_os = "linux"))]
    drop(events);
    cx.spawn(async move |cx| {
        while let Ok(Event { reason, done }) = received.recv().await {
            cx.update(|cx| {
                let settings = AppSettings::get(cx);
                let wanted = match reason {
                    Reason::Suspend => settings.lock_on_suspend,
                    Reason::ScreenLock => settings.lock_on_screen_lock,
                };
                if wanted {
                    vaults::lock_everywhere(cx);
                }
            });
            if let Some(done) = done {
                done.send(()).ok();
            }
        }
    })
    .detach();
}

/// Input came: idle time starts again.
fn active(cx: &mut App) {
    let activity = cx.global_mut::<Activity>();
    activity.last = Instant::now();
    activity.idle_locked = false;
}

/// Watches a window's pointer for input: moves, clicks and scrolls anywhere
/// in it, even those an element inside keeps to itself. A window's root
/// holds one.
pub fn watch_pointer() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            window.on_mouse_event(|_: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    active(cx);
                }
            });
            window.on_mouse_event(|_: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    active(cx);
                }
            });
            window.on_mouse_event(|_: &ScrollWheelEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    active(cx);
                }
            });
        },
    )
    .absolute()
}

#[cfg(target_os = "linux")]
mod system {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use zbus::blocking::{Connection, Proxy};

    use super::{Event, Reason};

    /// How long sleep waits for the folders to lock, at most.
    const SLEEP_WAIT: Duration = Duration::from_secs(3);

    /// The screensavers that say when the screen locks: the freedesktop one
    /// (KDE, Xfce, …) and GNOME's.
    const SCREENSAVERS: [(&str, &str); 2] = [
        (
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
        ),
        ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver"),
    ];

    /// Listens on threads of its own; any bus or service missing just means
    /// no locking on that event. Sleep is held back only while
    /// `lock_on_suspend` is set.
    pub(super) fn listen(events: async_channel::Sender<Event>, lock_on_suspend: Arc<AtomicBool>) {
        let spawn = |name: &str, run: Box<dyn FnOnce() + Send>| {
            thread::Builder::new().name(name.into()).spawn(run).ok();
        };
        let sleep_events = events.clone();
        spawn(
            "orbit-sleep",
            Box::new(move || {
                sleep(sleep_events, lock_on_suspend).ok();
            }),
        );
        let session_events = events.clone();
        spawn(
            "orbit-session-lock",
            Box::new(move || {
                session_lock(session_events).ok();
            }),
        );
        for (name, path) in SCREENSAVERS {
            let events = events.clone();
            spawn(
                "orbit-screensaver",
                Box::new(move || {
                    screensaver(events, name, path).ok();
                }),
            );
        }
    }

    /// logind's manager.
    fn manager(bus: &Connection) -> zbus::Result<Proxy<'static>> {
        Proxy::new(
            bus,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
    }

    /// logind: `PrepareForSleep(true)` before suspend, held back by a delay
    /// inhibitor until the folders lock.
    fn sleep(
        events: async_channel::Sender<Event>,
        lock_on_suspend: Arc<AtomicBool>,
    ) -> zbus::Result<()> {
        let bus = Connection::system()?;
        let manager = manager(&bus)?;
        let inhibit = || -> Option<zbus::zvariant::OwnedFd> {
            if !lock_on_suspend.load(Ordering::Relaxed) {
                return None;
            }
            manager
                .call(
                    "Inhibit",
                    &("sleep", "Orbit", "Locking protected folders", "delay"),
                )
                .ok()
        };
        let mut inhibitor = inhibit();
        for signal in manager.receive_signal("PrepareForSleep")? {
            let Ok(going) = signal.body().deserialize::<bool>() else {
                continue;
            };
            if going {
                let (done, locked) = mpsc::channel();
                let event = Event {
                    reason: Reason::Suspend,
                    done: Some(done),
                };
                if events.send_blocking(event).is_err() {
                    break;
                }
                locked.recv_timeout(SLEEP_WAIT).ok();
                // Sleep may go ahead.
                inhibitor = None;
            } else if inhibitor.is_none() {
                inhibitor = inhibit();
            }
        }
        drop(inhibitor);
        Ok(())
    }

    /// logind: Orbit's own session's `Lock`.
    fn session_lock(events: async_channel::Sender<Event>) -> zbus::Result<()> {
        let bus = Connection::system()?;
        let session: zbus::zvariant::OwnedObjectPath =
            manager(&bus)?.call("GetSessionByPID", &(std::process::id()))?;
        let session = Proxy::new(
            &bus,
            "org.freedesktop.login1",
            session,
            "org.freedesktop.login1.Session",
        )?;
        for _ in session.receive_signal("Lock")? {
            let event = Event {
                reason: Reason::ScreenLock,
                done: None,
            };
            if events.send_blocking(event).is_err() {
                break;
            }
        }
        Ok(())
    }

    /// A desktop screensaver: `ActiveChanged(true)` when it locks.
    fn screensaver(
        events: async_channel::Sender<Event>,
        name: &'static str,
        path: &'static str,
    ) -> zbus::Result<()> {
        let bus = Connection::session()?;
        let screensaver = Proxy::new(&bus, name, path, name)?;
        for signal in screensaver.receive_signal("ActiveChanged")? {
            if signal.body().deserialize::<bool>() != Ok(true) {
                continue;
            }
            let event = Event {
                reason: Reason::ScreenLock,
                done: None,
            };
            if events.send_blocking(event).is_err() {
                break;
            }
        }
        Ok(())
    }
}
