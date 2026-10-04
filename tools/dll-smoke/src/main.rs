//! `dll-smoke <path to patch_meta_ai.dll>`: loads the DLL the way the game does (LoadLibrary +
//! GetProcAddress of the two stable entry symbols) and plays one session against it. The DLL
//! finds its own folder, so settings.ini, diag.log and meta_table.txt appear next to it.

#[path = "../../../tests/common/scenario.rs"]
mod scenario;

#[cfg(windows)]
fn main() {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    let arg = std::env::args().nth(1).expect("usage: dll-smoke <path to patch_meta_ai.dll>");
    let dll = std::path::absolute(&arg).expect("bad path");
    let dir = dll.parent().expect("no folder").to_path_buf();
    let wide: Vec<u16> = dll.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let module = LoadLibraryW(wide.as_ptr());
        assert!(!module.is_null(), "LoadLibraryW failed for {}", dll.display());
        let required = GetProcAddress(module, b"tfm2_mod_required_abi_level\0".as_ptr());
        let entry = GetProcAddress(module, b"tfm2_mod_entry_stable\0".as_ptr());
        assert!(!required.is_null() && !entry.is_null(), "entry symbols missing");
        let required: scenario::RequiredFn = std::mem::transmute(required);
        let entry: scenario::EntryFn = std::mem::transmute(entry);
        scenario::run(required, entry, &dir);
    }
    println!("dll-smoke: OK ({})", dll.display());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("dll-smoke loads a Windows DLL: build it for x86_64-pc-windows-gnu and run it on Windows or Wine");
}
