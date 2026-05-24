//! Build script: supply the Win32 import libs that the forced self-contained
//! link is missing, on the windows-gnu target only.
//!
//! `.cargo/config.toml` sets `-Clink-self-contained=yes` (required so rustc uses
//! its own linker + libgcc, see the note there). That self-contained set ships
//! only a subset of Win32 import libraries — enough for the `windows` crate, but
//! eframe/winit pull a few more (shlwapi, dwmapi, uxtheme, imm32, ...) that aren't
//! in it, so the link fails with `cannot find -lshlwapi`.
//!
//! We can't just add all of w64devkit/lib to the link path: its *runtime* libs
//! (msvcrt/mingwex/gcc) differ from rustc's bundled mingw and produce binaries
//! that crash at startup (STATUS_STACK_OVERFLOW). So copy ONLY the safe,
//! ABI-stable import STUBS we're missing into OUT_DIR and add that to the search
//! path. This affects the final binary's link, not other crates' build scripts.

use std::path::PathBuf;
use std::process::Command;

/// Win32 import libs eframe/winit may need that rustc's self-contained set lacks.
/// All are thin DLL import stubs (no runtime code), safe alongside the
/// self-contained runtime. Only those that actually exist in w64devkit are copied.
const MISSING_IMPORT_LIBS: &[&str] = &[
    "libshlwapi.a",
    "libdwmapi.a",
    "libuxtheme.a",
    "libimm32.a",
    "libversion.a",
    "libcfgmgr32.a",
    "libpropsys.a",
    "libhid.a",
    "libpowrprof.a",
    "libdxgi.a",
    "libd3d12.a",
    "libdwrite.a",
];

fn main() {
    // Only the GNU toolchain forces the self-contained link; MSVC/others are fine.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        return;
    }
    let Some(mingw_lib) = mingw_lib_dir() else {
        println!(
            "cargo:warning=mingw lib dir not found; if the link fails on -lshlwapi, \
             put w64devkit\\bin on PATH"
        );
        return;
    };

    let stub_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("mingw-import-stubs");
    let _ = std::fs::create_dir_all(&stub_dir);
    for lib in MISSING_IMPORT_LIBS {
        let src = mingw_lib.join(lib);
        if src.is_file() {
            let _ = std::fs::copy(&src, stub_dir.join(lib));
        }
    }
    println!("cargo:rustc-link-search=native={}", stub_dir.display());
}

/// Locate w64devkit's lib dir via the mingw gcc on PATH (`-print-file-name`
/// returns the full path when the lib is found, else just the bare name).
fn mingw_lib_dir() -> Option<PathBuf> {
    let out = Command::new("x86_64-w64-mingw32-gcc")
        .arg("-print-file-name=libshlwapi.a")
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    if path.is_file() {
        Some(path.parent()?.to_path_buf())
    } else {
        None
    }
}
