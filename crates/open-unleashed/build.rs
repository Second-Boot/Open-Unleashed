//! **PLATFORM GLUE** (packaging): a Windows build gets the application icon as a resource of the executable
//! (`res/app.rc`); cross-compiling, `embed-resource` compiles it with `llvm-rc`. Other targets embed nothing.

/// FNV-1a, 64 bit: the fingerprint of a working tree that differs from its commit.
fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

fn git(args: &[&str]) -> Option<Vec<u8>> {
    let o = std::process::Command::new("git").args(["-C", "../.."]).args(args).output().ok()?;
    o.status.success().then_some(o.stdout)
}

include!("sim_set.rs");

include!("sim_hash.rs");

/// **PORT-DEVIATION D56** (`RE/notes/PORT-DEVIATIONS.md`): what the internet lobby link announces (`netplay/internet.rs`, `brand::build_id`):
/// the physics version number of `sim_version.toml` (`sim=`) and the informative hash of the simulation set's sources (`src=`, `sim_hash.rs`).
fn sim_env() {
    for f in sim_files() {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    for c in SIM_CRATES {
        println!("cargo:rerun-if-changed=../../crates/{c}/src");
    }
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=sim_version.toml");
    println!("cargo:rustc-env=OU_SIM_VERSION={}", read_sim_version().version);
    println!("cargo:rustc-env=OU_SIM_SOURCE_HASH={}", sim_source_hash());
}

/// The commit the build was made from, plus `-dirty.<8 hex>` when the working tree differs (display only); `nogit` outside a repository.
fn build_commit() -> String {
    let Some(head) = git(&["rev-parse", "--short=12", "HEAD"]) else { return "nogit".into() };
    let mut fp = String::from_utf8_lossy(&head).trim().to_string();
    for what in ["HEAD", "logs/HEAD"] {
        let Some(h) = git(&["rev-parse", "--git-path", what]) else { continue };
        println!("cargo:rerun-if-changed={}", std::path::Path::new("../..").join(String::from_utf8_lossy(&h).trim()).display());
    }
    println!("cargo:rerun-if-changed=../../crates");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut dirty = false;
    if let Some(d) = git(&["diff", "HEAD", "--", "crates", "tools", "Cargo.toml", "Cargo.lock"]) {
        if !d.is_empty() {
            dirty = true;
            fnv(&mut h, &d);
        }
    }
    if let Some(list) = git(&["ls-files", "--others", "--exclude-standard", "-z", "--", "crates", "tools", "Cargo.toml", "Cargo.lock"]) {
        for name in list.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            dirty = true;
            fnv(&mut h, name);
            if let Ok(c) = std::fs::read(std::path::Path::new("../..").join(String::from_utf8_lossy(name).as_ref())) {
                fnv(&mut h, &c);
            }
        }
    }
    if dirty {
        fp.push_str(&format!("-dirty.{:08x}", (h ^ (h >> 32)) as u32));
    }
    fp
}

fn main() {
    println!("cargo:rustc-env=OU_BUILD_COMMIT={}", build_commit());
    sim_env();
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=../../packaging/brand/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("res/app.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
