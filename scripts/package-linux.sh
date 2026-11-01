#!/usr/bin/env bash
# Linux packages for x86-64 into ./dist:
#
#   f-term_<version>-1_amd64.deb       Ubuntu, Debian and others
#   f-term-<version>-1.x86_64.rpm      Fedora, openSUSE and others
#   F-Term-<version>-x86_64.AppImage   any distribution, nothing to install
#
# The .deb and .rpm contain one binary for terminal and window that uses the
# system's DejaVu Sans Mono font and CA certificates (installed as package
# dependencies) instead of built-in copies. The AppImage is self-contained.
# All link against glibc 2.17 or newer and load X11/Wayland at run time.
#
# Run `f-term` in a terminal to use it there; the application menu entry
# opens a window (`f-term --window`).
#
# Requires scripts/setup-tools.sh to have been run once.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
export PATH="$ROOT/.tools/bin:$PATH"
DIST=$ROOT/dist
STAGE=$ROOT/target/linux-pkg
TARGET=x86_64-unknown-linux-gnu
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
APPIMAGETOOL=$ROOT/.tools/appimage/appimagetool-x86_64.AppImage
RUNTIME=$ROOT/.tools/appimage/runtime-x86_64
mkdir -p "$DIST" "$STAGE"

for t in cargo-zigbuild cargo-deb cargo-generate-rpm; do
    command -v "$t" >/dev/null || { echo "Missing $t — run scripts/setup-tools.sh first." >&2; exit 1; }
done
[ -x "$APPIMAGETOOL" ] && [ -f "$RUNTIME" ] || { echo "Missing appimagetool — run scripts/setup-tools.sh first." >&2; exit 1; }

# build <name> <features>: release binary into $STAGE/<name>/f-term
build() {
    echo "=== f-term ($1): $2"
    cargo zigbuild --release --locked --target "$TARGET.2.17" --features "$2" --target-dir "$ROOT/target/linux-pkg-$1"
    mkdir -p "$STAGE/$1"
    cp "$ROOT/target/linux-pkg-$1/$TARGET/release/f-term" "$STAGE/$1/f-term"
}
build system gui,prefer-terminal,system-fonts,system-certs
build appimage gui,prefer-terminal

echo "=== .deb"
deb=$DIST/f-term_${VERSION}-1_amd64.deb
cargo deb --no-build --no-strip --target "$TARGET" --output "$deb"

echo "=== .rpm"
rpm=$DIST/f-term-${VERSION}-1.x86_64.rpm
cargo generate-rpm --arch x86_64 --output "$rpm"

echo "=== AppImage"
app=$STAGE/AppDir
rm -rf "$app"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/metainfo" "$app/usr/share/icons/hicolor/scalable/apps"
cp "$STAGE/appimage/f-term" "$app/usr/bin/f-term"
ID=io.github.tomthedragon.f-term
cp "packaging/linux/$ID.desktop" "$app/$ID.desktop"
cp "packaging/linux/$ID.desktop" "$app/usr/share/applications/"
cp "packaging/linux/$ID.metainfo.xml" "$app/usr/share/metainfo/$ID.appdata.xml"
cp assets/f-term.svg "$app/f-term.svg"
cp assets/f-term.svg "$app/usr/share/icons/hicolor/scalable/apps/"
if command -v rsvg-convert >/dev/null; then
    rsvg-convert -w 256 -h 256 assets/f-term.svg -o "$app/.DirIcon"
else
    cp assets/f-term.svg "$app/.DirIcon"
fi
cat > "$app/AppRun" <<'EOF'
#!/bin/sh
# Started from the desktop: a window. Started in a terminal: runs there
# (--window opens a window anyway).
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/f-term" "$@"
EOF
chmod +x "$app/AppRun"
appimage=$DIST/F-Term-${VERSION}-x86_64.AppImage
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --runtime-file "$RUNTIME" "$app" "$appimage"

echo
ls -la "$deb" "$rpm" "$appimage"
