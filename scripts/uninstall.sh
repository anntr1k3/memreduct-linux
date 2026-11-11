#!/usr/bin/env bash
set -e

echo "=== Mem Reduct Linux - Uninstallation ==="

echo "Stopping services..."
sudo systemctl stop memreduct.service 2>/dev/null || true
sudo systemctl disable memreduct.service 2>/dev/null || true

echo "Removing installed files..."
sudo rm -f /usr/local/bin/memreduct
sudo rm -f /usr/local/bin/memreduct-daemon
sudo rm -f /usr/local/bin/memreduct-tray
sudo rm -f /etc/systemd/system/memreduct.service
sudo rm -f /usr/share/polkit-1/actions/org.memreduct.policy
rm -f "$HOME/.config/autostart/memreduct-tray.desktop"

sudo systemctl daemon-reload

echo "Uninstallation complete."
