# Orbit Project Justfile
# Run `just` or `just --list` to see available recipes.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Dynamic project metadata
version := `grep -m1 '^version = ' Cargo.toml | cut -d '"' -f2`
target_bin := "target/release/orbit"
profiling_bin := "target/profiling/orbit"
dist_dir := "dist"

# Default recipe: list available commands
default:
    @just --list --unsorted

# -----------------------------------------------------------------------------
# Development & Running
# -----------------------------------------------------------------------------

# Run Orbit in debug mode with optional arguments
run *args:
    cargo run -p app -- {{args}}

# Run Orbit in release mode with optional arguments
run-release *args:
    cargo run --release -p app -- {{args}}

# Run Orbit against a specified vault folder
run-vault vault *args:
    cargo run -p app -- "{{vault}}" {{args}}

# -----------------------------------------------------------------------------
# Building
# -----------------------------------------------------------------------------

# Build Orbit binary in debug mode
build:
    cargo build -p app

# Build Orbit binary in release mode
build-release:
    cargo build --release -p app

# Build Orbit with profiling symbols and GPUI dev-profiler feature
build-profiling:
    cargo build --profile profiling -p app --features dev-profiler

# Build all workspace crates and members
build-all:
    cargo build --workspace

# -----------------------------------------------------------------------------
# Quality, Checks & Tests
# -----------------------------------------------------------------------------

# Fast workspace check
check:
    cargo check --workspace --all-targets

# Format Rust source code
fmt:
    cargo fmt --all

# Check Rust source code formatting
fmt-check:
    cargo fmt --all --check

# Run Clippy lints across all workspace targets
clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run workspace unit and integration tests
test *args:
    cargo test --workspace --locked {{args}}

# Run unit tests for core domain crates only
test-core:
    cargo test -p orbit-core -p orbit-fs -p orbit-crypto -p orbit-index -p md-engine -p diagrams

# Run architectural dependency check (ADR 0003)
guard:
    scripts/check-core-deps.sh

# Run test verifying that guard catches forbidden dependencies
test-guard:
    scripts/test-check-core-deps.sh

# Run complete CI test suite (formatting, clippy, tests, architectural guards)
ci:
    scripts/ci.sh

# -----------------------------------------------------------------------------
# Debugging & Tracing
# -----------------------------------------------------------------------------

# Run Orbit with debug-level logging enabled
debug *args:
    ORBIT_LOG="debug" cargo run -p app -- {{args}}

# Run Orbit with trace-level logging enabled
trace *args:
    ORBIT_LOG="trace" cargo run -p app -- {{args}}

# Run Orbit under GDB debugger
gdb *args: build
    gdb --args target/debug/orbit {{args}}

# Run Orbit under LLDB debugger
lldb *args: build
    lldb -- target/debug/orbit {{args}}

# -----------------------------------------------------------------------------
# Performance & Profiling ("perps")
# -----------------------------------------------------------------------------

# Run benchmarks across workspace
bench *args:
    cargo bench --workspace {{args}}

# Generate a deterministic fixture vault with N notes
fixture-vault path notes="5000":
    python3 scripts/make-fixture-vault.py "{{path}}" --notes {{notes}}

# Measure Orbit performance on a scenario (idle, open:<note>, type:<note>)
measure vault scenario="idle" warmup="5" span="10": build-profiling
    scripts/measure.sh "{{profiling_bin}}" "{{vault}}" "{{scenario}}" {{warmup}} {{span}}

# -----------------------------------------------------------------------------
# Linux Packaging
# -----------------------------------------------------------------------------

# Build Debian (.deb) package
package-deb: build-release
    @mkdir -p {{dist_dir}}
    cargo deb -p app --no-build -o {{dist_dir}}/orbit_{{version}}_amd64.deb

# Build RPM (.rpm) package
package-rpm: build-release
    @mkdir -p {{dist_dir}}
    (cd crates/app && cargo generate-rpm --auto-req disabled -o ../../{{dist_dir}}/orbit-{{version}}-1.x86_64.rpm)

# Build Arch Linux (.pkg.tar.zst) package
package-arch: build-release
    @mkdir -p {{dist_dir}}
    packaging/linux/arch/build-arch-package.sh {{version}} {{target_bin}} {{dist_dir}}

# Build universal AppImage
package-appimage: build-release
    @mkdir -p {{dist_dir}}
    packaging/linux/appimage/build-appimage.sh {{version}} {{target_bin}} {{dist_dir}}

# Build generic Linux tarball distribution
package-tarball: build-release
    @mkdir -p {{dist_dir}}
    @rm -rf {{dist_dir}}/orbit-{{version}}-linux-x86_64
    @mkdir -p {{dist_dir}}/orbit-{{version}}-linux-x86_64
    cp {{target_bin}} {{dist_dir}}/orbit-{{version}}-linux-x86_64/
    cp packaging/linux/install-local.sh {{dist_dir}}/orbit-{{version}}-linux-x86_64/
    cp packaging/linux/io.github.blackeyelabs.orbit.desktop {{dist_dir}}/orbit-{{version}}-linux-x86_64/
    cp packaging/linux/io.github.blackeyelabs.orbit.metainfo.xml {{dist_dir}}/orbit-{{version}}-linux-x86_64/
    cp -r packaging/linux/icons {{dist_dir}}/orbit-{{version}}-linux-x86_64/
    tar -czf {{dist_dir}}/orbit-{{version}}-linux-x86_64.tar.gz -C {{dist_dir}} orbit-{{version}}-linux-x86_64
    @rm -rf {{dist_dir}}/orbit-{{version}}-linux-x86_64
    @echo "Created {{dist_dir}}/orbit-{{version}}-linux-x86_64.tar.gz"

# Build all package formats into dist/
package-all: build-release package-deb package-rpm package-arch package-appimage package-tarball
    @echo "All distribution packages generated in {{dist_dir}}/"
    @ls -lh {{dist_dir}}

# -----------------------------------------------------------------------------
# Installation & Local System
# -----------------------------------------------------------------------------

# Install Orbit locally for current user (~/.local)
install-user: build-release
    mkdir -p "${HOME}/.local/bin" "${HOME}/.local/share/applications" "${HOME}/.local/share/metainfo" "${HOME}/.local/share/icons"
    install -m755 {{target_bin}} "${HOME}/.local/bin/orbit"
    install -m644 packaging/linux/io.github.blackeyelabs.orbit.desktop "${HOME}/.local/share/applications/io.github.blackeyelabs.orbit.desktop"
    install -m644 packaging/linux/io.github.blackeyelabs.orbit.metainfo.xml "${HOME}/.local/share/metainfo/io.github.blackeyelabs.orbit.metainfo.xml"
    cp -r packaging/linux/icons/* "${HOME}/.local/share/icons/"
    @command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "${HOME}/.local/share/applications" 2>/dev/null || true
    @command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true
    @echo "Orbit installed successfully to ${HOME}/.local"

# Uninstall Orbit locally for current user
uninstall-user:
    rm -f "${HOME}/.local/bin/orbit"
    rm -f "${HOME}/.local/share/applications/io.github.blackeyelabs.orbit.desktop"
    rm -f "${HOME}/.local/share/metainfo/io.github.blackeyelabs.orbit.metainfo.xml"
    rm -f "${HOME}/.local/share/icons/hicolor/scalable/apps/orbit.png"
    @for size in 16 24 32 48 64 128 256 512; do \
        rm -f "${HOME}/.local/share/icons/hicolor/${size}x${size}/apps/orbit.png"; \
        rm -f "${HOME}/.local/share/icons/hicolor/${size}x${size}/apps/io.github.blackeyelabs.orbit.png"; \
    done
    @command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "${HOME}/.local/share/applications" || true
    @command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true
    @echo "Orbit uninstalled from ~/.local"

# Install Orbit system-wide (invokes install.sh with sudo)
install-system:
    sudo ./install.sh

# -----------------------------------------------------------------------------
# Assets, Validation & Maintenance
# -----------------------------------------------------------------------------

# Regenerate PNG icon sizes from master SVG icon
icons:
    packaging/linux/generate-icons.sh

# Validate desktop entry and AppStream metadata specifications
validate-desktop:
    desktop-file-validate packaging/linux/io.github.blackeyelabs.orbit.desktop
    appstreamcli validate --no-net packaging/linux/io.github.blackeyelabs.orbit.metainfo.xml

# Clean build artifacts and dist directory
clean:
    cargo clean
    rm -rf {{dist_dir}}
