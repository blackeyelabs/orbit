#!/usr/bin/env bash
# Local installer helper included in standalone release tarballs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ID="io.github.blackeyelabs.orbit"

PREFIX="${HOME}/.local"
if [[ "${1:-}" == "--system" ]] || [[ $EUID -eq 0 && "${1:-}" != "--user" ]]; then
    PREFIX="/usr/local"
fi

echo "Installing Orbit to ${PREFIX}..."

mkdir -p "${PREFIX}/bin"
mkdir -p "${PREFIX}/share/applications"
mkdir -p "${PREFIX}/share/metainfo"
mkdir -p "${PREFIX}/share/icons"

# Install binary
install -m755 "${SCRIPT_DIR}/orbit" "${PREFIX}/bin/orbit"

# Install desktop entry and metainfo
install -m644 "${SCRIPT_DIR}/io.github.blackeyelabs.orbit.desktop" \
    "${PREFIX}/share/applications/${APP_ID}.desktop"
install -m644 "${SCRIPT_DIR}/io.github.blackeyelabs.orbit.metainfo.xml" \
    "${PREFIX}/share/metainfo/${APP_ID}.metainfo.xml"

# Install icons
if [[ -d "${SCRIPT_DIR}/icons" ]]; then
    cp -r "${SCRIPT_DIR}/icons"/* "${PREFIX}/share/icons/"
fi

# Update desktop cache if tool exists
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${PREFIX}/share/applications" 2>/dev/null || true
fi

echo "Orbit installed successfully to ${PREFIX}!"
echo "Run 'orbit' or launch from your application menu."
