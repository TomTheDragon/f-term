//! Implementation for Windows.
//!
//! F-Term patch: the RNG functions are looked up at runtime instead of
//! being linked statically, so the program still starts on systems without
//! bcrypt.dll (Windows 9x/ME/NT4/2000/XP). Order of preference:
//! BCryptGenRandom (Vista+), RtlGenRandom (XP+), CryptGenRandom (95 OSR2+/NT4).
use crate::Error;
use core::{ffi::c_void, mem::MaybeUninit, ptr};
use core::sync::atomic::{AtomicUsize, Ordering};

const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x00000002;
const PROV_RSA_FULL: u32 = 1;
const CRYPT_VERIFYCONTEXT: u32 = 0xF000_0000;

type BCryptGenRandomFn = unsafe extern "system" fn(*mut c_void, *mut u8, u32, u32) -> i32;
type RtlGenRandomFn = unsafe extern "system" fn(*mut c_void, u32) -> u8;
type CryptAcquireContextAFn = unsafe extern "system" fn(*mut usize, *const u8, *const u8, u32, u32) -> i32;
type CryptGenRandomFn = unsafe extern "system" fn(usize, u32, *mut u8) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

unsafe fn lookup(dll: &[u8], func: &[u8]) -> usize {
    let module = LoadLibraryA(dll.as_ptr());
    if module.is_null() {
        return 0;
    }
    GetProcAddress(module, func.as_ptr()) as usize
}

// 0 = not looked up yet, 1 = unavailable, otherwise the function pointer.
static BCRYPT: AtomicUsize = AtomicUsize::new(0);
static RTL: AtomicUsize = AtomicUsize::new(0);
static CRYPT_PROV: AtomicUsize = AtomicUsize::new(0);
static CRYPT_GEN: AtomicUsize = AtomicUsize::new(0);

fn cached(slot: &AtomicUsize, dll: &[u8], func: &[u8]) -> Option<usize> {
    let mut v = slot.load(Ordering::Relaxed);
    if v == 0 {
        v = unsafe { lookup(dll, func) };
        if v == 0 {
            v = 1;
        }
        slot.store(v, Ordering::Relaxed);
    }
    if v == 1 {
        None
    } else {
        Some(v)
    }
}

fn crypt_gen_random(chunk: &mut [MaybeUninit<u8>]) -> bool {
    unsafe {
        let gen = match cached(&CRYPT_GEN, b"advapi32.dll\0", b"CryptGenRandom\0") {
            Some(f) => core::mem::transmute::<usize, CryptGenRandomFn>(f),
            None => return false,
        };
        let mut prov = CRYPT_PROV.load(Ordering::Relaxed);
        if prov == 0 {
            let acquire = match lookup(b"advapi32.dll\0", b"CryptAcquireContextA\0") {
                0 => return false,
                f => core::mem::transmute::<usize, CryptAcquireContextAFn>(f),
            };
            let mut h = 0usize;
            if acquire(&mut h, ptr::null(), ptr::null(), PROV_RSA_FULL, CRYPT_VERIFYCONTEXT) == 0 {
                return false;
            }
            prov = h;
            CRYPT_PROV.store(prov, Ordering::Relaxed);
        }
        gen(prov, chunk.len() as u32, chunk.as_mut_ptr() as *mut u8) != 0
    }
}

pub fn getrandom_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    // Prevent overflow of u32
    for chunk in dest.chunks_mut(u32::MAX as usize) {
        if let Some(f) = cached(&BCRYPT, b"bcrypt.dll\0", b"BCryptGenRandom\0") {
            let f: BCryptGenRandomFn = unsafe { core::mem::transmute(f) };
            let ret = unsafe {
                f(ptr::null_mut(), chunk.as_mut_ptr() as *mut u8, chunk.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
            };
            if (ret as u32) >> 30 != 0b11 {
                continue;
            }
        }
        #[cfg(not(target_vendor = "uwp"))]
        {
            if let Some(f) = cached(&RTL, b"advapi32.dll\0", b"SystemFunction036\0") {
                let f: RtlGenRandomFn = unsafe { core::mem::transmute(f) };
                if unsafe { f(chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32) } != 0 {
                    continue;
                }
            }
            if crypt_gen_random(chunk) {
                continue;
            }
        }
        // SAFETY: non-zero constant.
        return Err(Error::WINDOWS_RTL_GEN_RANDOM);
    }
    Ok(())
}
