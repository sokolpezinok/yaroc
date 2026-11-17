#!/bin/bash
# setup_yaroc.sh - Clones yaroc sample config files and installs systemd services and dependencies
set -e

REPO_URL=${1:-"https://github.com/sokolpezinok/yaroc"}
YAROC_SPEC=${2:-${YAROC_SPEC:-"yaroc[display]"}}
YAROC_REF=${YAROC_REF:-}

if [ ! -d "/etc/systemd/system" ]; then
    echo "Error: This script must be run on a system with /etc/systemd/system directory."
    exit 1
fi

export DEBIAN_FRONTEND=noninteractive

echo "Installing and configuring meshtasticd..."
apt update
apt install -y --no-install-recommends curl gpg

# Enable SPI and I2C for RAK6421 Pi HAT
if [ -f /boot/firmware/config.txt ]; then
  grep -q "^dtparam=spi=on" /boot/firmware/config.txt || echo "dtparam=spi=on" >> /boot/firmware/config.txt
  grep -q "^dtoverlay=spi0-0cs" /boot/firmware/config.txt || echo "dtoverlay=spi0-0cs" >> /boot/firmware/config.txt
  grep -q "^dtparam=i2c_arm=on" /boot/firmware/config.txt || echo "dtparam=i2c_arm=on" >> /boot/firmware/config.txt
fi

# Install meshtasticd
DEB_VERSION=$(. /etc/os-release && echo "$VERSION_ID")
ARCH=$(dpkg --print-architecture)
if [ "$ARCH" = "armhf" ]; then
    OS_TYPE="Raspbian"
else
    OS_TYPE="Debian"
fi
REPO_TARGET="${OS_TYPE}_${DEB_VERSION}"

echo "Configuring meshtasticd repository for ${REPO_TARGET} (${ARCH})..."
echo "deb http://download.opensuse.org/repositories/network:/Meshtastic:/beta/${REPO_TARGET}/ /" > /etc/apt/sources.list.d/network:Meshtastic:beta.list
curl -fsSL "https://download.opensuse.org/repositories/network:Meshtastic:beta/${REPO_TARGET}/Release.key" | gpg --dearmor > /etc/apt/trusted.gpg.d/network_Meshtastic_beta.gpg
apt update
apt install -y --no-install-recommends meshtasticd

# Configure for RAK6421 and RAK13300 in slot 2
mkdir -p /etc/meshtasticd/config.d
if [ -f /etc/meshtasticd/available.d/lora-RAK6421-13300-slot2.yaml ]; then
  ln -sf /etc/meshtasticd/available.d/lora-RAK6421-13300-slot2.yaml /etc/meshtasticd/config.d/
fi


echo "Installing uv and $YAROC_SPEC for pi user..."
runuser -u pi -- bash -c '
  set -e
  export HOME=/home/pi
  unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME
  YAROC_SPEC="$1"
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export UV_LINK_MODE=copy
  "$HOME/.local/bin/uv" tool install --force --no-cache "$YAROC_SPEC" \
    --extra-index-url https://www.piwheels.org/simple \
    --index-strategy unsafe-best-match
  if ! grep -q "\.local/bin" "$HOME/.bashrc"; then
    echo '\''export PATH="$HOME/.local/bin:$PATH"'\'' >> "$HOME/.bashrc"
  fi
' _ "$YAROC_SPEC"

echo "Cloning $REPO_URL..."
TEMP_DIR=$(mktemp -d)
if [ -n "$YAROC_REF" ]; then
    git clone --depth 1 --branch "$YAROC_REF" "$REPO_URL" "$TEMP_DIR"
else
    git clone --depth 1 "$REPO_URL" "$TEMP_DIR"
fi

echo "Copying service files to /etc/systemd/system/..."
cp "$TEMP_DIR/conf/send-punch.service" "/etc/systemd/system/"
cp "$TEMP_DIR/conf/yarocd.service" "/etc/systemd/system/"

echo "Reloading systemd manager configuration..."
systemctl daemon-reload || true

echo "Copying sample configuration files to /home/pi/..."
cp "$TEMP_DIR/conf/send-punch.toml.example" "/home/pi/send-punch.toml"
cp "$TEMP_DIR/conf/yarocd.toml.example" "/home/pi/yarocd.toml"
chown pi:pi /home/pi/send-punch.toml /home/pi/yarocd.toml

rm -rf "$TEMP_DIR"
echo "YAROC services installed."
