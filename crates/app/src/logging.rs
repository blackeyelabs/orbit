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

/// Orbit's own crates, by the target their events carry.
const ORBIT_TARGETS: [&str; 7] = [
    "orbit",
    "orbit_core",
    "orbit_fs",
    "orbit_index",
    "md_engine",
    "diagrams",
    "editor",
];

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
    let env = std::env::var("ORBIT_LOG")
        .ok()
        .or_else(|| std::env::var("RUST_LOG").ok())
        .filter(|env| !env.trim().is_empty());
    let directives = filter_directives(env.as_deref(), detailed);
    let filter = EnvFilter::try_new(&directives)
        .unwrap_or_else(|_| EnvFilter::new(filter_directives(None, detailed)));
    let (filter, handle) = reload::Layer::new(filter);
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
        log_dir = ?dir,
        filter = %directives,
        "Orbit started"
    );
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

/// Today's log file in `dir`, made private to the user, rotating daily.
fn open_file(dir: &Path) -> Result<RollingFileAppender, String> {
    create_private_dir(dir).map_err(|e| e.to_string())?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("orbit")
        .filename_suffix("log")
        .max_log_files(KEEP_DAYS)
        .build(dir)
        .map_err(|e| e.to_string())
}

/// `dir`, and its parents, readable by the user only.
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
/// stderr: a crash leaves its cause in the file.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        tracing::error!(
            target: "panic",
            %location,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "{message}"
        );
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::filter_directives;

    #[test]
    fn by_default_orbit_says_what_it_does_and_others_only_what_goes_wrong() {
        let filter = filter_directives(None, false);
        assert!(filter.starts_with("warn,"));
        assert!(filter.contains("orbit=info"));
        assert!(filter.contains("orbit_core=info"));
        assert!(!filter.contains("debug"));
    }

    #[test]
    fn detailed_logging_raises_orbit_to_debug_only() {
        let filter = filter_directives(None, true);
        assert!(filter.starts_with("warn,"), "dependencies stay at warn");
        assert!(filter.contains("orbit=debug"));
        assert!(filter.contains("orbit_index=debug"));
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
