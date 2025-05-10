#![allow(unused_imports)] // Allow unused imports for now, libc types might not be directly used if signature is simple

use hooktracer_macro::hook_trace;

use libc::{c_char, size_t, ssize_t}; // Standard C types

// This is the function that will contain our hook logic.
// The #[hook_trace] attribute will generate the actual `readlink` symbol.
//
// The signature of the original readlink is:
// ssize_t readlink(const char *pathname, char *buf, size_t bufsiz);
//
// Our logic function's signature must be:
// fn name(original_fn_ptr: OriginalFnType, pathname: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t;
// where OriginalFnType is `unsafe extern "C" fn(*const c_char, *mut c_char, size_t) -> ssize_t`

#[hook_trace(symbol = "readlink")]
fn my_readlink_hook(
    original_readlink: unsafe extern "C" fn(pathname: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t,
    pathname: *const c_char,
    buf: *mut c_char,
    bufsiz: size_t
) -> ssize_t {
    eprintln!("[hooktrace] readlink called for path: {:?}, bufsiz: {}", unsafe { std::ffi::CStr::from_ptr(pathname) }, bufsiz);
    // 暂时不调用 _original_readlink_ptr
    // 返回一个错误码或者一个模拟的成功值
    if bufsiz > 0 && !buf.is_null() {
         unsafe { *buf = 0; } // 写入一个空终止符
    }
    return -1; // 或者 0 如果模拟成功
    // let result = unsafe { original_readlink(pathname, buf, bufsiz) };
    // result
}

// To make this example runnable, you would typically compile this into a .so/.dylib file.
// Then, you'd use LD_PRELOAD (Linux) or DYLD_INSERT_LIBRARIES (macOS) to load this
// library when running an application that calls readlink.
//
// For example, on Linux:
// 1. cargo build --manifest-path=example/read_link/Cargo.toml
// 2. LD_PRELOAD=../../target/debug/libreadlinkspy.so ls -l /path/to/a/symlink
//
// On macOS:
// 1. cargo build --manifest-path=example/read_link/Cargo.toml
// 2. DYLD_INSERT_LIBRARIES=../../target/debug/libreadlinkspy.dylib DYLD_FORCE_FLAT_NAMESPACE=1 ls -l /path/to/a/symlink
//    (DYLD_FORCE_FLAT_NAMESPACE=1 might be needed for dlsym(RTLD_NEXT, ...) to work correctly for system symbols)
