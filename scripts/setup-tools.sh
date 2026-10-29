#!/usr/bin/env bash
# Installs the cross-compilation helpers into ./.tools (nothing outside the project
# is touched except rustup toolchains/targets):
#   - zig (from the "ziglang" Python wheel)  -> linker for Linux, macOS and Windows
#   - cargo-zigbuild                          -> drives cargo with zig as linker
#   - rustup targets for every platform
#   - Rust 1.77.2 (last release that supports Windows 7)
#   - llvm-mingw (msvcrt variant)             -> Windows linker without the UCRT dependency
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
TOOLS=$ROOT/.tools
mkdir -p "$TOOLS/bin"

if [ ! -x "$TOOLS/bin/zig" ]; then
    echo ">> Downloading zig"
    tmp=$(mktemp -d)
    python3 -m pip download ziglang --no-deps --only-binary=:all: -d "$tmp" -q
    python3 -I -m zipfile -e "$tmp"/ziglang-*.whl "$TOOLS/zigwheel"
    chmod +x "$TOOLS/zigwheel/ziglang/zig"
    ln -sf ../zigwheel/ziglang/zig "$TOOLS/bin/zig"
    rm -rf "$tmp"
fi
"$TOOLS/bin/zig" version

if [ ! -x "$TOOLS/bin/cargo-zigbuild" ]; then
    echo ">> Installing cargo-zigbuild"
    cargo install --root "$TOOLS" cargo-zigbuild
fi

LLVM_MINGW_VERSION=${LLVM_MINGW_VERSION:-20261020}
if [ ! -x "$TOOLS/llvm-mingw/bin/i686-w64-mingw32-clang" ]; then
    echo ">> Downloading llvm-mingw $LLVM_MINGW_VERSION (msvcrt)"
    name="llvm-mingw-$LLVM_MINGW_VERSION-msvcrt-ubuntu-22.04-x86_64"
    curl -fsSL -o "$TOOLS/llvm-mingw.tar.xz" "https://github.com/mstorsjo/llvm-mingw/releases/download/$LLVM_MINGW_VERSION/$name.tar.xz"
    tar -xf "$TOOLS/llvm-mingw.tar.xz" -C "$TOOLS"
    rm -f "$TOOLS/llvm-mingw.tar.xz"
    rm -rf "$TOOLS/llvm-mingw"
    mv "$TOOLS/$name" "$TOOLS/llvm-mingw"
fi

# Rust's windows-gnu targets link against GCC's libgcc/libgcc_eh. llvm-mingw has the
# LLVM equivalents (compiler-rt and libunwind); provide them under the GCC names.
echo ">> Preparing libgcc compatibility libraries"
M=$TOOLS/llvm-mingw
rt=$(ls -d "$M"/lib/clang/*/lib/windows | head -n1)
for pair in i686:i386 x86_64:x86_64; do
    arch=${pair%%:*}
    d=$TOOLS/mingw-gcc-compat/$arch
    mkdir -p "$d"
    cp "$M/$arch-w64-mingw32/lib/libunwind.a" "$d/libgcc_eh.a"
    cp "$rt/libclang_rt.builtins-${pair#*:}.a" "$d/libgcc.a"
done
"$M/bin/i686-w64-mingw32-clang" -O2 -c scripts/mingw/frame-info-shim.c -o "$TOOLS/mingw-gcc-compat/i686/shim.o"
"$M/bin/llvm-ar" rs "$TOOLS/mingw-gcc-compat/i686/libgcc.a" "$TOOLS/mingw-gcc-compat/i686/shim.o"

echo ">> Installing rustup targets"
rustup target add x86_64-unknown-linux-musl i586-unknown-linux-musl aarch64-apple-darwin x86_64-apple-darwin \
    x86_64-unknown-linux-gnu i586-unknown-linux-gnu
rustup toolchain install 1.77.2 --profile minimal
rustup target add --toolchain 1.77.2 i686-pc-windows-gnu x86_64-pc-windows-gnu

if rustup toolchain list | grep -q '^rust9x'; then
    echo ">> rust9x toolchain found (Windows 9x builds possible, see README)"
else
    echo ">> rust9x toolchain not installed; Windows 9x builds will be skipped (see README)"
fi
echo "Done."
