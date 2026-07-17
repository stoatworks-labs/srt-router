//! Links against the real OMT SDK's `libomt` dynamic library. Unlike
//! `ndi-io`, there's no bindgen step here — the C API is small enough that
//! `src/sys.rs` hand-transcribes it directly from `libomt.h` (see that
//! file's header comment for the exact SDK version/source).
//!
//! Set `OMT_LIB_DIR` to the directory containing `libomt.dylib`/`.so`/`.dll`
//! (the `Libraries/<platform>` folder from a
//! https://github.com/openmediatransport/libomtnet release zip). No
//! standard install location is assumed — OMT doesn't ship a system
//! installer/package the way the NDI SDK does.

use std::path::PathBuf;

fn main() {
    let lib_dir = std::env::var("OMT_LIB_DIR").unwrap_or_else(|_| {
        panic!(
            "OMT_LIB_DIR is not set. Point it at the directory containing libomt's \
             dynamic library, extracted from a libomtnet release zip's Libraries/<platform> \
             folder: https://github.com/openmediatransport/libomtnet/releases"
        )
    });
    let lib_dir = PathBuf::from(lib_dir);

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=omt");
    // libomt.dylib doesn't reference libvmx.dylib (the VMX video codec) as
    // a load-time dependency per `otool -L` — it's loaded dynamically at
    // its own runtime when actually needed, not something this crate calls
    // directly, so no `-lvmx` here. The rpath above still covers it in
    // case libomt looks for it relative to itself.
    // Embed the lib dir as an rpath so the built binary finds libomt at
    // runtime without needing DYLD_LIBRARY_PATH/LD_LIBRARY_PATH set — a
    // real gotcha hit earlier with ndi-io, where only `cargo run` (not the
    // built binary run directly) could locate libndi.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    println!("cargo:rerun-if-env-changed=OMT_LIB_DIR");
}
