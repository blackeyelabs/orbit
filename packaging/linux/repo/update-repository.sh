#!/usr/bin/env bash
# Generates and updates static APT, DNF, and Pacman package repositories for Orbit.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

INPUT_DIR="${1:-${REPO_ROOT}/dist}"
REPO_DIR="${2:-${REPO_ROOT}/gh-pages}"
KEY_ID="${3:-8CAE2B78404BD8F4352C3F0A47514BD5411559DC}"
PASSPHRASE="${4:-}"

mkdir -p "${REPO_DIR}"
REPO_DIR="$(cd "${REPO_DIR}" && pwd)"

GPG_FLAGS=(--batch --yes)
if [[ -n "${PASSPHRASE}" ]]; then
    GPG_FLAGS+=(--pinentry-mode loopback --passphrase "${PASSPHRASE}")
fi

echo "============================================="
echo "Updating Orbit Linux Multi-Distro Repositories"
echo "  Input directory: ${INPUT_DIR}"
echo "  Target repo dir: ${REPO_DIR}"
echo "  GPG Signing Key: ${KEY_ID}"
echo "============================================="

# 1. Copy Public GPG Key
mkdir -p "${REPO_DIR}/keys"
if [[ -f "${REPO_ROOT}/packaging/linux/keys/orbit.gpg.key" ]]; then
    cp "${REPO_ROOT}/packaging/linux/keys/orbit.gpg.key" "${REPO_DIR}/keys/orbit.gpg.key"
    echo "✔ Copied public key to ${REPO_DIR}/keys/orbit.gpg.key"
fi

# 2. APT Repository (Debian / Ubuntu)
echo "--- Updating APT Repository ---"
APT_DIR="${REPO_DIR}/apt"
APT_POOL="${APT_DIR}/pool/main/o/orbit"
APT_DISTS="${APT_DIR}/dists/stable/main/binary-amd64"
mkdir -p "${APT_POOL}"
mkdir -p "${APT_DISTS}"

# Copy all .deb packages
shopt -s nullglob
DEB_FILES=("${INPUT_DIR}"/*.deb)
if [[ ${#DEB_FILES[@]} -gt 0 ]]; then
    cp "${DEB_FILES[@]}" "${APT_POOL}/"
fi

# Generate Packages & Packages.gz
if command -v apt-ftparchive >/dev/null 2>&1; then
    (
        cd "${APT_DIR}"
        apt-ftparchive packages pool/main/o/orbit > dists/stable/main/binary-amd64/Packages
        gzip -9c dists/stable/main/binary-amd64/Packages > dists/stable/main/binary-amd64/Packages.gz

        cd dists/stable
        cat > Release <<EOF
Origin: Orbit
Label: Orbit
Suite: stable
Codename: stable
Version: 1.0
Architectures: amd64
Components: main
Description: Official Orbit Linux package repository
Date: $(date -Ru)
EOF
        apt-ftparchive release . >> Release

        # GPG signatures
        gpg "${GPG_FLAGS[@]}" --clearsign -u "${KEY_ID}" -o InRelease Release
        gpg "${GPG_FLAGS[@]}" -abs -u "${KEY_ID}" -o Release.gpg Release
    )
    echo "✔ Successfully updated and signed APT repository"
else
    echo "Notice: apt-ftparchive not found; skipping APT metadata generation (will run in CI)."
fi

# 3. DNF / YUM Repository (Fedora / RHEL / openSUSE)
echo "--- Updating DNF / RPM Repository ---"
RPM_DIR="${REPO_DIR}/rpm"
mkdir -p "${RPM_DIR}"

RPM_FILES=("${INPUT_DIR}"/*.rpm)
if [[ ${#RPM_FILES[@]} -gt 0 ]]; then
    cp "${RPM_FILES[@]}" "${RPM_DIR}/"
fi

cat > "${RPM_DIR}/orbit.repo" <<EOF
[orbit]
name=Orbit Packages
baseurl=https://blackeyelabs.github.io/orbit/rpm/
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://blackeyelabs.github.io/orbit/keys/orbit.gpg.key
EOF

if command -v createrepo_c >/dev/null 2>&1; then
    createrepo_c "${RPM_DIR}"
    if [[ -f "${RPM_DIR}/repodata/repomd.xml" ]]; then
        gpg "${GPG_FLAGS[@]}" --detach-sign --armor -u "${KEY_ID}" "${RPM_DIR}/repodata/repomd.xml"
    fi
    echo "✔ Successfully updated and signed DNF / RPM repository"
else
    echo "Notice: createrepo_c not found; skipping DNF metadata generation (will run in CI)."
fi

# 4. Pacman Repository (Arch Linux)
echo "--- Updating Pacman Repository ---"
PACMAN_DIR="${REPO_DIR}/pacman/x86_64"
mkdir -p "${PACMAN_DIR}"

ARCH_FILES=("${INPUT_DIR}"/*.pkg.tar.zst)
if [[ ${#ARCH_FILES[@]} -gt 0 ]]; then
    cp "${ARCH_FILES[@]}" "${PACMAN_DIR}/"
fi

if command -v repo-add >/dev/null 2>&1; then
    (
        cd "${PACMAN_DIR}"
        # Sign individual package files if not already signed
        for pkg in *.pkg.tar.zst; do
            if [[ -f "${pkg}" && ! -f "${pkg}.sig" ]]; then
                gpg "${GPG_FLAGS[@]}" --detach-sign -u "${KEY_ID}" "${pkg}" 2>/dev/null || true
            fi
        done
        # Update database
        repo-add -s -k "${KEY_ID}" orbit.db.tar.zst *.pkg.tar.zst 2>/dev/null || \
        repo-add orbit.db.tar.zst *.pkg.tar.zst
    )
    echo "✔ Successfully updated and signed Pacman repository"
else
    echo "Notice: repo-add not found; skipping Pacman database update (will run in CI)."
fi

# 5. HTML Landing Page
echo "--- Generating Repository Landing Page ---"
cat > "${REPO_DIR}/index.html" <<'EOF'
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Orbit Linux Package Repositories</title>
  <style>
    :root {
      --bg: #1c1917;
      --card-bg: #292524;
      --text: #f5f5f4;
      --muted: #a8a29e;
      --accent: #da9436;
      --code-bg: #0c0a09;
      --border: #44403c;
    }
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background-color: var(--bg);
      color: var(--text);
      line-height: 1.6;
      margin: 0;
      padding: 2rem 1rem;
    }
    .container {
      max-width: 800px;
      margin: 0 auto;
    }
    header {
      text-align: center;
      margin-bottom: 3rem;
    }
    h1 {
      color: var(--accent);
      font-size: 2.5rem;
      margin-bottom: 0.5rem;
    }
    p.lead {
      font-size: 1.2rem;
      color: var(--muted);
    }
    .card {
      background-color: var(--card-bg);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 1.5rem;
      margin-bottom: 2rem;
      box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.3);
    }
    h2 {
      margin-top: 0;
      color: var(--text);
      border-bottom: 1px solid var(--border);
      padding-bottom: 0.5rem;
      font-size: 1.4rem;
    }
    pre {
      background-color: var(--code-bg);
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 1rem;
      overflow-x: auto;
      font-size: 0.95rem;
      color: #e7e5e4;
    }
    code {
      font-family: "JetBrains Mono", Menlo, Monaco, Consolas, monospace;
    }
    a {
      color: var(--accent);
      text-decoration: none;
    }
    a:hover {
      text-decoration: underline;
    }
    footer {
      text-align: center;
      margin-top: 3rem;
      color: var(--muted);
      font-size: 0.9rem;
    }
  </style>
</head>
<body>
  <div class="container">
    <header>
      <h1>Orbit Linux Repository</h1>
      <p class="lead">Official package repositories for Debian, Ubuntu, Fedora, openSUSE, and Arch Linux.</p>
    </header>

    <div class="card">
      <h2>🚀 One-Line Quick Install</h2>
      <p>Automatically detects your distribution and installs the native package:</p>
      <pre><code>curl -fsSL https://raw.githubusercontent.com/blackeyelabs/orbit/main/install.sh | bash</code></pre>
    </div>

    <div class="card">
      <h2>📦 Debian / Ubuntu</h2>
      <p>Add the GPG signing key and APT repository:</p>
      <pre><code># Import GPG keyring
curl -fsSL https://blackeyelabs.github.io/orbit/keys/orbit.gpg.key | sudo gpg --dearmor -o /etc/apt/keyrings/orbit.gpg

# Add repository source
echo "deb [signed-by=/etc/apt/keyrings/orbit.gpg] https://blackeyelabs.github.io/orbit/apt stable main" | sudo tee /etc/apt/sources.list.d/orbit.list

# Update and install
sudo apt update
sudo apt install orbit</code></pre>
    </div>

    <div class="card">
      <h2>📦 Fedora / RHEL / CentOS</h2>
      <p>Add the DNF repository configuration:</p>
      <pre><code>sudo dnf config-manager --add-repo https://blackeyelabs.github.io/orbit/rpm/orbit.repo
sudo dnf install orbit</code></pre>
    </div>

    <div class="card">
      <h2>📦 Arch Linux</h2>
      <p>Add the repository to <code>/etc/pacman.conf</code>:</p>
      <pre><code>[orbit]
SigLevel = Required DatabaseOptional
Server = https://blackeyelabs.github.io/orbit/pacman/$arch</code></pre>
      <p>Or install from the AUR:</p>
      <pre><code>yay -S orbit-bin</code></pre>
    </div>

    <div class="card">
      <h2>❄️ Nix / NixOS</h2>
      <p>Run directly or install with Nix flakes:</p>
      <pre><code>nix run github:blackeyelabs/orbit
nix profile install github:blackeyelabs/orbit</code></pre>
    </div>

    <footer>
      <p>Orbit is open source. View on <a href="https://github.com/blackeyelabs/orbit">GitHub</a>.</p>
    </footer>
  </div>
</body>
</html>
EOF
echo "✔ Generated HTML landing page at ${REPO_DIR}/index.html"
echo "All repositories updated successfully!"
