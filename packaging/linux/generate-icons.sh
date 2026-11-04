#!/usr/bin/env bash
# Generates standard Freedesktop hicolor PNG icons for Orbit across common resolutions.
# A size with its own hand-drawn logo in assets/logo uses that one; the others are
# scaled down from the largest logo. Every icon is padded to a square, as the
# logos aren't all square.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

LOGO_DIR="${REPO_ROOT}/assets/logo"
SOURCE_LOGO="${LOGO_DIR}/Orbit_1254x1254.png"
OUTPUT_DIR="${SCRIPT_DIR}/icons/hicolor"

if [[ ! -f "${SOURCE_LOGO}" ]]; then
    echo "Error: Source logo not found at: ${SOURCE_LOGO}" >&2
    exit 1
fi

# Detect ImageMagick command (magick in v7+, convert in v6)
if command -v magick >/dev/null 2>&1; then
    IM_CMD="magick"
elif command -v convert >/dev/null 2>&1; then
    IM_CMD="convert"
else
    echo "Error: Neither 'magick' nor 'convert' (ImageMagick) was found in PATH." >&2
    exit 1
fi

SIZES=(16 24 32 48 64 128 256 512)

# The hand-drawn logo for a size, if there is one (32's is named Orbit_32_32).
hand_drawn() {
    local size="$1" name
    for name in "Orbit_${size}x${size}.png" "Orbit_${size}_${size}.png"; do
        if [[ -f "${LOGO_DIR}/${name}" ]]; then
            echo "${LOGO_DIR}/${name}"
            return
        fi
    done
}

echo "Generating hicolor icons from ${LOGO_DIR} using ${IM_CMD}..."

for size in "${SIZES[@]}"; do
    TARGET_DIR="${OUTPUT_DIR}/${size}x${size}/apps"
    mkdir -p "${TARGET_DIR}"
    TARGET_FILE="${TARGET_DIR}/orbit.png"

    source="$(hand_drawn "${size}")"
    source="${source:-${SOURCE_LOGO}}"
    "${IM_CMD}" "${source}" -resize "${size}x${size}" \
        -background none -gravity center -extent "${size}x${size}" "${TARGET_FILE}"
    echo "  -> Created ${TARGET_FILE} (${size}x${size}) from $(basename "${source}")"
done

# Scalable / high-resolution fallback
SCALABLE_DIR="${OUTPUT_DIR}/scalable/apps"
mkdir -p "${SCALABLE_DIR}"
cp "${SOURCE_LOGO}" "${SCALABLE_DIR}/orbit.png"
echo "  -> Copied scalable fallback to ${SCALABLE_DIR}/orbit.png"

echo "All icons generated successfully under ${OUTPUT_DIR}."
