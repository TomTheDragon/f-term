# Vendored crates

## getrandom-0.2.17

Unmodified except `src/windows.rs`: the upstream version links `BCryptGenRandom`
from `bcrypt.dll` statically, which only exists on Windows Vista and later, so
the program would not even start on Windows 9x/ME/2000/XP. The patched version
looks the functions up at runtime and falls back to `RtlGenRandom` and then
`CryptGenRandom`. Licensed MIT/Apache-2.0 like upstream.

## crossterm_winapi-0.9.1

Unmodified except `src/structs/input.rs` (and a deprecated `std::u32::MAX` in
`src/console.rs`): upstream panics on console input
records with an unknown event type, and on resize events when the window size
can't be read. The Windows 9x console produces such records, which crashed the
program right after start. The patched version ignores them. Licensed MIT like
upstream.
