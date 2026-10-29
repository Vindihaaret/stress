//! Minimal dlopen wrapper for the vendor libraries Stress only uses when they
//! exist (libEGL, NVML), so the program has no hard link-time dependency on them.

use std::ffi::{c_void, CString};

pub struct Lib(*mut c_void);

// The handle is never closed and dlsym is thread-safe.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

impl Lib {
    pub fn open(name: &str) -> Option<Lib> {
        let name = CString::new(name).ok()?;
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        (!handle.is_null()).then_some(Lib(handle))
    }

    pub fn raw(&self, name: &str) -> Option<*const c_void> {
        let name = CString::new(name).ok()?;
        let ptr = unsafe { libc::dlsym(self.0, name.as_ptr()) };
        (!ptr.is_null()).then_some(ptr as *const c_void)
    }

    /// Looks up a function.
    ///
    /// # Safety
    /// `T` must be the exact `extern "C"` function pointer type of the symbol.
    pub unsafe fn sym<T: Copy>(&self, name: &str) -> Option<T> {
        self.raw(name).map(|ptr| unsafe { cast(ptr) })
    }
}

/// Turns a symbol address into a function pointer.
///
/// # Safety
/// `T` must be a function pointer type matching what `ptr` points to.
pub unsafe fn cast<T: Copy>(ptr: *const c_void) -> T {
    assert_eq!(size_of::<T>(), size_of::<*const c_void>());
    unsafe { std::mem::transmute_copy(&ptr) }
}
