//! Logging (ADR 0007): every crate logs through `tracing`; this installs
//! the subscriber. Events go to stderr and to a daily file under
//! `$XDG_STATE_HOME/orbit/logs`, kept a week; `log` messages from GPUI and
//! other dependencies are bridged in, and panics are logged with a
//! backtrace. `ORBIT_LOG` (else `RUST_LOG`) sets the filter, and Settings'
//! Detailed logging raises Orbit's own crates to debug without a restart.
//! Logs never hold vault content: see the ADR.

use std::{
    io::IsTerminal as _,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    EnvFilter, Registry, fmt, layer::SubscriberExt as _, reload, util::SubscriberInitExt as _,
};

/// How many daily log files are kept.
const KEEP_DAYS: usize = 7;

/// Orbit's own crates, by the start of the targets their events carry:
/// `orbit` covers the app and orbit-core, orbit-fs and orbit-index too.
const ORBIT_TARGETS: [&str; 4] = ["orbit", "md_engine", "diagrams", "editor"];

/// The filter: `env` (from `ORBIT_LOG` or `RUST_LOG`) when set, else
/// warnings and errors from everything and info from Orbit's crates, or
/// debug with `detailed`.
pub fn filter_directives(env: Option<&str>, detailed: bool) -> String {
    if let Some(env) = env.map(str::trim).filter(|env| !env.is_empty()) {
        return env.to_string();
    }
    let level = if detailed { "debug" } else { "info" };
    std::iter::once("warn".to_string())
        .chain(
            ORBIT_TARGETS
                .iter()
                .map(|target| format!("{target}={level}")),
        )
        .collect::<Vec<_>>()
        .join(",")
}

/// Changes the filter while Orbit runs, unless the environment set it.
static RELOAD: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

/// Installs logging: stderr (coloured on a terminal), the daily file, the
/// `log` bridge and the panic hook. A log file that can't be opened leaves
/// stderr alone, said once; Orbit starts regardless.
pub fn init(detailed: bool) {
    let set = |var: &str| std::env::var(var).ok().filter(|v| !v.trim().is_empty());
    let mut env = set("ORBIT_LOG").or_else(|| set("RUST_LOG"));
    let mut directives = filter_directives(env.as_deref(), detailed);
    // A filter that doesn't parse is said, and the default used instead.
    let rejected = match EnvFilter::try_new(&directives) {
        Ok(_) => None,
        Err(e) => {
            env = None;
            directives = filter_directives(None, detailed);
            Some(e.to_string())
        }
    };
    let (filter, handle) = reload::Layer::new(EnvFilter::new(&directives));
    let dir = log_dir();
    let file = dir.as_deref().map(open_file);
    // Written as each event happens, not from a queue: when Orbit
    // crashes, the lines saying why are already in the file.
    let (file_layer, file_error) = match file {
        Some(Ok(appender)) => (
            Some(fmt::layer().with_ansi(false).with_writer(appender)),
            None,
        ),
        Some(Err(e)) => (None, Some(e)),
        None => (None, Some("there's no home folder".to_string())),
    };
    let stderr = fmt::layer()
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr);
    // Also bridges `log` (GPUI's and others') into tracing.
    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file_layer)
        .try_init()
        .is_ok();
    if installed && env.is_none() {
        RELOAD.set(handle).ok();
    }
    install_panic_hook();
    let session = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "wayland"
    } else if std::env::var_os("DISPLAY").is_some() {
        "x11"
    } else {
        "unknown"
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        session,
        log_dir = %dir.as_deref().map_or("none".into(), |d| d.display().to_string()),
        filter = %directives,
        "Orbit started"
    );
    if let Some(e) = rejected {
        tracing::warn!(error = %e, "ORBIT_LOG or RUST_LOG isn't a filter Orbit understands; using the default");
    }
    if let Some(e) = file_error {
        tracing::warn!(error = %e, "couldn't open the log file; logging to stderr only");
    }
}

/// Raises Orbit's crates to debug, or back: Settings' Detailed logging.
/// The environment's filter, when set, stays.
pub fn set_detailed(detailed: bool) {
    let Some(handle) = RELOAD.get() else {
        return;
    };
    let filter = EnvFilter::new(filter_directives(None, detailed));
    if handle.reload(filter).is_ok() {
        tracing::info!(detailed, "log level changed");
    }
}

/// Where the log files are.
pub fn log_dir() -> Option<PathBuf> {
    orbit_core::default_state_dir().map(|dir| dir.join("logs"))
}

/// Shows the log folder in the file manager, for attaching a log to a bug
/// report.
pub fn open_folder(cx: &gpui_kit::App) {
    let Some(dir) = log_dir() else {
        return tracing::warn!("there's no log folder: no home folder");
    };
    if let Err(e) = create_private_dir(&dir) {
        return tracing::warn!(error = %e, "couldn't make the log folder");
    }
    cx.open_with_system(&dir);
}

/// Today's log file in `dir`, made private to the user, rotating daily
/// (by UTC dates). Files beyond a week are deleted now: the appender only
/// prunes when a day rolls over while Orbit runs.
fn open_file(dir: &Path) -> Result<RollingFileAppender, String> {
    create_private_dir(dir).map_err(|e| e.to_string())?;
    prune(dir);
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("orbit")
        .filename_suffix("log")
        .max_log_files(KEEP_DAYS)
        .build(dir)
        .map_err(|e| e.to_string())
}

/// Deletes all but the newest week of log files in `dir`: by name, which
/// carries the date.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("orbit.") && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    // Today's is made after: a week is today's and the six before it.
    let keep = KEEP_DAYS.saturating_sub(1);
    let old = logs.len().saturating_sub(keep);
    for log in &logs[..old] {
        std::fs::remove_file(log).ok();
    }
}

/// `dir`, made with its parents; `dir` itself readable by the user only.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Logs a panic, where and with a backtrace, before the usual report on
/// stderr: a crash leaves its cause in the file. Not its message, which
/// can quote note text or a path (a slicing panic does): that's on stderr
/// only (ADR 0007).
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        tracing::error!(
            target: "panic",
            %location,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "Orbit panicked"
        );
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::{KEEP_DAYS, filter_directives, prune};

    #[test]
    fn a_week_of_logs_is_kept_and_other_files_are_left_alone() {
        let dir = std::env::temp_dir().join(format!("orbit-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for day in 1..=10 {
            std::fs::write(dir.join(format!("orbit.2026-10-{day:02}.log")), "").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        prune(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        std::fs::remove_dir_all(&dir).ok();
        let kept: Vec<String> = std::iter::once("notes.txt".to_string())
            .chain((5..=10).map(|day| format!("orbit.2026-10-{day:02}.log")))
            .collect();
        assert_eq!(
            left,
            kept,
            "the newest {} days, with today's still to come",
            KEEP_DAYS - 1
        );
    }

    #[test]
    fn by_default_orbit_says_what_it_does_and_others_only_what_goes_wrong() {
        let filter = filter_directives(None, false);
        assert!(filter.starts_with("warn,"));
        assert!(filter.contains("orbit=info"), "orbit_core and the rest too");
        assert!(filter.contains("editor=info"));
        assert!(!filter.contains("debug"));
    }

    #[test]
    fn detailed_logging_raises_orbit_to_debug_only() {
        let filter = filter_directives(None, true);
        assert!(filter.starts_with("warn,"), "dependencies stay at warn");
        assert!(filter.contains("orbit=debug"));
        assert!(filter.contains("diagrams=debug"));
    }

    #[test]
    fn the_environment_wins_when_set() {
        assert_eq!(filter_directives(Some("trace"), false), "trace");
        assert_eq!(
            filter_directives(Some("orbit_core=trace"), true),
            "orbit_core=trace"
        );
        assert_eq!(
            filter_directives(Some("  "), false),
            filter_directives(None, false),
            "blank is unset"
        );
    }
}
