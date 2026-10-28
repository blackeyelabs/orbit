#!/usr/bin/env bash
# Generates standard Freedesktop hicolor PNG icons for Orbit across common resolutions.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

SOURCE_LOGO="${REPO_ROOT}/assets/logo/OrbitDarkNobg.png"
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

echo "Generating hicolor icons from ${SOURCE_LOGO} using ${IM_CMD}..."

for size in "${SIZES[@]}"; do
    TARGET_DIR="${OUTPUT_DIR}/${size}x${size}/apps"
    mkdir -p "${TARGET_DIR}"
    TARGET_FILE="${TARGET_DIR}/orbit.png"
    
    "${IM_CMD}" "${SOURCE_LOGO}" -resize "${size}x${size}" "${TARGET_FILE}"
    echo "  -> Created ${TARGET_FILE} (${size}x${size})"
done

# Scalable / high-resolution fallback
SCALABLE_DIR="${OUTPUT_DIR}/scalable/apps"
mkdir -p "${SCALABLE_DIR}"
cp "${SOURCE_LOGO}" "${SCALABLE_DIR}/orbit.png"
echo "  -> Copied scalable fallback to ${SCALABLE_DIR}/orbit.png"

echo "All icons generated successfully under ${OUTPUT_DIR}."
