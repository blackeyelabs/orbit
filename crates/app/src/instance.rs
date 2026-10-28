//! One Orbit at a time: a second `orbit` hands what it was asked to open
//! to the one already running, over a socket in the runtime folder, so a
//! vault open there is focused rather than refused.

use std::{
    io::{Read as _, Write as _},
    os::unix::ffi::OsStrExt as _,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
};

use gpui_kit::*;

use crate::vaults;

/// `$XDG_RUNTIME_DIR/orbit.sock`; without a runtime folder there's no
/// handing off, and each Orbit runs on its own.
fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("orbit.sock"))
}

/// Asks a running Orbit to open the vault at `path` (or show its Welcome
/// window); true if one took the request.
pub fn hand_off(path: Option<&Path>) -> bool {
    let Some(socket) = socket_path() else {
        return false;
    };
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    // The real path, since the running Orbit has another working folder.
    let path = path.map(|p| {
        p.canonicalize()
            .or_else(|_| std::path::absolute(p))
            .unwrap_or_else(|_| p.to_path_buf())
    });
    let bytes = path.as_ref().map_or(&[][..], |p| p.as_os_str().as_bytes());
    stream.write_all(bytes).is_ok()
}

/// Takes requests from later `orbit`s for as long as this one runs.
pub fn listen(cx: &mut App) {
    let Some(socket) = socket_path() else { return };
    // Nothing answered `hand_off`, so a socket left there is stale.
    std::fs::remove_file(&socket).ok();
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(e) => {
            return tracing::warn!(
                socket = %socket.display(),
                error = %e,
                "couldn't listen for other Orbit instances"
            );
        }
    };
    let (tx, rx) = async_channel::unbounded::<Option<PathBuf>>();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut bytes = Vec::new();
            if stream.read_to_end(&mut bytes).is_ok() {
                let path =
                    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)));
                if tx.send_blocking(path).is_err() {
                    break;
                }
            }
        }
    });
    cx.spawn(async move |cx| {
        while let Ok(path) = rx.recv().await {
            cx.update(|cx| vaults::open_or_welcome(path, cx));
        }
    })
    .detach();
}
