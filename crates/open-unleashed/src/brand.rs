//! The project's own user-facing name (window title, error boxes) and icon. A working name: the packaging scripts
//! read the same one from `packaging/brand.sh`. It names no trademark of the original game's owners;
//! descriptions say which game it runs ("for Need for Speed: Porsche Unleashed (2000)").

/// The window title and the caption of the start-up error box.
pub const TITLE: &str = "OpenUnleashed";

/// The build identity the internet lobby link announces once after sign-on (**PORT-DEVIATION D56**): `<name> <version> <commit> sim=<n> src=<hash>`.
/// `commit` (`<12 hex>`, `-dirty.<hash>` for uncommitted changes, `nogit` outside a repository) and `src` (the hash of the simulation set's
/// sources, informative) are for the admin's eyes; `sim` is the physics version, the integer of `sim_version.toml` (master and the enhanced
/// branch share it). The server compares only `sim`.
pub fn build_id() -> String {
    format!(
        "{} {} {} sim={} src={}",
        TITLE,
        env!("CARGO_PKG_VERSION"),
        env!("OU_BUILD_COMMIT"),
        env!("OU_SIM_VERSION"),
        env!("OU_SIM_SOURCE_HASH")
    )
}

/// **PLATFORM GLUE** (not in the original): the window's icon — the title bar and the task switcher on Windows, the
/// window list on Linux / X11 (macOS takes the app bundle's icon and ignores it). The picture is
/// `packaging/brand/icon.svg`, rendered by `packaging/brand/render_icons.py`. `None` if it fails to decode.
pub fn window_icon() -> Option<winit::window::Icon> {
    let rgba = image::load_from_memory(include_bytes!("../../../packaging/brand/icon-256.png")).ok()?.into_rgba8();
    let (w, h) = rgba.dimensions();
    winit::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes() {
        assert!(super::window_icon().is_some());
    }
}

#[cfg(test)]
mod sim_set_tests {
    include!("../sim_set.rs");

    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }

    fn listed(list: &[&str], file: &str) -> bool {
        list.iter().any(|e| if let Some(d) = e.strip_suffix('/') { file.starts_with(&format!("{d}/")) } else { *e == file })
    }

    /// A new source file of the application must be classified: in the simulation set (hashed into the lobby fingerprint) or in the list of
    /// files that cannot change a race. Forgetting one fails here instead of silently leaving physics out of the fingerprint.
    #[test]
    fn every_source_file_is_classified_as_simulation_or_not() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&base, &base, &mut files);
        for f in &files {
            assert!(listed(SIM_SRC, f) != listed(NOT_SIM, f), "{f}: put it in exactly one of SIM_SRC / NOT_SIM (sim_set.rs)");
        }
        for e in SIM_SRC.iter().chain(NOT_SIM) {
            assert!(base.join(e.trim_end_matches('/')).exists(), "{e} is listed but missing");
        }
        for c in SIM_CRATES {
            assert!(base.join("../../").join(c).join("src").is_dir(), "{c}");
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod sim_version_tests {
    fn fnv(h: &mut u64, bytes: &[u8]) {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    include!("../sim_set.rs");
    include!("../sim_hash.rs");

    /// The announced string: `sim=` is the integer, the parser of the server finds it by word.
    #[test]
    fn build_id_announces_the_physics_number_and_the_source_hash() {
        let id = super::build_id();
        println!("0xC7 text: {id}");
        let sim = read_sim_version();
        let words: Vec<&str> = id.split(' ').collect();
        assert_eq!(words.len(), 5, "{id}");
        assert_eq!(words[3], format!("sim={}", sim.version));
        assert_eq!(words[4], format!("src={}", sim_source_hash()));
        assert_eq!(nfs_net::lobby::parse_client_build(&nfs_net::lobby::client_build(&id)).unwrap().sim(), Some(sim.version.to_string().as_str()));
    }

    #[test]
    fn sim_version_file_parses() {
        assert_eq!(
            parse_sim_version("# c\nversion = 7\nsource_hash = \"00ab\"\n"),
            Some(SimVersionFile { version: 7, source_hash: "00ab".into() })
        );
        assert_eq!(parse_sim_version("version = x\nsource_hash = \"a\""), None);
    }

    /// The simulation set's sources must match `sim_version.toml`; see the header of that file.
    ///
    /// **TEST GLUE** (tooling): `NFS_UPDATE_SIM_VERSION=1` rewrites the hash in `sim_version.toml`; `=bump` also increments the number.
    #[test]
    fn sim_version_file_matches_the_simulation_sources() {
        let have = sim_source_hash();
        let file = read_sim_version();
        if file.source_hash == have {
            return;
        }
        if let Ok(mode) = std::env::var("NFS_UPDATE_SIM_VERSION") {
            let version = if mode == "bump" { file.version + 1 } else { file.version };
            let text = std::fs::read_to_string("sim_version.toml").unwrap();
            let text: String = text
                .lines()
                .map(|l| match l.split_once('=').map(|(k, _)| k.trim()) {
                    Some("version") => format!("version = {version}\n"),
                    Some("source_hash") => format!("source_hash = \"{have}\"\n"),
                    _ => format!("{l}\n"),
                })
                .collect();
            std::fs::write("sim_version.toml", text).unwrap();
            eprintln!("sim_version.toml: version {version}, source_hash {have}");
            return;
        }
        panic!(
            "the simulation set changed (hash {have}, sim_version.toml has {} for physics version {}).\n\
             * physics behaviour may have changed: NFS_UPDATE_SIM_VERSION=bump cargo test -p open-unleashed sim_version_file  (number + 1, new hash)\n\
             * provably non-behavioural (comments, refactor): NFS_UPDATE_SIM_VERSION=1 cargo test -p open-unleashed sim_version_file  (hash only), allowed ONLY \
             when the three race goldens AND net_loop_det.sh are identical; when in doubt, bump.\n\
             Commit sim_version.toml together with the change.",
            file.source_hash, file.version
        );
    }
}
