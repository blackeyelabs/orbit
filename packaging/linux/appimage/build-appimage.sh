#!/usr/bin/env bash
# Assembles the AppDir and builds the Orbit AppImage.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

VERSION="${1:-0.1.0}"
BINARY_INPUT="${2:-${REPO_ROOT}/target/release/orbit}"
OUTPUT_INPUT="${3:-${REPO_ROOT}/target/appimage}"

BINARY_PATH="$(readlink -f "${BINARY_INPUT}" || realpath "${BINARY_INPUT}")"
mkdir -p "${OUTPUT_INPUT}"
OUTPUT_DIR="$(cd "${OUTPUT_INPUT}" && pwd)"

if [[ ! -f "${BINARY_PATH}" ]]; then
    echo "Error: Release binary not found at: ${BINARY_PATH}" >&2
    exit 1
fi

APP_NAME="Orbit"
ARCH="x86_64"
OUTPUT_FILE="${OUTPUT_DIR}/${APP_NAME}-${VERSION}-${ARCH}.AppImage"

echo "Assembling AppDir for ${APP_NAME} ${VERSION}..."

BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "${BUILD_DIR}"' EXIT

APPDIR="${BUILD_DIR}/AppDir"
mkdir -p "${APPDIR}/usr/bin"
mkdir -p "${APPDIR}/usr/share/applications"
mkdir -p "${APPDIR}/usr/share/metainfo"

# 1. Install binary
install -m755 "${BINARY_PATH}" "${APPDIR}/usr/bin/orbit"

# 2. Install AppRun
install -m755 "${SCRIPT_DIR}/AppRun" "${APPDIR}/AppRun"

# 3. Install desktop launcher
install -m644 "${REPO_ROOT}/packaging/linux/io.github.blackeyelabs.orbit.desktop" \
    "${APPDIR}/io.github.blackeyelabs.orbit.desktop"
install -m644 "${REPO_ROOT}/packaging/linux/io.github.blackeyelabs.orbit.desktop" \
    "${APPDIR}/usr/share/applications/io.github.blackeyelabs.orbit.desktop"

# 4. Install AppStream metadata
install -m644 "${REPO_ROOT}/packaging/linux/io.github.blackeyelabs.orbit.metainfo.xml" \
    "${APPDIR}/usr/share/metainfo/io.github.blackeyelabs.orbit.metainfo.xml"

# 5. Install icons
for size in 16 24 32 48 64 128 256 512; do
    icon_src="${REPO_ROOT}/packaging/linux/icons/hicolor/${size}x${size}/apps/orbit.png"
    if [[ -f "${icon_src}" ]]; then
        mkdir -p "${APPDIR}/usr/share/icons/hicolor/${size}x${size}/apps"
        install -m644 "${icon_src}" "${APPDIR}/usr/share/icons/hicolor/${size}x${size}/apps/orbit.png"
    fi
done
scalable_icon="${REPO_ROOT}/packaging/linux/icons/hicolor/scalable/apps/orbit.png"
if [[ -f "${scalable_icon}" ]]; then
    mkdir -p "${APPDIR}/usr/share/icons/hicolor/scalable/apps"
    install -m644 "${scalable_icon}" "${APPDIR}/usr/share/icons/hicolor/scalable/apps/orbit.png"
fi

# Root level icon for AppImage
cp "${REPO_ROOT}/packaging/linux/icons/hicolor/512x512/apps/orbit.png" "${APPDIR}/orbit.png"
ln -sf orbit.png "${APPDIR}/.DirIcon"

# 6. Check for appimagetool or download if in CI / requested
if command -v appimagetool >/dev/null 2>&1; then
    APPIMAGETOOL="appimagetool"
elif [[ -f "${BUILD_DIR}/appimagetool" ]]; then
    APPIMAGETOOL="${BUILD_DIR}/appimagetool"
else
    echo "Notice: 'appimagetool' not found in PATH."
    echo "AppDir prepared successfully at: ${APPDIR}"
    # If ARCH is set and network is available, we can download appimagetool
    if command -v curl >/dev/null 2>&1; then
        echo "Downloading standalone appimagetool..."
        curl -fsSL -o "${BUILD_DIR}/appimagetool" \
            "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage" \
            2>/dev/null || true
        if [[ -f "${BUILD_DIR}/appimagetool" ]]; then
            chmod +x "${BUILD_DIR}/appimagetool"
            APPIMAGETOOL="${BUILD_DIR}/appimagetool"
        fi
    fi
fi

if [[ -n "${APPIMAGETOOL:-}" && -x "${APPIMAGETOOL:-}" ]]; then
    echo "Generating ${OUTPUT_FILE}..."
    ARCH="${ARCH}" "${APPIMAGETOOL}" --appimage-extract-and-run "${APPDIR}" "${OUTPUT_FILE}"
    echo "Successfully generated: ${OUTPUT_FILE}"
else
    echo "AppDir layout created. To build AppImage: appimagetool ${APPDIR} ${OUTPUT_FILE}"
fi
