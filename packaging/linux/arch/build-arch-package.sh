#!/usr/bin/env bash
# Builds a standalone Arch Linux package (.pkg.tar.zst) from local build artifacts.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

VERSION="${1:-0.1.0}"
BINARY_INPUT="${2:-${REPO_ROOT}/target/release/orbit}"
OUTPUT_INPUT="${3:-${REPO_ROOT}/target/arch}"

# Resolve absolute paths
BINARY_PATH="$(readlink -f "${BINARY_INPUT}" || realpath "${BINARY_INPUT}")"
mkdir -p "${OUTPUT_INPUT}"
OUTPUT_DIR="$(cd "${OUTPUT_INPUT}" && pwd)"

if [[ ! -f "${BINARY_PATH}" ]]; then
    echo "Error: Release binary not found at: ${BINARY_PATH}" >&2
    exit 1
fi

PKGNAME="orbit"
PKGREL="1"
FULL_PKGVER="${VERSION}-${PKGREL}"
ARCH="x86_64"
PKG_FILE="${OUTPUT_DIR}/${PKGNAME}-${FULL_PKGVER}-${ARCH}.pkg.tar.zst"

echo "Building Arch package: ${PKG_FILE}..."

BUILD_DIR="$(mktemp -d)"
trap 'rm -rf "${BUILD_DIR}"' EXIT

PKGDIR="${BUILD_DIR}/pkg"
mkdir -p "${PKGDIR}/usr/bin"
mkdir -p "${PKGDIR}/usr/share/applications"
mkdir -p "${PKGDIR}/usr/share/metainfo"
mkdir -p "${OUTPUT_DIR}"

# Install binary
install -m755 "${BINARY_PATH}" "${PKGDIR}/usr/bin/orbit"

# Install desktop entry and AppStream metadata
install -m644 "${REPO_ROOT}/packaging/linux/io.github.blackeyelabs.orbit.desktop" \
    "${PKGDIR}/usr/share/applications/io.github.blackeyelabs.orbit.desktop"
install -m644 "${REPO_ROOT}/packaging/linux/io.github.blackeyelabs.orbit.metainfo.xml" \
    "${PKGDIR}/usr/share/metainfo/io.github.blackeyelabs.orbit.metainfo.xml"

# Install icons
for size in 16 24 32 48 64 128 256 512; do
    icon_src="${REPO_ROOT}/packaging/linux/icons/hicolor/${size}x${size}/apps/orbit.png"
    if [[ -f "${icon_src}" ]]; then
        mkdir -p "${PKGDIR}/usr/share/icons/hicolor/${size}x${size}/apps"
        install -m644 "${icon_src}" "${PKGDIR}/usr/share/icons/hicolor/${size}x${size}/apps/orbit.png"
    fi
done
scalable_icon="${REPO_ROOT}/packaging/linux/icons/hicolor/scalable/apps/orbit.png"
if [[ -f "${scalable_icon}" ]]; then
    mkdir -p "${PKGDIR}/usr/share/icons/hicolor/scalable/apps"
    install -m644 "${scalable_icon}" "${PKGDIR}/usr/share/icons/hicolor/scalable/apps/orbit.png"
fi

# Calculate installed size in bytes
INSTALLED_SIZE="$(du -sb "${PKGDIR}" | awk '{print $1}')"
BUILD_DATE="$(date -u +%s)"

# Generate .PKGINFO
cat > "${PKGDIR}/.PKGINFO" <<EOF
pkgname = ${PKGNAME}
pkgbase = ${PKGNAME}
pkgver = ${FULL_PKGVER}
pkgdesc = Local-first, encrypted workspace for notes and diagrams
url = https://github.com/blackeyelabs/orbit
builddate = ${BUILD_DATE}
packager = Blackeye Labs <dev@blackeyelabs.com>
size = ${INSTALLED_SIZE}
arch = ${ARCH}
license = MIT
depend = fontconfig
depend = freetype2
depend = libxkbcommon
depend = libxkbcommon-x11
depend = wayland
depend = vulkan-icd-loader
optdepend = vulkan-driver: Hardware accelerated GPU rendering
provides = orbit
conflict = orbit-bin
EOF

# Generate .MTREE if bsdtar is present
if command -v bsdtar >/dev/null 2>&1; then
    (
        cd "${PKGDIR}"
        bsdtar -czf .MTREE --format=mtree \
            --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' \
            .PKGINFO usr
    )
fi

# Create final compressed package
(
    cd "${PKGDIR}"
    # Use bsdtar or tar with zstd
    tar --zstd -cf "${PKG_FILE}" .PKGINFO $([ -f .MTREE ] && echo .MTREE) usr
)

echo "Successfully built Arch package: ${PKG_FILE}"
ls -lh "${PKG_FILE}"
