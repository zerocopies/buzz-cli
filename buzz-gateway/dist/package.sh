#!/usr/bin/env bash
set -euo pipefail

# Builds release binaries and stages a tarball an IT team can copy to a
# target machine and run install.sh from (deck slide 02: "machine-wide"
# deploy). See README.md's Operations > Packaging section for why this
# is a plain tarball rather than extending qfz3's Tauri bundler.
#
# Linux only — the systemd unit this packages is Linux-specific, matching
# this crate's existing single-user-Unix-like-machine threat model
# (README.md, "Deliberately out of scope").

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERSION="$(grep -m1 '^version' "$WORKSPACE_ROOT/buzz-gateway/Cargo.toml" | cut -d '"' -f2)"
ARCH="$(uname -m)"
STAGE_NAME="buzz-gateway-${VERSION}-linux-${ARCH}"
STAGE_DIR="$WORKSPACE_ROOT/target/package/${STAGE_NAME}"

cd "$WORKSPACE_ROOT"
cargo build --release --bin buzz-cli --bin buzz-gateway

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"
cp target/release/buzz-cli target/release/buzz-gateway "$STAGE_DIR/"
cp "$SCRIPT_DIR/buzz-gateway.service" "$SCRIPT_DIR/install.sh" "$STAGE_DIR/"
chmod +x "$STAGE_DIR/install.sh"

TARBALL="${STAGE_DIR}.tar.gz"
tar -C "$(dirname "$STAGE_DIR")" -czf "$TARBALL" "$STAGE_NAME"

echo "packaged: $TARBALL"
echo "on the target machine: tar xzf $(basename "$TARBALL") && cd ${STAGE_NAME} && sudo ./install.sh"
