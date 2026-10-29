/* Minimal C runtime for the rust9x (i586-rust9x-windows-msvc) build, replacing
 * Microsoft's libcmt.lib so the Windows 9x exe can be linked without Visual C++.
 *
 * Rust's std needs very little from the CRT: an entry point, TLS support, a few
 * compiler helpers (__chkstk, 64-bit division) and some string/math functions.
 * The string and basic math functions are imported from msvcrt.dll (see
 * msvcrt.def, which lists only exports that every Win9x msvcrt.dll has); the
 * rest is implemented here.
 *
 * Built with: clang --target=i586-pc-windows-msvc -O2 -ffreestanding -fno-builtin */
#include <stdint.h>

#pragma comment(lib, "r9x_msvcrt")
#pragma comment(lib, "kernel32")

typedef void(__cdecl *init_fn)(void);
__declspec(dllimport) void __stdcall ExitProcess(unsigned int code);

/* ---- floating point marker ------------------------------------------------ */
int _fltused = 0x9875;

/* ---- thread-local storage (static TLS in the exe works on Win9x) ---------- */
typedef void(__stdcall *tls_callback)(void *, unsigned long, void *);
unsigned long _tls_index = 0;
__attribute__((section(".tls"))) char _tls_start = 0;
__attribute__((section(".tls$ZZZ"))) char _tls_end = 0;
__attribute__((section(".CRT$XLA"))) tls_callback __xl_a = 0;
__attribute__((section(".CRT$XLZ"))) tls_callback __xl_z = 0;

struct tls_directory {
    uintptr_t start, end, index, callbacks;
    uint32_t zero_fill, characteristics;
};
__attribute__((section(".rdata$T"), used)) const struct tls_directory _tls_used = {
    (uintptr_t)&_tls_start, (uintptr_t)&_tls_end, (uintptr_t)&_tls_index,
    (uintptr_t)(&__xl_a + 1), 0, 0};

/* ---- static initializers and entry point --------------------------------- */
__attribute__((section(".CRT$XIA"))) init_fn __xi_a = 0;
__attribute__((section(".CRT$XIZ"))) init_fn __xi_z = 0;
__attribute__((section(".CRT$XCA"))) init_fn __xc_a = 0;
__attribute__((section(".CRT$XCZ"))) init_fn __xc_z = 0;

static void run_initializers(init_fn *p, init_fn *end) {
    for (; p < end; p++)
        if (*p)
            (*p)();
}

int __cdecl main(int argc, char **argv);

void __cdecl mainCRTStartup(void) {
    run_initializers(&__xi_a + 1, &__xi_z);
    run_initializers(&__xc_a + 1, &__xc_z);
    /* Rust's std reads the command line with GetCommandLineW itself. */
    ExitProcess((unsigned int)main(0, 0));
}

/* ---- stack probe (MSVC semantics: also moves esp down by eax bytes) ------- */
__asm__(".text\n"
        ".globl __chkstk\n"
        ".globl __alloca_probe\n"
        "__chkstk:\n"
        "__alloca_probe:\n"
        "  pushl %ecx\n"
        "  leal 8(%esp), %ecx\n" /* caller's esp */
        "  cmpl $0x1000, %eax\n"
        "  jb 1f\n"
        "2:\n"
        "  subl $0x1000, %ecx\n"
        "  testl %ecx, (%ecx)\n" /* touch each page in order */
        "  subl $0x1000, %eax\n"
        "  cmpl $0x1000, %eax\n"
        "  ja 2b\n"
        "1:\n"
        "  subl %eax, %ecx\n"
        "  testl %ecx, (%ecx)\n"
        "  leal 4(%esp), %eax\n" /* address of the return address */
        "  movl %ecx, %esp\n"
        "  movl -4(%eax), %ecx\n" /* restore ecx */
        "  pushl (%eax)\n"        /* return address on the new stack top */
        "  ret\n");

/* ---- 64-bit division helpers (stdcall, result in edx:eax) ---------------- */
/* Only 32-bit operations, shifts and compares, so clang does not turn this
 * back into calls to the helpers themselves. */
static uint64_t udivmod64(uint64_t n, uint64_t d, uint64_t *rem) {
    if (d == 0)
        __builtin_trap();
    uint64_t q = 0, r = 0;
    if ((uint32_t)(d >> 32) == 0 && (uint32_t)(n >> 32) == 0) {
        uint32_t a = (uint32_t)n, b = (uint32_t)d;
        *rem = a % b;
        return a / b;
    }
    for (int i = 63; i >= 0; i--) {
        r = (r << 1) | ((n >> i) & 1);
        if (r >= d) {
            r -= d;
            q |= (uint64_t)1 << i;
        }
    }
    *rem = r;
    return q;
}

__attribute__((stdcall)) uint64_t aulldiv(uint64_t a, uint64_t b) __asm__("__aulldiv");
__attribute__((stdcall)) uint64_t aulldiv(uint64_t a, uint64_t b) {
    uint64_t r;
    return udivmod64(a, b, &r);
}

__attribute__((stdcall)) uint64_t aullrem(uint64_t a, uint64_t b) __asm__("__aullrem");
__attribute__((stdcall)) uint64_t aullrem(uint64_t a, uint64_t b) {
    uint64_t r;
    udivmod64(a, b, &r);
    return r;
}

__attribute__((stdcall)) int64_t alldiv(int64_t a, int64_t b) __asm__("__alldiv");
__attribute__((stdcall)) int64_t alldiv(int64_t a, int64_t b) {
    uint64_t r;
    uint64_t ua = a < 0 ? 0 - (uint64_t)a : (uint64_t)a;
    uint64_t ub = b < 0 ? 0 - (uint64_t)b : (uint64_t)b;
    uint64_t q = udivmod64(ua, ub, &r);
    return (a < 0) != (b < 0) ? (int64_t)(0 - q) : (int64_t)q;
}

__attribute__((stdcall)) int64_t allrem(int64_t a, int64_t b) __asm__("__allrem");
__attribute__((stdcall)) int64_t allrem(int64_t a, int64_t b) {
    uint64_t r;
    uint64_t ua = a < 0 ? 0 - (uint64_t)a : (uint64_t)a;
    uint64_t ub = b < 0 ? 0 - (uint64_t)b : (uint64_t)b;
    udivmod64(ua, ub, &r);
    return a < 0 ? (int64_t)(0 - r) : (int64_t)r;
}

/* ---- C99 math missing from old msvcrt.dll -------------------------------- */
double __cdecl floor(double);
double __cdecl ceil(double);

double __cdecl trunc(double x) { return x < 0 ? ceil(x) : floor(x); }
float __cdecl truncf(float x) { return (float)trunc(x); }

double __cdecl round(double x) {
    if (x < 0)
        return -round(-x);
    double r = floor(x);
    return x - r >= 0.5 ? r + 1.0 : r; /* x - r is exact */
}
float __cdecl roundf(float x) { return (float)round(x); }

/* ---- C++ exception support referenced by std's unwind tables -------------- */
/* The build uses panic=abort, so these are never really used. The handler
 * just lets other handlers run, like having no handler at all. */
int __cdecl __CxxFrameHandler3(void *rec, void *frame, void *ctx, void *dc) {
    (void)rec;
    (void)frame;
    (void)ctx;
    (void)dc;
    return 1; /* ExceptionContinueSearch */
}

/* Panics that try to unwind (only in test builds, which ignore panic=abort)
 * end the process instead. */
__declspec(dllimport) void *__stdcall GetStdHandle(unsigned long);
__declspec(dllimport) int __stdcall WriteFile(void *, const void *, unsigned long,
                                              unsigned long *, void *);
__attribute__((stdcall, noreturn)) void CxxThrowException(void *obj, void *info)
    __asm__("__CxxThrowException@8");
__attribute__((stdcall, noreturn)) void CxxThrowException(void *obj, void *info) {
    static const char msg[] = "fatal: panic unwinding is not supported in this build\r\n";
    unsigned long n;
    (void)obj;
    (void)info;
    WriteFile(GetStdHandle((unsigned long)-12), msg, sizeof msg - 1, &n, 0);
    ExitProcess(101);
    __builtin_unreachable();
}

/* const type_info::`vftable' */
__attribute__((used)) void __cdecl r9x_type_info_dtor(void) {}
__asm__(".section .rdata,\"dr\"\n"
        ".globl \"??_7type_info@@6B@\"\n"
        "\"??_7type_info@@6B@\":\n"
        "  .long _r9x_type_info_dtor\n"
        ".text\n");
