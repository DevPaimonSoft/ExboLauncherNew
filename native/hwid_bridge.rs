use std::{ffi::{c_void, CString}, io::Write};
#[link(name="kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(path: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}
#[repr(C)]
struct Bytes { bytes: Vec<u8> }
unsafe extern "system" fn new_array(_: *mut c_void, length: i32) -> *mut Bytes {
    if length != 40 { std::process::exit(3); }
    Box::into_raw(Box::new(Bytes { bytes: vec![0; length as usize] }))
}
unsafe extern "system" fn set_region(_: *mut c_void, array: *mut Bytes, start: i32, length: i32, data: *const u8) {
    if array.is_null() || start != 0 || length != 40 || data.is_null() { std::process::exit(4); }
    unsafe { (*array).bytes.copy_from_slice(std::slice::from_raw_parts(data, 40)); }
}
fn main() {
    let path = std::env::args_os().nth(1).expect("JMT path required");
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = path.encode_wide().chain(Some(0)).collect();
    unsafe {
        let module = LoadLibraryW(path.as_ptr()); if module.is_null() { std::process::exit(1); }
        let name = CString::new("_Java_ru_justagod_launcher_client_Native_getHWID@8").unwrap();
        let proc = GetProcAddress(module, name.as_ptr().cast()); if proc.is_null() { std::process::exit(2); }
        let mut vtable = [std::ptr::null::<c_void>(); 234];
        vtable[176] = new_array as *const c_void;
        vtable[208] = set_region as *const c_void;
        let mut env = vtable.as_ptr();
        let function: unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut Bytes = std::mem::transmute(proc);
        let bytes = function((&mut env as *mut *const *const c_void).cast(), std::ptr::null_mut());
        if bytes.is_null() { std::process::exit(5); }
        let bytes = Box::from_raw(bytes);
        std::io::stdout().write_all(&bytes.bytes).unwrap();
        FreeLibrary(module);
    }
}
