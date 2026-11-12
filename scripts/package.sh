#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

VERSION=$(grep -m1 'version = ' "$ROOT_DIR/Cargo.toml" | cut -d '"' -f 2)
ARCH="amd64"
DIST_DIR="$ROOT_DIR/dist"

echo "=== Packaging Mem Reduct Linux v${VERSION} (${ARCH}) ==="

# 1. Build release binaries
export PATH="$HOME/.cargo/bin:$PATH"
cd "$ROOT_DIR"
echo "[1/4] Compiling optimized release binaries..."
cargo build --release

# Prepare dist directory
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# 2. Package Debian (.deb)
echo "[2/4] Building Debian package (.deb)..."
DEB_BUILD_DIR="$DIST_DIR/build_deb"
mkdir -p "$DEB_BUILD_DIR/DEBIAN"
mkdir -p "$DEB_BUILD_DIR/usr/bin"
mkdir -p "$DEB_BUILD_DIR/usr/share/applications"
mkdir -p "$DEB_BUILD_DIR/usr/share/icons/hicolor/scalable/apps"
mkdir -p "$DEB_BUILD_DIR/usr/share/polkit-1/actions"
mkdir -p "$DEB_BUILD_DIR/etc/systemd/system"
mkdir -p "$DEB_BUILD_DIR/etc/xdg/autostart"

# Binaries
cp "$ROOT_DIR/target/release/memreduct" "$DEB_BUILD_DIR/usr/bin/"
cp "$ROOT_DIR/target/release/memreduct-daemon" "$DEB_BUILD_DIR/usr/bin/"
cp "$ROOT_DIR/target/release/memreduct-tray" "$DEB_BUILD_DIR/usr/bin/"

# Assets & Integration
cp "$ROOT_DIR/assets/memreduct.desktop" "$DEB_BUILD_DIR/usr/share/applications/"
cp "$ROOT_DIR/assets/icon.svg" "$DEB_BUILD_DIR/usr/share/icons/hicolor/scalable/apps/memreduct.svg"
cp "$ROOT_DIR/polkit/org.memreduct.policy" "$DEB_BUILD_DIR/usr/share/polkit-1/actions/"
cp "$ROOT_DIR/systemd/memreduct.service" "$DEB_BUILD_DIR/etc/systemd/system/"

cat << 'EOF' > "$DEB_BUILD_DIR/etc/xdg/autostart/memreduct-tray.desktop"
[Desktop Entry]
Type=Application
Name=Mem Reduct Tray
Comment=System Memory Monitor & Cleaner Indicator
Exec=memreduct-tray
Icon=memreduct
Terminal=false
Categories=System;Monitor;
X-GNOME-Autostart-enabled=true
EOF

# DEBIAN/control
cat << EOF > "$DEB_BUILD_DIR/DEBIAN/control"
Package: memreduct
Version: ${VERSION}
Architecture: ${ARCH}
Maintainer: anntr1k3 <https://github.com/anntr1k3/memreduct-linux>
Section: utils
Priority: optional
Homepage: https://github.com/anntr1k3/memreduct-linux
Description: Lightweight system memory monitor and cleaner for Linux
 Pure Rust port of Mem Reduct for Linux.
 Features real-time memory usage badge in system tray, background
 cache dropping, memory compaction, safe swap flush, and dual
 Russian & English language support.
EOF

# DEBIAN/postinst
cat << 'EOF' > "$DEB_BUILD_DIR/DEBIAN/postinst"
#!/bin/sh
set -e

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
    systemctl enable --now memreduct.service 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
fi
EOF
chmod 755 "$DEB_BUILD_DIR/DEBIAN/postinst"

# DEBIAN/prerm
cat << 'EOF' > "$DEB_BUILD_DIR/DEBIAN/prerm"
#!/bin/sh
set -e

if command -v systemctl >/dev/null 2>&1; then
    systemctl stop memreduct.service 2>/dev/null || true
    systemctl disable memreduct.service 2>/dev/null || true
fi
EOF
chmod 755 "$DEB_BUILD_DIR/DEBIAN/prerm"

dpkg-deb --build --root-owner-group "$DEB_BUILD_DIR" "$DIST_DIR/memreduct_${VERSION}_${ARCH}.deb"
rm -rf "$DEB_BUILD_DIR"

# 3. Package standalone tar.gz bundle
echo "[3/4] Building standalone tar.gz bundle..."
TAR_DIR="$DIST_DIR/memreduct-v${VERSION}-linux-x86_64"
mkdir -p "$TAR_DIR/bin"
mkdir -p "$TAR_DIR/systemd"
mkdir -p "$TAR_DIR/polkit"
mkdir -p "$TAR_DIR/assets"

cp "$ROOT_DIR/target/release/memreduct" "$TAR_DIR/bin/"
cp "$ROOT_DIR/target/release/memreduct-daemon" "$TAR_DIR/bin/"
cp "$ROOT_DIR/target/release/memreduct-tray" "$TAR_DIR/bin/"
cp "$ROOT_DIR/systemd/memreduct.service" "$TAR_DIR/systemd/"
cp "$ROOT_DIR/polkit/org.memreduct.policy" "$TAR_DIR/polkit/"
cp "$ROOT_DIR/assets/icon.svg" "$TAR_DIR/assets/"
cp "$ROOT_DIR/assets/memreduct.desktop" "$TAR_DIR/assets/"
cp "$ROOT_DIR/README.md" "$TAR_DIR/"
cp "$ROOT_DIR/README.ru.md" "$TAR_DIR/" 2>/dev/null || true
cp "$ROOT_DIR/LICENSE" "$TAR_DIR/"

cat << 'EOF' > "$TAR_DIR/install.sh"
#!/usr/bin/env bash
set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== Installing Mem Reduct Linux ==="

# Check root/sudo
SUDO=""
if [ "$EUID" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    else
        echo "Error: root privileges or sudo required." >&2
        exit 1
    fi
fi

echo "[1/4] Installing binaries..."
$SUDO install -m 755 "$DIR/bin/memreduct" /usr/local/bin/memreduct
$SUDO install -m 755 "$DIR/bin/memreduct-daemon" /usr/local/bin/memreduct-daemon
$SUDO install -m 755 "$DIR/bin/memreduct-tray" /usr/local/bin/memreduct-tray
$SUDO ln -sf /usr/local/bin/memreduct-daemon /usr/bin/memreduct-daemon
$SUDO ln -sf /usr/local/bin/memreduct /usr/bin/memreduct
$SUDO ln -sf /usr/local/bin/memreduct-tray /usr/bin/memreduct-tray

echo "[2/4] Installing systemd service..."
$SUDO install -m 644 "$DIR/systemd/memreduct.service" /etc/systemd/system/memreduct.service
$SUDO systemctl daemon-reload
$SUDO systemctl enable --now memreduct.service

echo "[3/4] Installing Polkit policy..."
if [ -d /usr/share/polkit-1/actions ]; then
    $SUDO install -m 644 "$DIR/polkit/org.memreduct.policy" /usr/share/polkit-1/actions/org.memreduct.policy
fi

echo "[4/4] Installing Desktop launcher and icons..."
$SUDO mkdir -p /usr/share/icons/hicolor/scalable/apps
$SUDO install -m 644 "$DIR/assets/icon.svg" /usr/share/icons/hicolor/scalable/apps/memreduct.svg
$SUDO install -m 644 "$DIR/assets/memreduct.desktop" /usr/share/applications/memreduct.desktop

AUTOSTART_DIR="/etc/xdg/autostart"
if [ -d "$AUTOSTART_DIR" ]; then
    $SUDO cp "$DIR/assets/memreduct.desktop" "$AUTOSTART_DIR/memreduct-tray.desktop"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    $SUDO gtk-update-icon-cache -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi

echo ""
echo "Installation complete!"
echo "Run 'memreduct-tray &' or launch Mem Reduct from your Applications menu."
EOF
chmod +x "$TAR_DIR/install.sh"

cat << 'EOF' > "$TAR_DIR/uninstall.sh"
#!/usr/bin/env bash
set -e

SUDO=""
if [ "$EUID" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    else
        echo "Error: root privileges or sudo required." >&2
        exit 1
    fi
fi

echo "=== Uninstalling Mem Reduct Linux ==="
$SUDO systemctl stop memreduct.service 2>/dev/null || true
$SUDO systemctl disable memreduct.service 2>/dev/null || true
$SUDO rm -f /etc/systemd/system/memreduct.service
$SUDO systemctl daemon-reload

$SUDO rm -f /usr/local/bin/memreduct /usr/local/bin/memreduct-daemon /usr/local/bin/memreduct-tray
$SUDO rm -f /usr/bin/memreduct /usr/bin/memreduct-daemon /usr/bin/memreduct-tray
$SUDO rm -f /usr/share/applications/memreduct.desktop
$SUDO rm -f /usr/share/icons/hicolor/scalable/apps/memreduct.svg
$SUDO rm -f /usr/share/polkit-1/actions/org.memreduct.policy
$SUDO rm -f /etc/xdg/autostart/memreduct-tray.desktop

killall memreduct-tray 2>/dev/null || true
echo "Uninstallation complete."
EOF
chmod +x "$TAR_DIR/uninstall.sh"

tar -czf "$DIST_DIR/memreduct-v${VERSION}-linux-x86_64.tar.gz" -C "$DIST_DIR" "memreduct-v${VERSION}-linux-x86_64"
rm -rf "$TAR_DIR"

# 4. Checksums
echo "[4/4] Calculating SHA256 checksums..."
cd "$DIST_DIR"
sha256sum "memreduct_${VERSION}_${ARCH}.deb" "memreduct-v${VERSION}-linux-x86_64.tar.gz" > SHA256SUMS.txt

echo ""
echo "=== Packaging successfully completed! ==="
ls -lh "$DIST_DIR"
cat "$DIST_DIR/SHA256SUMS.txt"
