#!/usr/bin/env bash
# Build free replacements for the Microsoft libraries the rust9x (Windows 9x)
# build links against, into .tools/r9x-libs:
#
#   libcmt.lib       minimal C runtime (crt.c) + msvcrt.dll imports (msvcrt.def)
#   unicows.lib      Microsoft Layer for Unicode loader (unicows.c, unicows.txt)
#   <name>.lib       system import libraries, taken from llvm-mingw
#
# Used by build-all.sh when R9X_LIBPATHS is not set.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
M=$ROOT/.tools/llvm-mingw
OUT=${1:-$ROOT/.tools/r9x-libs}
SRC=$ROOT/scripts/r9x
[ -x "$M/bin/clang" ] || { echo "llvm-mingw missing; run scripts/setup-tools.sh" >&2; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT/obj"
CC=("$M/bin/clang" --target=i586-pc-windows-msvc -march=pentium -mno-sse -O2 -ffreestanding -Wall -Wno-unused-command-line-argument)
CFLAGS_NOBUILTIN=-fno-builtin

# System import libraries under their MSVC names.
for f in "$M"/i686-w64-mingw32/lib/lib*.a; do
    n=$(basename "$f" .a)
    # Relative, so the links survive moving the project folder.
    ln -sr "$f" "$OUT/${n#lib}.lib"
done

# C runtime.
"$M/bin/llvm-dlltool" -m i386 -d "$SRC/msvcrt.def" -l "$OUT/r9x_msvcrt.lib"
"${CC[@]}" $CFLAGS_NOBUILTIN -c "$SRC/crt.c" -o "$OUT/obj/crt.obj"
"$M/bin/llvm-lib" /out:"$OUT/libcmt.lib" "$OUT/obj/crt.obj"

# unicows: one import slot + thunk per listed function.
asm=$OUT/obj/unicows-thunks.s
{
    # Mark the object SafeSEH-compatible (it has no exception handlers).
    echo '.globl "@feat.00"'
    echo '"@feat.00" = 1'
    declare -A seen_dll
    while read -r dll fn; do
        [[ -z "$dll" || "$dll" == \#* ]] && continue
        sym=$("$M/bin/llvm-nm" "$M/i686-w64-mingw32/lib/lib$dll.a" 2>/dev/null |
            grep -oE "__imp__${fn}@[0-9]+\$" | head -n1 || true)
        if [ -z "$sym" ]; then
            echo "warning: $dll!$fn not found in llvm-mingw import libraries, skipped" >&2
            continue
        fi
        direct=${sym#__imp_}
        if [ -z "${seen_dll[$dll]:-}" ]; then
            seen_dll[$dll]=1
            echo ".section .rdata,\"dr\""
            echo "dll_$dll: .asciz \"$dll.dll\""
        fi
        cat <<EOF
.section .rdata,"dr"
name_$fn: .asciz "$fn"
.data
.globl "$sym"
.p2align 2
"$sym": .long thunk_$fn
.text
thunk_$fn:
    pushl \$missing_$fn
    pushl \$"$sym"
    pushl \$name_$fn
    pushl \$dll_$dll
    call _r9x_unicows_resolve
    addl \$16, %esp
    jmp *%eax
# Used when no DLL has the function: fail like Win9x's own stubs.
missing_$fn:
    pushl \$120
    call *__imp__SetLastError@4
    xorl %eax, %eax
    ret \$${sym##*@}
.globl "$direct"
"$direct":
    jmp *"$sym"
EOF
    done <"$SRC/unicows.txt"
} >"$asm"
"${CC[@]}" -c "$asm" -o "$OUT/obj/unicows-thunks.obj"
"${CC[@]}" -c "$SRC/unicows.c" -o "$OUT/obj/unicows.obj"
"$M/bin/llvm-lib" /out:"$OUT/unicows.lib" "$OUT/obj/unicows-thunks.obj" "$OUT/obj/unicows.obj"
echo "Win9x libraries ready in $OUT"
