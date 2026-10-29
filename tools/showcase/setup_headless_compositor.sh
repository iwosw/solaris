#!/bin/sh
# Rootless install of sway + wlroots into a private prefix (no system change), used by
# client_render_showcase.py to present the AMD iGPU's frames into a headless compositor.
# On Ubuntu 26.04 these are the 9 packages `apt install sway` would add beyond a desktop install.
set -eu
PREFIX="${1:-$(dirname "$0")/../../.analysis/cage-prefix}"
mkdir -p "$PREFIX/debs" "$PREFIX/root"
cd "$PREFIX/debs"
apt-get download libliftoff0 libseat1 libxcb-composite0 libxcb-errors0 libxcb-ewmh2 libxcb-xinput0 \
    libwlroots-0.19 sway
for deb in ./*.deb; do dpkg -x "$deb" "$PREFIX/root"; done
LD_LIBRARY_PATH="$PREFIX/root/usr/lib/x86_64-linux-gnu" ldd "$PREFIX/root/usr/bin/sway" | { ! grep "not found"; }
echo "ok: $PREFIX/root/usr/bin/sway"
