#!/usr/bin/env bash
# Universal, distro-detecting installer script for Orbit.
# Usage: curl -fsSL https://raw.githubusercontent.com/blackeyelabs/orbit/main/install.sh | bash
set -euo pipefail

REPO="blackeyelabs/orbit"
APP_NAME="orbit"
APP_ID="io.github.blackeyelabs.orbit"

# Colors for terminal output
BOLD="\033[1m"
GREEN="\033[32m"
YELLOW="\033[33m"
RED="\033[31m"
BLUE="\033[34m"
RESET="\033[0m"

log_info() {
    echo -e "${BLUE}${BOLD}[INFO]${RESET} $*"
}

log_success() {
    echo -e "${GREEN}${BOLD}[SUCCESS]${RESET} $*"
}

log_warn() {
    echo -e "${YELLOW}${BOLD}[WARN]${RESET} $*"
}

log_error() {
    echo -e "${RED}${BOLD}[ERROR]${RESET} $*" >&2
}

# CLI Flags
FORCE_APPIMAGE=false
TARGET_VERSION=""
DRY_RUN=false
NON_INTERACTIVE=false
UNINSTALL=false

show_help() {
    cat <<EOF
Orbit Universal Linux Installer

Usage:
  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | bash
  # or
  ./install.sh [OPTIONS]

Options:
  --version <v>    Install a specific release version (e.g. v0.1.0 or 0.1.0)
  --appimage       Force standalone AppImage installation regardless of distro
  --uninstall      Uninstall Orbit and remove desktop entries
  --dry-run        Print detected distro and planned actions without executing
  -y, --yes        Non-interactive mode, accept all prompts
  -h, --help       Show this help message
EOF
    exit 0
}

# Parse CLI arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            TARGET_VERSION="${2:-}"
            shift 2
            ;;
        --appimage)
            FORCE_APPIMAGE=true
            shift
            ;;
        --uninstall)
            UNINSTALL=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -y|--yes)
            NON_INTERACTIVE=true
            shift
            ;;
        -h|--help)
            show_help
            ;;
        *)
            log_error "Unknown option: $1"
            show_help
            ;;
    esac
done

# 1. OS & Architecture checks
OS="$(uname -s)"
ARCH="$(uname -m)"

if [[ "${OS}" != "Linux" ]]; then
    log_error "This installer only supports Linux. Detected: ${OS}"
    exit 1
fi

if [[ "${ARCH}" != "x86_64" ]]; then
    log_error "Currently, pre-compiled Orbit packages only support x86_64. Detected: ${ARCH}"
    exit 1
fi

# 2. Check root / sudo capability
HAS_SUDO=false
if [[ $EUID -eq 0 ]]; then
    HAS_SUDO=true
    SUDO_CMD=""
elif command -v sudo >/dev/null 2>&1; then
    HAS_SUDO=true
    SUDO_CMD="sudo"
else
    SUDO_CMD=""
fi

# Helper function to run commands (respecting dry-run and sudo)
run_cmd() {
    if [[ "${DRY_RUN}" == "true" ]]; then
        echo -e "  ${YELLOW}(dry-run)${RESET} $*"
    else
        "$@"
    fi
}

run_sudo_cmd() {
    if [[ "${DRY_RUN}" == "true" ]]; then
        echo -e "  ${YELLOW}(dry-run)${RESET} ${SUDO_CMD} $*"
    else
        if [[ -n "${SUDO_CMD}" ]]; then
            ${SUDO_CMD} "$@"
        else
            "$@"
        fi
    fi
}

# 3. Detect Linux distribution
DISTRO_ID="unknown"
DISTRO_LIKE=""

if [[ -f /etc/os-release ]]; then
    # Source /etc/os-release in a subshell to avoid polluting environment
    DISTRO_ID="$(bash -c '. /etc/os-release && echo "${ID:-unknown}"' | tr '[:upper:]' '[:lower:]')"
    DISTRO_LIKE="$(bash -c '. /etc/os-release && echo "${ID_LIKE:-}"' | tr '[:upper:]' '[:lower:]')"
fi

# Determine appropriate package format
PACKAGE_FORMAT="appimage"

if [[ "${FORCE_APPIMAGE}" == "true" ]]; then
    PACKAGE_FORMAT="appimage"
elif [[ "${DISTRO_ID}" =~ debian|ubuntu|linuxmint|pop|elementary|zorin|kali ]] || [[ "${DISTRO_LIKE}" =~ debian|ubuntu ]]; then
    PACKAGE_FORMAT="deb"
elif [[ "${DISTRO_ID}" =~ fedora|rhel|centos|rocky|almalinux ]] || [[ "${DISTRO_LIKE}" =~ fedora|rhel ]]; then
    PACKAGE_FORMAT="rpm"
elif [[ "${DISTRO_ID}" =~ opensuse|sles|suse ]] || [[ "${DISTRO_LIKE}" =~ suse ]]; then
    PACKAGE_FORMAT="zypper-rpm"
elif [[ "${DISTRO_ID}" =~ arch|cachyos|manjaro|endeavouros|garuda|steamos ]] || [[ "${DISTRO_LIKE}" =~ arch ]]; then
    PACKAGE_FORMAT="arch"
elif [[ "${DISTRO_ID}" == "nixos" ]]; then
    PACKAGE_FORMAT="nix"
fi

# If user cannot run sudo and it's not nix, fall back to user-space AppImage
if [[ "${HAS_SUDO}" == "false" && "${PACKAGE_FORMAT}" != "nix" ]]; then
    log_warn "No root or sudo permissions available. Falling back to local user AppImage."
    PACKAGE_FORMAT="appimage"
fi

# Handle Uninstallation
if [[ "${UNINSTALL}" == "true" ]]; then
    log_info "Initiating uninstallation for Orbit..."
    case "${PACKAGE_FORMAT}" in
        deb)
            log_info "Removing Debian package 'orbit'..."
            run_sudo_cmd apt-get remove -y orbit || run_sudo_cmd dpkg -r orbit
            ;;
        rpm)
            log_info "Removing RPM package 'orbit'..."
            run_sudo_cmd dnf remove -y orbit
            ;;
        zypper-rpm)
            log_info "Removing RPM package 'orbit'..."
            run_sudo_cmd zypper remove -y orbit
            ;;
        arch)
            log_info "Removing Pacman package 'orbit'..."
            run_sudo_cmd pacman -R --noconfirm orbit || true
            ;;
        nix)
            log_info "Removing Nix package 'orbit'..."
            run_cmd nix profile remove github:${REPO} || true
            ;;
        appimage)
            log_info "Removing user-space AppImage and desktop integrations..."
            run_cmd rm -f "${HOME}/.local/bin/orbit"
            run_cmd rm -f "${HOME}/.local/share/applications/${APP_ID}.desktop"
            run_cmd rm -f "${HOME}/.local/share/icons/hicolor/*/apps/orbit.png"
            if command -v update-desktop-database >/dev/null 2>&1; then
                run_cmd update-desktop-database "${HOME}/.local/share/applications/" || true
            fi
            ;;
    esac
    log_success "Orbit has been successfully uninstalled."
    exit 0
fi

# 4. Resolve Version
CLEAN_VERSION=""
if [[ -n "${TARGET_VERSION}" ]]; then
    CLEAN_VERSION="${TARGET_VERSION#v}"
    RELEASE_TAG="v${CLEAN_VERSION}"
else
    log_info "Checking latest release from GitHub (${REPO})..."
    LATEST_JSON="$(curl -fsSL -H "Accept: application/vnd.github.v3+json" "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null || true)"
    if [[ -n "${LATEST_JSON}" ]]; then
        RELEASE_TAG="$(echo "${LATEST_JSON}" | grep -o '"tag_name": *"[^"]*"' | head -n 1 | cut -d'"' -f4)"
        CLEAN_VERSION="${RELEASE_TAG#v}"
    fi

    # Fallback default if GitHub API is unreachable or empty
    if [[ -z "${CLEAN_VERSION}" ]]; then
        CLEAN_VERSION="0.1.0"
        RELEASE_TAG="v${CLEAN_VERSION}"
        log_warn "Could not resolve latest release via API; defaulting to ${RELEASE_TAG}."
    else
        log_info "Resolved latest release: ${BOLD}${RELEASE_TAG}${RESET}"
    fi
fi

# 5. Resolve Asset filename based on format
case "${PACKAGE_FORMAT}" in
    deb)
        ASSET_NAME="orbit_${CLEAN_VERSION}-1_amd64.deb"
        # Support both underscore and hyphen naming
        FALLBACK_ASSET="orbit_${CLEAN_VERSION}_amd64.deb"
        ;;
    rpm|zypper-rpm)
        ASSET_NAME="orbit-${CLEAN_VERSION}-1.x86_64.rpm"
        FALLBACK_ASSET=""
        ;;
    arch)
        ASSET_NAME="orbit-${CLEAN_VERSION}-1-x86_64.pkg.tar.zst"
        FALLBACK_ASSET=""
        ;;
    appimage)
        ASSET_NAME="Orbit-${CLEAN_VERSION}-x86_64.AppImage"
        FALLBACK_ASSET=""
        ;;
    nix)
        ASSET_NAME=""
        FALLBACK_ASSET=""
        ;;
esac

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${RELEASE_TAG}/${ASSET_NAME}"

log_info "System details:"
echo "  • Operating System: ${OS}"
echo "  • Architecture:     ${ARCH}"
echo "  • Detected Distro:  ${DISTRO_ID} (${DISTRO_LIKE:-standalone})"
echo "  • Install Strategy: ${PACKAGE_FORMAT}"
if [[ -n "${ASSET_NAME}" ]]; then
    echo "  • Target Asset:     ${ASSET_NAME}"
    echo "  • Download URL:     ${DOWNLOAD_URL}"
fi

if [[ "${DRY_RUN}" == "true" ]]; then
    log_info "Dry-run execution completed. No changes were made."
    exit 0
fi

# 6. Perform Installation
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

cd "${TMP_DIR}"

if [[ "${PACKAGE_FORMAT}" == "nix" ]]; then
    log_info "Installing Orbit via Nix profile..."
    run_cmd nix profile install "github:${REPO}"
    log_success "Orbit installed via Nix!"
    exit 0
fi

log_info "Downloading ${ASSET_NAME}..."
if ! curl -fsSL -o "${ASSET_NAME}" "${DOWNLOAD_URL}"; then
    if [[ -n "${FALLBACK_ASSET}" ]]; then
        log_warn "Primary URL failed, attempting fallback asset: ${FALLBACK_ASSET}..."
        FALLBACK_URL="https://github.com/${REPO}/releases/download/${RELEASE_TAG}/${FALLBACK_ASSET}"
        curl -fsSL -o "${FALLBACK_ASSET}" "${FALLBACK_URL}"
        ASSET_NAME="${FALLBACK_ASSET}"
    else
        log_error "Failed to download asset from: ${DOWNLOAD_URL}"
        exit 1
    fi
fi

# Optional checksum validation if SHA256SUMS.txt exists in release
CHECKSUMS_URL="https://github.com/${REPO}/releases/download/${RELEASE_TAG}/SHA256SUMS.txt"
if curl -fsSL -o "SHA256SUMS.txt" "${CHECKSUMS_URL}" 2>/dev/null; then
    log_info "Verifying SHA256 checksum..."
    if grep "${ASSET_NAME}" SHA256SUMS.txt >/dev/null 2>&1; then
        sha256sum --check --ignore-missing SHA256SUMS.txt
        log_success "Checksum verified."
    fi
fi

# Install the downloaded package
case "${PACKAGE_FORMAT}" in
    deb)
        log_info "Installing Debian package..."
        if command -v apt-get >/dev/null 2>&1; then
            run_sudo_cmd apt-get install -y "./${ASSET_NAME}"
        else
            run_sudo_cmd dpkg -i "./${ASSET_NAME}" || run_sudo_cmd apt-get install -f -y
        fi
        ;;
    rpm)
        log_info "Installing RPM package with dnf..."
        run_sudo_cmd dnf install -y "./${ASSET_NAME}"
        ;;
    zypper-rpm)
        log_info "Installing RPM package with zypper..."
        run_sudo_cmd zypper --no-confirm install "./${ASSET_NAME}"
        ;;
    arch)
        log_info "Installing Arch package with pacman..."
        run_sudo_cmd pacman -U --noconfirm "./${ASSET_NAME}"
        ;;
    appimage)
        log_info "Setting up local user AppImage..."
        mkdir -p "${HOME}/.local/bin"
        mkdir -p "${HOME}/.local/share/applications"
        mkdir -p "${HOME}/.local/share/icons/hicolor/512x512/apps"

        DEST_BIN="${HOME}/.local/bin/orbit"
        install -m755 "./${ASSET_NAME}" "${DEST_BIN}"

        # Create user desktop entry
        cat > "${HOME}/.local/share/applications/${APP_ID}.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Orbit
GenericName=Note Taking Application
Comment=Local-first, encrypted workspace for notes and diagrams
Exec=${DEST_BIN} %F
Icon=orbit
Terminal=false
Categories=Utility;TextEditor;
MimeType=text/markdown;text/plain;
StartupWMClass=orbit
StartupNotify=true
Keywords=notes;markdown;diagrams;vault;encryption;editor;
EOF

        # Extract icon if possible
        if command -v update-desktop-database >/dev/null 2>&1; then
            update-desktop-database "${HOME}/.local/share/applications/" 2>/dev/null || true
        fi
        ;;
esac

log_success "Orbit has been successfully installed!"
echo -e "\nYou can launch Orbit by typing '${BOLD}orbit${RESET}' or from your application menu."
