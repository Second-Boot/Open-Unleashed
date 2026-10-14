//! **PLATFORM GLUE** (packaging): a Windows build gets the application icon as a resource of the executable
//! (`res/app.rc`); cross-compiling, `embed-resource` compiles it with `llvm-rc`. Other targets embed nothing.

fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=../../packaging/brand/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("res/app.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
