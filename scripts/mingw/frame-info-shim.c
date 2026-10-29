/* DWARF unwind-table registration for i686-pc-windows-gnu with llvm-mingw.
 *
 * Rust's startup object (rsbegin.o) wants to register .eh_frame with libgcc's
 * __register_frame_info, from a .ctors entry. With llvm-mingw this breaks in
 * two ways:
 *   - lld leaves rsbegin's .ctors entry outside __CTOR_LIST__, so it never runs;
 *   - rsend.o's zero terminator lands in the middle of .eh_frame, because lld
 *     places archive members (std, dependencies, libunwind) after it. Unwinders
 *     stop at the first terminator, so most frames have no unwind info.
 * Either way, every panic aborts with "failed to initiate panic".
 *
 * So the libgcc entry points are no-ops, and instead a CRT initializer
 * (.CRT$XCU, run by the mingw-w64 CRT) finds this module's .eh_frame and
 * registers every chunk of it with LLVM libunwind. */
#include <stdint.h>
#include <string.h>
#include <windows.h>

void __unw_add_dynamic_eh_frame_section(uintptr_t eh_frame_start);

void __register_frame_info(const void *begin, void *object) {
    (void)begin;
    (void)object;
}

void *__deregister_frame_info(const void *begin) {
    (void)begin;
    return 0;
}

static void register_eh_frame(void) {
    HMODULE mod;
    if (!GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            (LPCSTR)&register_eh_frame, &mod))
        return;

    /* PE section names are cut to 8 bytes: ".eh_frame" becomes ".eh_fram". */
    const uint8_t *base = (const uint8_t *)mod;
    const IMAGE_NT_HEADERS *nt =
        (const IMAGE_NT_HEADERS *)(base + ((const IMAGE_DOS_HEADER *)base)->e_lfanew);
    const IMAGE_SECTION_HEADER *sec = IMAGE_FIRST_SECTION(nt);
    const uint8_t *p = 0, *end = 0;
    for (unsigned i = 0; i < nt->FileHeader.NumberOfSections; i++, sec++) {
        if (memcmp(sec->Name, ".eh_fram", 8) == 0) {
            p = base + sec->VirtualAddress;
            end = p + sec->Misc.VirtualSize;
            break;
        }
    }
    if (!p)
        return;

    /* Register each run of CIEs/FDEs between zero terminators. */
    int chunk_start = 1;
    while (p + 4 <= end) {
        uint32_t len = *(const uint32_t *)p;
        if (len == 0) {
            p += 4;
            chunk_start = 1;
            continue;
        }
        if (len == 0xffffffffu) /* 64-bit lengths never occur on i686 */
            break;
        if (chunk_start)
            __unw_add_dynamic_eh_frame_section((uintptr_t)p);
        chunk_start = 0;
        p += 4 + len;
    }
}

__attribute__((section(".CRT$XCU"), used))
static void (*const register_eh_frame_init)(void) = register_eh_frame;
