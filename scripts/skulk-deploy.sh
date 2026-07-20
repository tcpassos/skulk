#!/usr/bin/env bash
# Skulk deploy / update for a Raspberry Pi (or any Linux SBC).
#
# Idempotent: run it once to install, run it again to update. The live config at
# /etc/skulk/skulk.toml is created ONCE from the shipped example and is NEVER
# overwritten by an update, so your edits (peripherals, nav, listen address,
# implant id) survive every upgrade.
#
#   First install:
#     sudo SKULK_REPO=owner/name bash skulk-deploy.sh
#   Update anytime after that:
#     sudo skulk-update
#
# Env overrides:
#   SKULK_REPO     GitHub "owner/name" of the Skulk repo (saved on first run)
#   SKULK_TARGET   Rust target triple (default: auto-detected from `uname -m`)
#   GITHUB_TOKEN   token for a private repo (used by curl / gh)

set -euo pipefail

PREFIX="/opt/skulk"
CONFIG_DIR="/etc/skulk"
CONFIG="$CONFIG_DIR/skulk.toml"
ENV_FILE="$CONFIG_DIR/deploy.env"
UNIT="/etc/systemd/system/skulkd.service"

die() { echo "skulk-deploy: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root (use sudo)"

mkdir -p "$CONFIG_DIR"

# --- resolve the repo: env override > saved (deploy.env) > upstream default ---
[ -f "$ENV_FILE" ] && . "$ENV_FILE"
REPO="${SKULK_REPO:-${REPO:-tcpassos/skulk}}"

# --- detect the target architecture ------------------------------------------
if [ -z "${SKULK_TARGET:-}" ]; then
  case "$(uname -m)" in
    armv7l | armv6l) SKULK_TARGET="armv7-unknown-linux-gnueabihf" ;;
    aarch64 | arm64) SKULK_TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)          SKULK_TARGET="x86_64-unknown-linux-gnu" ;;
    *) die "unsupported arch '$(uname -m)'; set SKULK_TARGET explicitly" ;;
  esac
fi
TAG="latest-$SKULK_TARGET"
ASSET="skulkd-$SKULK_TARGET.tar.gz"
URL="https://github.com/$REPO/releases/download/$TAG/$ASSET"

echo "==> Skulk deploy   repo=$REPO   target=$SKULK_TARGET"

# --- download + extract the rolling latest release ---------------------------
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
echo "==> fetching $ASSET"
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  gh release download "$TAG" --repo "$REPO" --pattern "$ASSET" --dir "$tmp" --clobber
else
  curl -fSL ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} \
    -o "$tmp/$ASSET" "$URL" || die "download failed ($URL)"
fi
tar -xzf "$tmp/$ASSET" -C "$tmp"
src="$tmp/skulkd-$SKULK_TARGET"
[ -x "$src/skulkd" ] || die "tarball has no skulkd binary"

# --- install the binary, the example config, and (if shipped) themes ---------
install -Dm755 "$src/skulkd" "$PREFIX/skulkd"
install -Dm644 "$src/skulk.toml" "$PREFIX/skulk.toml.example"
[ -d "$src/themes" ] && cp -r "$src/themes" "$PREFIX/"

# install this script as `skulk-update` for one-command upgrades
install -Dm755 "$0" "$PREFIX/skulk-deploy.sh" 2>/dev/null || true
ln -sf "$PREFIX/skulk-deploy.sh" /usr/local/bin/skulk-update

# remember the repo/target for `skulk-update`
{ echo "REPO=$REPO"; echo "SKULK_TARGET=$SKULK_TARGET"; } > "$ENV_FILE"
chmod 600 "$ENV_FILE"

# --- the config: create once, never clobber ---------------------------------
first_run=0
if [ ! -f "$CONFIG" ]; then
  first_run=1
  cp "$PREFIX/skulk.toml.example" "$CONFIG"
  # a planted Pi listens on all interfaces so the operator can reach it
  sed -i 's|^addr = "127.0.0.1:9000"|addr = "0.0.0.0:9000"|' "$CONFIG"
  echo "==> created $CONFIG"
else
  echo "==> kept your $CONFIG (updates never touch it)"
fi

# --- systemd unit (created once) ---------------------------------------------
if [ ! -f "$UNIT" ]; then
  cat > "$UNIT" <<EOF
[Unit]
Description=Skulk implant daemon
After=network-online.target
Wants=network-online.target

[Service]
# Root: GPIO/SPI (LCD, buttons) and raw sockets (host discovery) need it.
User=root
ExecStart=$PREFIX/skulkd $CONFIG
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable skulkd >/dev/null 2>&1 || true
fi

# --- (re)start and report ----------------------------------------------------
systemctl restart skulkd
sleep 1
systemctl --no-pager --lines=0 status skulkd || true

echo
if [ "$first_run" -eq 1 ]; then
  cat <<EOF
==> First install done. Now tailor your config once:
      sudo nano $CONFIG      # id, [display], [[peripherals]], [nav]
      sudo systemctl restart skulkd

    Update anytime with:  sudo skulk-update
    Logs:                 journalctl -u skulkd -f
EOF
else
  echo "==> Updated. Your config was preserved.  Logs: journalctl -u skulkd -f"
fi
