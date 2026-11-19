//! The switches' storage and start-up choice: mode, `NFS_ENH` / GPU class, `enhancements.json`, the A/B key, the layouts dir.
use super::*;

/// How the switches were chosen at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// the player's own (`enhancements.json`, or an `NFS_ENH` list / `all`)
    Normal,
    /// `NFS_ENH=off`: the original, with none of the front-end additions (see the module docs)
    HardOriginal,
}

static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();

/// Pin the mode for the run (the game's start-up, once). Until then — a library user, the engine's
/// own tests — the front-end additions are absent, i.e. the original.
pub fn init(mode: Mode) {
    let _ = MODE.set(mode);
}

/// Whether the front-end additions (the entry button, the screen, the strings) exist in this run.
pub fn enabled() -> bool {
    MODE.get() == Some(&Mode::Normal)
}

/// The switches at start and how they were chosen: `NFS_ENH` over the stored file over the default
/// (every entry at its default level).
pub fn load() -> (Enhancements, Mode) {
    let env = std::env::var("NFS_ENH").ok();
    let file = read_file();
    let r = load_with(env.as_deref(), file.as_deref());
    // ENH-LITE: neither a list in the environment nor a stored file = the first run (or a player who never
    // touched the panel): the set is still the full defaults here, the GPU class is not known yet; the
    // renderer's start-up decides ([`auto_pending`], `app::session`). Nothing is written.
    let env_chosen = env.as_deref().is_some_and(|v| !v.trim().is_empty());
    if !env_chosen && file.is_none() {
        AUTO_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    r
}

static AUTO_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// ENH-LITE: whether this run's set is the "no choice made yet" default, to be matched to the GPU
/// (a weak one gets the light preset, `Enhancements::lite`) once the adapter is known; taken once.
pub fn auto_pending() -> bool {
    AUTO_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// ENH-LITE: the set for a first run on an adapter of this class — the full defaults, or the light
/// preset for a weak GPU. `NFS_GPU_CLASS=weak|strong` forces the class (test glue: the Mac is strong).
pub fn auto_set(class: nfs_renderer::GpuClass) -> (Enhancements, nfs_renderer::GpuClass) {
    let class = match std::env::var("NFS_GPU_CLASS").ok().as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("weak") | Some("light") | Some("lite") => nfs_renderer::GpuClass::Weak,
        Some("strong") | Some("full") => nfs_renderer::GpuClass::Strong,
        _ => class,
    };
    (if class == nfs_renderer::GpuClass::Weak { Enhancements::lite() } else { Enhancements::all() }, class)
}

pub(crate) fn load_with(env: Option<&str>, file: Option<&str>) -> (Enhancements, Mode) {
    if let Some(v) = env {
        let mut unknown = Vec::new();
        match Enhancements::parse_override(v, &mut unknown) {
            Some(e) => {
                for u in &unknown {
                    tracing::warn!(token = %u, "NFS_ENH: no such enhancement");
                }
                let hard = v.trim().eq_ignore_ascii_case("off") || v.trim().eq_ignore_ascii_case("none");
                return (e, if hard { Mode::HardOriginal } else { Mode::Normal });
            }
            None => tracing::warn!(value = %v, "NFS_ENH: empty value ignored"),
        }
    }
    (file.map(parse_file).unwrap_or_else(Enhancements::all), Mode::Normal)
}

/// `enhancements.json` — in `NFS5_SAVE_DIR` when that is set (a scripted run owns its directory
/// and must not read the player's file), else the port's data dir
/// (`nfs_assets::platform::data_dir`). Never inside the `.sav`.
pub fn settings_path() -> PathBuf {
    let dir = crate::config::save_root();
    dir.join("enhancements.json")
}

fn read_file() -> Option<String> {
    std::fs::read_to_string(settings_path()).ok()
}

/// The set a stored file gives; unreadable → the default (every entry at its default level). The
/// file is `{"version": 2, "levels": {"E1": 2, …}}`; the first format `{"version": 1, "switches":
/// {"E1": true, …}}` reads too (`true` = the entry's default level, `false` = off), and so does a
/// boolean in `levels`. A token the file does not name is at its default.
pub fn parse_file(text: &str) -> Enhancements {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Enhancements::all() };
    let Some(map) = v.get("levels").or_else(|| v.get("switches")).and_then(|s| s.as_object()) else { return Enhancements::all() };
    let stored = |v: &serde_json::Value| match v {
        serde_json::Value::Bool(b) => Some(EnhancementStored::Bool(*b)),
        serde_json::Value::Number(n) => n.as_u64().map(|l| EnhancementStored::Level(l.min(u8::MAX as u64) as u8)),
        _ => None,
    };
    // ENHANCEMENT E34 (temporal levels, file version 3): the old spatial levels 1..3 (quality / balanced / performance) are now 5..7 - the
    // temporal ones took 1..4 - so a file written before keeps its meaning
    let version = v.get("version").and_then(|n| n.as_u64()).unwrap_or(1);
    let mut set = Enhancements::from_stored_in(
        ENHANCEMENT_REGISTRY,
        map.iter().filter_map(|(k, v)| {
            let s = stored(v)?;
            Some((k.as_str(), match s {
                EnhancementStored::Level(l @ 1..=3) if k == "E34" && version < 3 => EnhancementStored::Level(l + 4),
                // ENHANCEMENT ENH-COAT (file version 5): E8's levels are the lacquer's now (1 new, 2 normal, 3 worn); the old ones were the
                // faces' size - 1 low (128 px) and 2 high (256 px). Low becomes normal, high becomes new (a sharper coat, the old high's
                // detail)
                EnhancementStored::Level(1) if k == "E8" && version < 5 => EnhancementStored::Level(2),
                EnhancementStored::Level(2) if k == "E8" && version < 5 => EnhancementStored::Level(1),
                // ENHANCEMENT E41 (file version 7): the textures are off / on now; every older level above 0 (x2, x4, the first «новые») is on
                EnhancementStored::Level(l @ 2..) if k == "E41" && version < 7 => EnhancementStored::Level(l.min(1)),
                s => s,
            }))
        }),
    );
    // ENHANCEMENT ENH-UPSCALER (file version 4): the upscaler choice E38 and FSR's sharpness E39 are new. A file of an older version has
    // E34 (MetalFX) or E37 (FSR) above 0 = that backend was on: the choice becomes it, with the stored level kept as the backend's
    // mode / scale and E39 at its default (E37 always sharpened by 0.3); with neither on the choice stays off.
    if version < 4 {
        set.infer_upscaler(false);
    }
    set
}

/// The stored form: `{"version": 7, "levels": {"E1": 2, …}}` (3: E34's levels renumbered; 4: the upscaler choice E38, see `parse_file`;
/// 5: E8's levels are the clear coat's; 6: E41's levels were original / x2 / x4; 7: E41 is off / on).
pub fn file_text(e: &Enhancements) -> String {
    let levels: serde_json::Map<String, serde_json::Value> = e.pairs_in(ENHANCEMENT_REGISTRY).into_iter().map(|(k, l)| (k.to_string(), l.into())).collect();
    let v = serde_json::json!({ "version": 7, "levels": levels });
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// Save on change. Best effort: a read-only data dir loses the setting, not the game.
pub fn save(e: &Enhancements) {
    let path = settings_path();
    if let Err(err) = save_to(&path, e) {
        tracing::warn!(path = %path.display(), error = %err, "enhancements.json not saved");
    }
}

/// [`save`] at a given path (the directory is created).
pub fn save_to(path: &std::path::Path, e: &Enhancements) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, file_text(e))
}

/// The A/B key (`0`, the window handler's `App::enh_key`): the player's set <-> every switch off (or, with the player's own set already all
/// off, every default). Returns the set to show and the player's set to bring back by the next press (`None`
/// = this press is the way back). **The A/B is a view, never stored**: before ENH-SAVE the key saved whatever
/// it showed, so two presses (off, then "all") overwrote the player's levels with the registry defaults —
/// «everything is reset» (2026-10-04).
pub fn ab_toggle(current: Enhancements, stash: Option<Enhancements>) -> (Enhancements, Option<Enhancements>) {
    match stash {
        Some(player) => (player, None),
        None => (if current.any_on() { Enhancements::ORIGINAL } else { Enhancements::all() }, Some(current)),
    }
}

/// Where the port-own layouts live: `$NFS5_REMASTER_DIR`, `enhanced/layouts` in the game root
/// (the packaged data, `packaging/stage_data.sh`), beside the executable / in a macOS bundle's Resources,
/// then — a development build — the repository's own `enhanced/layouts`. `remaster/` (the folder's
/// earlier name) is accepted after `enhanced/` at every place.
pub fn layouts_dir(paths: &GamePaths) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("NFS5_REMASTER_DIR") {
        c.push(PathBuf::from(d).join("layouts"));
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()));
    for folder in nfs_assets::hdpack::ENHANCED_FOLDERS {
        c.push(paths.root.join(folder).join("layouts"));
        if let Some(d) = &exe_dir {
            c.push(d.join(folder).join("layouts"));
            c.push(d.join("../Resources").join(folder).join("layouts"));
        }
        c.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(folder).join("layouts"));
    }
    c.into_iter().find(|d| d.is_dir())
}

