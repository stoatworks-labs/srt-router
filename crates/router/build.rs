//! `omt-io`'s own build.rs can only rpath *its own* test binaries — Cargo's
//! unsuffixed `cargo:rustc-link-arg` never propagates to a dependent
//! package's binary, and the suffixed `-bins` variant turned out to require
//! the *emitting* package itself to have a bin target (it errors otherwise),
//! which a lib-only crate like `omt-io` never does. So the final binary
//! crate — this one — is what has to embed the rpath for its own `[[bin]]`
//! target. Only relevant when the `omt` feature is on and `OMT_LIB_DIR` is
//! set; harmless no-op otherwise.

fn main() {
    if std::env::var_os("CARGO_FEATURE_OMT").is_some() {
        if let Ok(lib_dir) = std::env::var("OMT_LIB_DIR") {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{lib_dir}");
        }
    }
    println!("cargo:rerun-if-env-changed=OMT_LIB_DIR");
}
