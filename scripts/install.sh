#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "=== Mem Reduct Linux - Build & Installation ==="

# Build release binaries
echo "[1/5] Building release binaries via Cargo..."
export PATH="$HOME/.cargo/bin:$PATH"
cd "$ROOT_DIR"
cargo build --release

# Install binaries to /usr/local/bin
echo "[2/5] Installing binaries to /usr/local/bin..."
sudo install -m 755 "$ROOT_DIR/target/release/memreduct" /usr/local/bin/memreduct
sudo install -m 755 "$ROOT_DIR/target/release/memreduct-daemon" /usr/local/bin/memreduct-daemon
sudo install -m 755 "$ROOT_DIR/target/release/memreduct-tray" /usr/local/bin/memreduct-tray

# Install systemd service
echo "[3/5] Installing systemd service..."
sudo install -m 644 "$ROOT_DIR/systemd/memreduct.service" /etc/systemd/system/memreduct.service
sudo systemctl daemon-reload

# Install Polkit policy
echo "[4/5] Installing Polkit policy..."
if [ -d /usr/share/polkit-1/actions ]; then
    sudo install -m 644 "$ROOT_DIR/polkit/org.memreduct.policy" /usr/share/polkit-1/actions/org.memreduct.policy
fi

# Install Desktop Application entry & icon
echo "[5/5] Installing Desktop entry and App icon..."
sudo mkdir -p /usr/share/icons/hicolor/scalable/apps
sudo install -m 644 "$ROOT_DIR/assets/icon.svg" /usr/share/icons/hicolor/scalable/apps/memreduct.svg
sudo install -m 644 "$ROOT_DIR/assets/memreduct.desktop" /usr/share/applications/memreduct.desktop

# Create autostart entry for tray app
AUTOSTART_DIR="$HOME/.config/autostart"
mkdir -p "$AUTOSTART_DIR"
cat << 'EOF' > "$AUTOSTART_DIR/memreduct-tray.desktop"
[Desktop Entry]
Type=Application
Name=Mem Reduct Tray
Comment=System Memory Monitor & Cleaner Indicator
Exec=/usr/local/bin/memreduct-tray
Icon=memreduct
Terminal=false
Categories=System;Monitor;
X-GNOME-Autostart-enabled=true
EOF

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    sudo gtk-update-icon-cache -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

echo ""
echo "Installation completed successfully!"
echo ""
echo "Now you can:"
echo " 1. Search 'Mem Reduct' in GNOME Applications and pin it to Dock"
echo " 2. Restart tray indicator: killall memreduct-tray; memreduct-tray &"
echo " 3. Clean memory from hotkey: memreduct clean --all --notify"
