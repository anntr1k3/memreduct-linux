#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "=== Mem Reduct Linux - Build & Installation ==="

# Build release binaries
echo "[1/4] Building release binaries via Cargo..."
export PATH="$HOME/.cargo/bin:$PATH"
cd "$ROOT_DIR"
cargo build --release

# Install binaries to /usr/local/bin
echo "[2/4] Installing binaries to /usr/local/bin..."
sudo install -m 755 "$ROOT_DIR/target/release/memreduct" /usr/local/bin/memreduct
sudo install -m 755 "$ROOT_DIR/target/release/memreduct-daemon" /usr/local/bin/memreduct-daemon
sudo install -m 755 "$ROOT_DIR/target/release/memreduct-tray" /usr/local/bin/memreduct-tray

# Install systemd service
echo "[3/4] Installing systemd service..."
sudo install -m 644 "$ROOT_DIR/systemd/memreduct.service" /etc/systemd/system/memreduct.service
sudo systemctl daemon-reload

# Install Polkit policy
echo "[4/4] Installing Polkit policy..."
if [ -d /usr/share/polkit-1/actions ]; then
    sudo install -m 644 "$ROOT_DIR/polkit/org.memreduct.policy" /usr/share/polkit-1/actions/org.memreduct.policy
fi

# Create autostart entry for tray app (optional)
AUTOSTART_DIR="$HOME/.config/autostart"
mkdir -p "$AUTOSTART_DIR"
cat << 'EOF' > "$AUTOSTART_DIR/memreduct-tray.desktop"
[Desktop Entry]
Type=Application
Name=Mem Reduct Tray
Comment=System Memory Monitor & Cleaner Indicator
Exec=/usr/local/bin/memreduct-tray
Icon=utilities-system-monitor
Terminal=false
Categories=System;Monitor;
X-GNOME-Autostart-enabled=true
EOF

echo ""
echo "Installation completed successfully!"
echo ""
echo "To start the background daemon:"
echo "  sudo systemctl enable --now memreduct"
echo ""
echo "To run the tray indicator:"
echo "  memreduct-tray &"
echo ""
echo "To check memory in terminal:"
echo "  memreduct status"
echo ""
echo "To clean memory:"
echo "  memreduct clean --all"
