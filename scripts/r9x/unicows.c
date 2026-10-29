/* Runtime part of the free unicows.lib replacement (see build-libs.sh).
 *
 * Every wrapped function's import slot (__imp__FooW@N) starts out pointing to
 * a small thunk. On the first call the thunk asks r9x_unicows_resolve for the
 * real address, stores it in the slot and jumps there. On Windows 9x/ME the
 * function comes from this file's own console wrappers or unicows.dll
 * (Microsoft Layer for Unicode); otherwise, and on NT-based Windows, from the
 * normal system DLL.
 *
 * Console input and output on Windows 9x use our own wrappers around the
 * native ANSI functions, converting with the console's code page:
 *   ReadConsoleInputW, PeekConsoleInputW, WriteConsoleW
 * and ToUnicodeEx, which Windows 95 lacks, is built on ToAscii; MoveFileExW,
 * which no Windows 9x has, on MoveFileA.
 *
 * Environment variables for testing:
 *   F_TERM_R9X_CONSOLE=1     use these wrappers on NT-based Windows too
 *   F_TERM_CONLOG=<file>     append every console input record read to <file> */

/* Declared here: the MSVC target build has no Windows headers. */
typedef void *HANDLE, *HMODULE;
typedef unsigned long DWORD;
typedef unsigned short WORD, WCHAR;
typedef int BOOL;
#define STD_ERROR_HANDLE ((DWORD)-12)
#define IMPORT __declspec(dllimport) __stdcall
IMPORT HANDLE GetStdHandle(DWORD);
IMPORT BOOL WriteFile(HANDLE, const void *, DWORD, DWORD *, void *);
IMPORT int lstrlenA(const char *);
IMPORT void ExitProcess(unsigned int);
IMPORT DWORD GetVersion(void);
IMPORT HMODULE LoadLibraryA(const char *);
IMPORT void *GetProcAddress(HMODULE, const char *);
IMPORT DWORD GetEnvironmentVariableA(const char *, char *, DWORD);
IMPORT HANDLE CreateFileA(const char *, DWORD, DWORD, void *, DWORD, DWORD, HANDLE);
IMPORT DWORD SetFilePointer(HANDLE, long, long *, DWORD);
IMPORT BOOL CloseHandle(HANDLE);
IMPORT BOOL ReadConsoleInputA(HANDLE, void *, DWORD, DWORD *);
IMPORT BOOL PeekConsoleInputA(HANDLE, void *, DWORD, DWORD *);
IMPORT BOOL WriteConsoleA(HANDLE, const void *, DWORD, DWORD *, void *);
IMPORT unsigned int GetConsoleCP(void);
IMPORT unsigned int GetConsoleOutputCP(void);
IMPORT int MultiByteToWideChar(unsigned int, DWORD, const char *, int, WCHAR *, int);
IMPORT int WideCharToMultiByte(unsigned int, DWORD, const WCHAR *, int, char *, int,
                               const char *, BOOL *);
__declspec(dllimport) int __cdecl wsprintfA(char *, const char *, ...);
IMPORT int ToAscii(unsigned int, unsigned int, const unsigned char *, WORD *, unsigned int);
IMPORT BOOL MoveFileA(const char *, const char *);
IMPORT BOOL DeleteFileA(const char *);
IMPORT BOOL CopyFileA(const char *, const char *, BOOL);

static void fail(const char *a, const char *b) {
    DWORD n;
    HANDLE h = GetStdHandle(STD_ERROR_HANDLE);
    WriteFile(h, a, lstrlenA(a), &n, 0);
    WriteFile(h, b, lstrlenA(b), &n, 0);
    WriteFile(h, "\r\n", 2, &n, 0);
    ExitProcess(127);
}

/* ---- optional input log ----------------------------------------------------- */

static void log_records(const char *what, BOOL ok, DWORD len, DWORD n, const unsigned char *recs) {
    static int state; /* 0 = not checked, 1 = off, 2 = on */
    static char path[260];
    if (state == 0) {
        DWORD r = GetEnvironmentVariableA("F_TERM_CONLOG", path, sizeof path);
        state = r > 0 && r < sizeof path ? 2 : 1;
    }
    if (state != 2)
        return;
    /* GENERIC_WRITE, share read/write, OPEN_ALWAYS */
    HANDLE f = CreateFileA(path, 0x40000000, 3, 0, 4, 0, 0);
    if (f == (HANDLE)-1)
        return;
    SetFilePointer(f, 0, 0, 2 /* FILE_END */);
    char line[200];
    DWORD w;
    int k = wsprintfA(line, "%s ok=%d len=%lu read=%lu\r\n", what, ok, len, n);
    WriteFile(f, line, k, &w, 0);
    for (DWORD i = 0; ok && i < n && i < 64; i++) {
        const unsigned char *r = recs + i * 20;
        k = wsprintfA(line, "  type=%u:", r[0] | r[1] << 8);
        for (int j = 0; j < 20; j++)
            k += wsprintfA(line + k, " %02x", r[j]);
        line[k++] = '\r';
        line[k++] = '\n';
        WriteFile(f, line, k, &w, 0);
    }
    CloseHandle(f);
}

/* ---- console wrappers for Windows 9x ------------------------------------------ */

/* INPUT_RECORD is 20 bytes: WORD EventType, 2 bytes padding, then the event.
 * For KEY_EVENT (1) the character (uChar) is at byte 14 of the record. */
static void input_to_unicode(unsigned char *recs, DWORD n) {
    unsigned int cp = GetConsoleCP();
    for (DWORD i = 0; i < n; i++) {
        unsigned char *r = recs + i * 20;
        if ((r[0] | r[1] << 8) != 1)
            continue;
        unsigned char c = r[14];
        WCHAR w = c;
        if (c >= 0x80 && MultiByteToWideChar(cp, 0, (const char *)&c, 1, &w, 1) != 1)
            w = '?';
        r[14] = (unsigned char)w;
        r[15] = (unsigned char)(w >> 8);
    }
}

static BOOL __stdcall my_ReadConsoleInputW(HANDLE h, void *buf, DWORD len, DWORD *read) {
    DWORD n = 0;
    BOOL ok = ReadConsoleInputA(h, buf, len, &n);
    log_records("ReadConsoleInput", ok, len, n, buf);
    if (ok)
        input_to_unicode(buf, n);
    if (read)
        *read = n;
    return ok;
}

static BOOL __stdcall my_PeekConsoleInputW(HANDLE h, void *buf, DWORD len, DWORD *read) {
    DWORD n = 0;
    BOOL ok = PeekConsoleInputA(h, buf, len, &n);
    log_records("PeekConsoleInput", ok, len, n, buf);
    if (ok)
        input_to_unicode(buf, n);
    if (read)
        *read = n;
    return ok;
}

/* Characters missing from the DOS code pages, replaced with close ones. */
static WCHAR console_fallback(WCHAR c) {
    switch (c) {
    case 0x256d: return 0x250c; /* ╭ -> ┌ */
    case 0x256e: return 0x2510; /* ╮ -> ┐ */
    case 0x256f: return 0x2518; /* ╯ -> ┘ */
    case 0x2570: return 0x2514; /* ╰ -> └ */
    case 0x2026: return '.';    /* … */
    case 0x2018: case 0x2019: return '\'';
    case 0x201c: case 0x201d: return '"';
    case 0x2013: case 0x2014: return '-';
    case 0x2022: return 0x00b7; /* • -> · */
    default: return c;
    }
}

static BOOL __stdcall my_WriteConsoleW(HANDLE h, const WCHAR *s, DWORD len, DWORD *written,
                                      void *reserved) {
    unsigned int cp = GetConsoleOutputCP();
    char out[1024];
    DWORD done = 0;
    (void)reserved;
    while (done < len) {
        DWORD chunk = len - done > 256 ? 256 : len - done;
        /* Don't split a surrogate pair between chunks. */
        if (chunk < len - done && s[done + chunk - 1] >= 0xd800 && s[done + chunk - 1] < 0xdc00)
            chunk--;
        WCHAR tmp[256];
        for (DWORD i = 0; i < chunk; i++)
            tmp[i] = console_fallback(s[done + i]);
        int k = WideCharToMultiByte(cp, 0, tmp, (int)chunk, out, sizeof out, "?", 0);
        if (k <= 0) {
            /* Unknown code page: fall back to plain ASCII. */
            k = 0;
            for (DWORD i = 0; i < chunk; i++)
                out[k++] = tmp[i] < 0x80 ? (char)tmp[i] : '?';
        }
        DWORD w;
        if (!WriteConsoleA(h, out, (DWORD)k, &w, 0)) {
            if (written)
                *written = done;
            return done > 0;
        }
        done += chunk;
    }
    if (written)
        *written = done;
    return 1;
}

/* ToUnicodeEx (Windows 98+/NT) via ToAscii (all versions). The layout handle
 * and flags are ignored: ToAscii uses the thread's layout. */
static int __stdcall my_ToUnicodeEx(unsigned int vk, unsigned int scan, const unsigned char *state,
                                    WCHAR *buf, int len, unsigned int flags, void *layout) {
    WORD out = 0;
    (void)flags;
    (void)layout;
    if (vk == 0 || vk > 0xff || !buf || len < 1)
        return 0;
    int r = ToAscii(vk, scan, state, &out, 0);
    if (r < 0)
        return -1; /* dead key */
    if (r == 0)
        return 0;
    char c = (char)(out & 0xff);
    WCHAR w = (unsigned char)c;
    if ((unsigned char)c >= 0x80 && MultiByteToWideChar(0 /* CP_ACP */, 0, &c, 1, &w, 1) != 1)
        return 0;
    buf[0] = w;
    return 1;
}

/* ---- files ---------------------------------------------------------------------- */

#define MOVEFILE_REPLACE_EXISTING 1
#define MOVEFILE_COPY_ALLOWED 2

/* Windows 9x has no MoveFileEx (unicows.dll's version just fails), which
 * std::fs::rename uses. Built here on DeleteFileA, MoveFileA and CopyFileA. */
static BOOL __stdcall my_MoveFileExW(const WCHAR *from, const WCHAR *to, DWORD flags) {
    char a[520], b[520];
    if (!WideCharToMultiByte(0 /* CP_ACP */, 0, from, -1, a, sizeof a, 0, 0) ||
        !to || !WideCharToMultiByte(0, 0, to, -1, b, sizeof b, 0, 0))
        return 0;
    if (flags & MOVEFILE_REPLACE_EXISTING)
        DeleteFileA(b); /* fails harmlessly if `to` doesn't exist */
    if (MoveFileA(a, b))
        return 1;
    if ((flags & MOVEFILE_COPY_ALLOWED) && CopyFileA(a, b, !(flags & MOVEFILE_REPLACE_EXISTING))) {
        DeleteFileA(a);
        return 1;
    }
    return 0;
}

/* Our replacements, used on Windows 9x (or anywhere with F_TERM_R9X_CONSOLE). */
static const struct {
    const char *name;
    void *fn;
} console_wrappers[] = {
    {"ReadConsoleInputW", (void *)my_ReadConsoleInputW},
    {"PeekConsoleInputW", (void *)my_PeekConsoleInputW},
    {"WriteConsoleW", (void *)my_WriteConsoleW},
    {"ToUnicodeEx", (void *)my_ToUnicodeEx},
    {"MoveFileExW", (void *)my_MoveFileExW},
};

static int str_eq(const char *a, const char *b) {
    while (*a && *a == *b)
        a++, b++;
    return *a == *b;
}

/* ---- resolver ------------------------------------------------------------------- */

/* `missing` is a stub that returns 0 with ERROR_CALL_NOT_IMPLEMENTED, used when
 * no DLL has the function (like the stubs Windows 9x has for most W functions). */
void *__cdecl r9x_unicows_resolve(const char *dll, const char *name, void **slot,
                                  void *missing) {
    static HMODULE unicows;
    void *p = 0;
    int win9x = (GetVersion() & 0x80000000u) != 0;
    char force[4];
    if (win9x || GetEnvironmentVariableA("F_TERM_R9X_CONSOLE", force, sizeof force) > 0) {
        for (unsigned i = 0; i < sizeof console_wrappers / sizeof console_wrappers[0]; i++)
            if (str_eq(name, console_wrappers[i].name))
                p = console_wrappers[i].fn;
    }
    if (!p && win9x) {
        if (!unicows) {
            unicows = LoadLibraryA("unicows.dll");
            if (!unicows)
                fail("f-term: unicows.dll (Microsoft Layer for Unicode) is missing. ",
                     "Put it next to f-term.exe.");
        }
        p = GetProcAddress(unicows, name);
    }
    if (!p) {
        HMODULE h = LoadLibraryA(dll);
        if (h)
            p = GetProcAddress(h, name);
    }
    if (!p)
        p = missing;
    *slot = p;
    return p;
}
