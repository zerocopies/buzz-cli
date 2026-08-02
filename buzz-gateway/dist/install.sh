#!/usr/bin/env bash
set -euo pipefail

# buzz-gateway system-wide install (deck slide 02: "an IT team deploys it
# machine-wide"). Run as root on the target machine, from inside an
# extracted release tarball produced by package.sh — expects ./buzz-cli,
# ./buzz-gateway, and ./buzz-gateway.service to sit next to this script.
#
# Idempotent: safe to re-run (e.g. after an upgrade) — skips creating the
# service account/config if they already exist, but always refreshes the
# installed binaries and unit file.

SERVICE_USER="buzz-gateway"
SERVICE_HOME="/var/lib/buzz-gateway"
BIN_DIR="/usr/local/bin"
UNIT_DIR="/etc/systemd/system"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root (installs a system service + system user)" >&2
    exit 1
fi

for f in buzz-cli buzz-gateway buzz-gateway.service; do
    if [[ ! -f "$SCRIPT_DIR/$f" ]]; then
        echo "error: missing $f next to install.sh — run this from an extracted release tarball" >&2
        exit 1
    fi
done

# Dedicated unprivileged system account. --system gives it a locked
# password and a low, non-login UID; --create-home gives it a real home
# directory so ~/.buzz/config.toml has somewhere to live — matched by
# the HOME=/WorkingDirectory= in buzz-gateway.service (see that file's
# header comment for why this can't just rely on systemd's defaults).
if ! id "$SERVICE_USER" &>/dev/null; then
    useradd --system --create-home --home-dir "$SERVICE_HOME" \
        --shell /usr/sbin/nologin "$SERVICE_USER"
    echo "created system user $SERVICE_USER (home: $SERVICE_HOME)"
else
    echo "system user $SERVICE_USER already exists, leaving it as-is"
fi

install -m 0755 -o root -g root "$SCRIPT_DIR/buzz-cli" "$BIN_DIR/buzz-cli"
install -m 0755 -o root -g root "$SCRIPT_DIR/buzz-gateway" "$BIN_DIR/buzz-gateway"
echo "installed binaries to $BIN_DIR"

mkdir -p "$SERVICE_HOME/.buzz"
chown -R "$SERVICE_USER:$SERVICE_USER" "$SERVICE_HOME"
chmod 700 "$SERVICE_HOME/.buzz"

if [[ ! -f "$SERVICE_HOME/.buzz/config.toml" ]]; then
    touch "$SERVICE_HOME/.buzz/config.toml"
    chown "$SERVICE_USER:$SERVICE_USER" "$SERVICE_HOME/.buzz/config.toml"
    chmod 600 "$SERVICE_HOME/.buzz/config.toml"
    echo "wrote an empty starter config to $SERVICE_HOME/.buzz/config.toml"
    echo "  (buzz-gateway will start on this with local-only routing —"
    echo "   startup logs will name exactly which provider keys are"
    echo "   missing; see the README's Operations > Config validation)"
    echo "  fill in provider keys with:"
    echo "    sudo -u $SERVICE_USER $BIN_DIR/buzz-cli --setup"
else
    echo "config already exists at $SERVICE_HOME/.buzz/config.toml, leaving it as-is"
fi

install -m 0644 "$SCRIPT_DIR/buzz-gateway.service" "$UNIT_DIR/buzz-gateway.service"
systemctl daemon-reload
systemctl enable --now buzz-gateway.service

echo
echo "buzz-gateway installed and running as system user '$SERVICE_USER'."
echo "  status: systemctl status buzz-gateway"
echo "  logs:   journalctl -u buzz-gateway -o cat -f | jq ."
echo "  token:  sudo cat $SERVICE_HOME/.buzz/gateway.token"
