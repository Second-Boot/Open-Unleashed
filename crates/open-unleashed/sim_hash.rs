// The source hash of the simulation set and the physics-version file (**PORT-DEVIATION D56**, `RE/notes/lobby.md` section 14.1).
// Included by `build.rs` (which announces `sim=<version>` and `src=<hash>`) and by the master test in `src/brand.rs` (which fails when
// the hash differs from `sim_version.toml`). Needs `fnv` and `SIM_CRATES` / `SIM_SRC` (`sim_set.rs`) in scope. Paths are relative to
// the package directory, which is the working directory of both a build script and a test.

/// Every regular file under `dir`, sorted by path (a stable order for the hash).
fn files_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    es.sort();
    for p in es {
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The files of the simulation set, in hash order.
fn sim_files() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new("../..");
    let mut files = Vec::new();
    for c in SIM_CRATES {
        files_under(&root.join("crates").join(c).join("src"), &mut files);
    }
    let src = std::path::Path::new("src");
    for e in SIM_SRC {
        let p = src.join(e);
        if e.ends_with('/') { files_under(&p, &mut files) } else { files.push(p) }
    }
    files
}

/// FNV-1a 64 over the bytes (and relative paths) of every file of the simulation set and over the external packages of `Cargo.lock`
/// (the workspace's own entries carry the release version and are left out). Nothing is normalised: any byte change changes it.
fn sim_source_hash() -> String {
    let root = std::path::Path::new("../..");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &sim_files() {
        fnv(&mut h, f.to_string_lossy().replace('\\', "/").as_bytes());
        fnv(&mut h, &std::fs::read(f).unwrap_or_default());
    }
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap_or_default();
    for block in lock.split("[[package]]") {
        if block.contains("source = ") {
            fnv(&mut h, block.as_bytes());
        }
    }
    format!("{h:016x}")
}

/// `sim_version.toml`: the physics version number and the source hash it was last confirmed for.
#[derive(Debug, PartialEq, Eq)]
struct SimVersionFile {
    version: u32,
    source_hash: String,
}

fn parse_sim_version(text: &str) -> Option<SimVersionFile> {
    let (mut version, mut source_hash) = (None, None);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "version" => version = v.parse().ok(),
            "source_hash" => source_hash = Some(v.to_string()),
            _ => {}
        }
    }
    Some(SimVersionFile { version: version?, source_hash: source_hash? })
}

fn read_sim_version() -> SimVersionFile {
    let text = std::fs::read_to_string("sim_version.toml").expect("crates/open-unleashed/sim_version.toml is missing");
    parse_sim_version(&text).expect("sim_version.toml: needs `version = <integer>` and `source_hash = \"<16 hex>\"`")
}
