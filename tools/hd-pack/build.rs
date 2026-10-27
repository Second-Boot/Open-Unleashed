// PLATFORM GLUE: intel_tex_2's precompiled kernels hold C++ objects (ispc_texcomp_astc.cpp) that need the C++ runtime, and
// the crate does not ask for it; macOS and Windows link theirs anyway, Linux needs libstdc++ named (else rust-lld stops
// on an undefined __gxx_personality_v0).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}
