#!/usr/bin/env bash
# Build release binaries for all platforms into ./dist.
#
#   scripts/build-all.sh            # all targets
#   scripts/build-all.sh linux-i586 macos-arm64   # selected targets
#
# Targets:
#   linux-x86_64     x86_64-unknown-linux-musl   static, any x86-64 CPU
#   linux-i586       i586-unknown-linux-musl     static, Pentium or newer (no SSE needed)
#   macos-arm64      aarch64-apple-darwin        macOS 11+
#   macos-x86_64     x86_64-apple-darwin         macOS 10.12+, Core 2 or newer
#   windows-x86_64   x86_64-pc-windows-gnu       Windows 7+ (Rust 1.77.2, llvm-mingw, msvcrt.dll)
#   windows-i686     i686-pc-windows-gnu         Windows 7+ 32-bit (Rust 1.77.2, llvm-mingw, msvcrt.dll)
#   windows-9x       i586-rust9x-windows-msvc    Windows 95/98/ME/NT4/2000/XP (rust9x, see README)
#   *-gui            the same in an own window instead of the terminal (feature `gui`);
#                    Linux GUI builds link against glibc 2.17+ (x86_64/i586-unknown-linux-gnu)
#
# Requires scripts/setup-tools.sh to have been run once.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
export PATH="$ROOT/.tools/bin:$PATH"
DIST=$ROOT/dist
WIN7_TOOLCHAIN=${WIN7_TOOLCHAIN:-1.77.2}
mkdir -p "$DIST"

ALL=(linux-x86_64 linux-i586 macos-arm64 macos-x86_64 windows-x86_64 windows-i686 windows-9x
     linux-x86_64-gui linux-i586-gui macos-arm64-gui macos-x86_64-gui windows-x86_64-gui windows-i686-gui windows-9x-gui)
TARGETS=("$@")
[ ${#TARGETS[@]} -eq 0 ] && TARGETS=("${ALL[@]}")

need() {
    command -v "$1" >/dev/null 2>&1 || { echo "Missing $1 — run scripts/setup-tools.sh first." >&2; exit 1; }
}
need zig
need cargo-zigbuild

PURE_RUST_TLS=(--no-default-features --features tls-rustcrypto,images)
ok=()
failed=()

# put <file> <dest>: copy into dist. Copy and rename, so a binary that is
# currently running there can still be replaced.
put() {
    cp "$1" "$2.new" && mv -f "$2.new" "$2"
}

# zigbuild <name> <triple> <output-ext> [extra cargo args...]
zigbuild() {
    local name=$1 triple=$2 ext=$3
    shift 3
    echo
    echo "=== $name ($triple)"
    if cargo zigbuild --release --target "$triple" "$@"; then
        # A glibc version suffix (x86_64-unknown-linux-gnu.2.17) isn't part of the folder name.
        put "target/${triple%.[0-9]*.[0-9]*}/release/f-term$ext" "$DIST/f-term-$name$ext"
        ok+=("$name")
    else
        failed+=("$name")
    fi
}

# Windows builds: link with llvm-mingw (msvcrt.dll, runs on plain Windows 7).
# Without it, fall back to zig, which links the Universal C Runtime (UCRT).
MINGW=$ROOT/.tools/llvm-mingw
# winbuild <name> <arch> [extra rustflags] [extra cargo args...]
winbuild() {
    local name=$1 arch=$2 extra=${3:-}
    shift 2
    [ $# -gt 0 ] && shift
    local triple=$arch-pc-windows-gnu
    if [ ! -x "$MINGW/bin/$arch-w64-mingw32-clang" ]; then
        echo
        echo "!!! llvm-mingw not found: $name will need the UCRT on Windows 7 (run scripts/setup-tools.sh)"
        RUSTUP_TOOLCHAIN=$WIN7_TOOLCHAIN RUSTFLAGS="$extra" zigbuild "$name" "$triple" ".exe" "$@"
        return
    fi
    echo
    echo "=== $name ($triple, llvm-mingw)"
    local up
    up=$(echo "$triple" | tr 'a-z-' 'A-Z_')
    local low=${triple//-/_}
    if env PATH="$MINGW/bin:$PATH" \
        "CARGO_TARGET_${up}_LINKER=$arch-w64-mingw32-clang" \
        "CC_${low}=$arch-w64-mingw32-clang" \
        "AR_${low}=llvm-ar" \
        RUSTFLAGS="-L native=$ROOT/.tools/mingw-gcc-compat/$arch $extra" \
        cargo "+$WIN7_TOOLCHAIN" build --release --target "$triple" --target-dir target/mingw "$@"; then
        put "target/mingw/$triple/release/f-term.exe" "$DIST/f-term-$name.exe"
        ok+=("$name")
    else
        failed+=("$name")
    fi
}

# r9xbuild <name> <subsystem> <extra rustflags> [extra cargo args...]: Windows 9x build with rust9x.
r9xbuild() {
    local t=$1 subsystem=$2 extra=$3 libpaths flags missing
    shift 3
    echo
    echo "=== $t (i586-rust9x-windows-msvc)"
    if ! rustup toolchain list | grep -q '^rust9x'; then
        echo "skipped: rust9x toolchain not installed"
        failed+=("$t(no rust9x)")
        return
    fi
    libpaths=${R9X_LIBPATHS:-}
    if [ -z "$libpaths" ]; then
        # No Microsoft libraries given: use the free replacements.
        if ! scripts/r9x/build-libs.sh >/dev/null; then
            failed+=("$t(r9x libs)")
            return
        fi
        libpaths=$ROOT/.tools/r9x-libs
    fi
    flags="-C target-feature=+crt-static -C link-arg=/SUBSYSTEM:$subsystem -C link-arg=/OSVERSION:4.0 -C link-arg=/STACK:1048576"
    IFS=';:' read -r -a libdirs <<<"$libpaths"
    for d in "${libdirs[@]}"; do
        [ -n "$d" ] && flags="$flags -C link-arg=/LIBPATH:$d"
    done
    flags="$flags $extra ${R9X_EXTRA_RUSTFLAGS:-}"
    # Cargo doesn't notice changed link libraries; force a relink.
    find target/i586-rust9x-windows-msvc/release \( -name 'f-term.exe' -o -name 'f_term*.exe' \) -delete 2>/dev/null || true
    if CARGO_TARGET_I586_RUST9X_WINDOWS_MSVC_LINKER=rust-lld \
       CARGO_PROFILE_RELEASE_PANIC=abort \
       RUSTFLAGS="$flags" \
       cargo +rust9x build --release --target i586-rust9x-windows-msvc "${PURE_RUST_TLS[@]}" "$@"; then
        put target/i586-rust9x-windows-msvc/release/f-term.exe "$DIST/f-term-$t.exe"
        ok+=("$t")
        # Unicode (W) functions not routed through unicows break on Win9x.
        if [ -z "${R9X_LIBPATHS:-}" ]; then
            missing=$("$MINGW/bin/llvm-readobj" --coff-imports "$DIST/f-term-$t.exe" |
                awk '/Symbol:/{print $2}' | grep -E '^[A-Za-z0-9_]+W$' |
                grep -vxF -f <(awk '!/^#/{print $2}' scripts/r9x/unicows.txt) |
                grep -vxE 'MultiByteToWideChar|WideCharToMultiByte|ExtTextOutW' || true)
            [ -n "$missing" ] && echo "warning: add to scripts/r9x/unicows.txt for Win9x:" $missing
        fi
    else
        failed+=("$t")
    fi
    return 0
}

for t in "${TARGETS[@]}"; do
    case "$t" in
        linux-x86_64)
            zigbuild "$t" x86_64-unknown-linux-musl "" ;;
        linux-i586)
            # ring needs SSE2, so use the pure-Rust crypto backend.
            zigbuild "$t" i586-unknown-linux-musl "" "${PURE_RUST_TLS[@]}" ;;
        macos-arm64)
            MACOSX_DEPLOYMENT_TARGET=11.0 zigbuild "$t" aarch64-apple-darwin "" ;;
        macos-x86_64)
            MACOSX_DEPLOYMENT_TARGET=10.12 RUSTFLAGS="-C target-cpu=core2" zigbuild "$t" x86_64-apple-darwin "" ;;
        windows-x86_64)
            winbuild "$t" x86_64 ;;
        windows-i686)
            winbuild "$t" i686 ;;
        # GUI versions: an own window instead of the terminal (feature `gui`).
        linux-x86_64-gui)
            # winit loads the X11/Wayland libraries at run time, so link against
            # glibc (2.17 or newer) instead of static musl.
            zigbuild "$t" x86_64-unknown-linux-gnu.2.17 "" --features gui ;;
        linux-i586-gui)
            zigbuild "$t" i586-unknown-linux-gnu.2.17 "" "${PURE_RUST_TLS[@]}" --features gui ;;
        macos-arm64-gui | macos-x86_64-gui)
            # A window needs AppKit, which only Apple's macOS SDK provides.
            if [ ! -d "${SDKROOT:-}/System/Library/Frameworks/AppKit.framework" ]; then
                echo
                echo "=== $t: skipped, set SDKROOT to a MacOSX.sdk folder (or build on a Mac)"
                failed+=("$t(no SDKROOT)")
            elif [ "$t" = macos-arm64-gui ]; then
                MACOSX_DEPLOYMENT_TARGET=11.0 zigbuild "$t" aarch64-apple-darwin "" --features gui
            else
                MACOSX_DEPLOYMENT_TARGET=10.12 RUSTFLAGS="-C target-cpu=core2" zigbuild "$t" x86_64-apple-darwin "" --features gui
            fi ;;
        windows-x86_64-gui)
            winbuild "$t" x86_64 "-C link-arg=-Wl,--subsystem,windows" --features gui ;;
        windows-i686-gui)
            winbuild "$t" i686 "-C link-arg=-Wl,--subsystem,windows" --features gui ;;
        windows-9x)
            r9xbuild "$t" CONSOLE,4.0 "" --features narrow-console ;;
        windows-9x-gui)
            # A GUI program: no console window; our CRT starts at mainCRTStartup.
            r9xbuild "$t" WINDOWS,4.0 "-C link-arg=/ENTRY:mainCRTStartup" --features gui ;;
        *)
            echo "Unknown target $t (known: ${ALL[*]})" >&2
            exit 2 ;;
    esac
done

echo
echo "Built:  ${ok[*]:-none}"
[ ${#failed[@]} -gt 0 ] && echo "Failed/skipped: ${failed[*]}"
ls -la "$DIST"
