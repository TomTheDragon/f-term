#!/usr/bin/env bash
# Run the test suite for the Windows builds under Wine (portable Wine in .tools/wine).
#
#   scripts/test-windows.sh            # both architectures
#   scripts/test-windows.sh i686       # only 32-bit
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
WINE=${WINE:-$ROOT/.tools/wine/bin/wine}
MINGW=$ROOT/.tools/llvm-mingw
WIN7_TOOLCHAIN=${WIN7_TOOLCHAIN:-1.77.2}
[ -x "$WINE" ] || { echo "Wine not found at $WINE (see README)" >&2; exit 1; }
[ -x "$MINGW/bin/i686-w64-mingw32-clang" ] || { echo "llvm-mingw missing; run scripts/setup-tools.sh" >&2; exit 1; }

export WINEPREFIX=${WINEPREFIX:-$ROOT/.tools/wineprefix}
export WINEDEBUG=${WINEDEBUG:--all}
# Skip the Mono/Gecko installers (they need a display and would hang).
export WINEDLLOVERRIDES=${WINEDLLOVERRIDES:-mscoree,mshtml=}
export PATH="$MINGW/bin:$PATH"

ARCHES=("$@")
[ ${#ARCHES[@]} -eq 0 ] && ARCHES=(x86_64 i686)
for arch in "${ARCHES[@]}"; do
    triple=$arch-pc-windows-gnu
    up=$(echo "$triple" | tr 'a-z-' 'A-Z_')
    low=${triple//-/_}
    echo "=== tests on $triple (Wine)"
    env "CARGO_TARGET_${up}_LINKER=$arch-w64-mingw32-clang" \
        "CARGO_TARGET_${up}_RUNNER=$WINE" \
        "CC_${low}=$arch-w64-mingw32-clang" \
        "AR_${low}=llvm-ar" \
        RUSTFLAGS="-L native=$ROOT/.tools/mingw-gcc-compat/$arch" \
        cargo "+$WIN7_TOOLCHAIN" test --target "$triple" --target-dir target/mingw
done
