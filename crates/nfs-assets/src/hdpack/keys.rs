//! The keys of the HD pack: `<class>/<scope>/<name>`, every component made filesystem-safe (see the module docs of [`super`]).

/// A lower-case, filesystem-safe path component: `[a-z0-9_-]` kept, every other byte `~XX`.
pub fn safe(name: &str) -> String {
    let t = name.trim().to_lowercase();
    if t.is_empty() {
        return "~".to_string();
    }
    let mut out = String::with_capacity(t.len());
    for b in t.bytes() {
        if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("~{b:02x}"));
        }
    }
    out
}

/// `track/<track>/<entry>`
pub fn key_track(track: &str, entry: &str) -> String {
    format!("track/{}/{}", safe(track), safe(entry))
}
/// `sky/<track>/<entry>`: an entry of `Track/Sky/<track>.fsh` (the horizon mountains, the cloud layers)
pub fn key_sky(track: &str, entry: &str) -> String {
    format!("sky/{}/{}", safe(track), safe(entry))
}
/// `car/<model>/f<file>/<entry>`, `file` the `.tpg` file number the embedded pack belongs to
pub fn key_car(model: &str, file: usize, entry: &str) -> String {
    format!("car/{}/f{file}/{}", safe(model), safe(entry))
}
/// `carglobal/<file>/<entry>`, `file` the global pack's name without extension
pub fn key_car_global(file: &str, entry: &str) -> String {
    format!("carglobal/{}/{}", safe(file), safe(entry))
}
/// `car/<model>/damage_<d|l>/<index>_<entry>`: the decal packs repeat their entry names (several `0000`), so the
/// entry's 3-digit index in the pack is part of the key
pub fn key_car_damage(model: &str, light: bool, index: usize, entry: &str) -> String {
    format!("car/{}/damage_{}/{index:03}_{}", safe(model), if light { 'l' } else { 'd' }, safe(entry))
}

/// The manifest's `class` words (`asset-extractor hd-export`): what kind of texture a row is.
pub mod class {
    /// a texture of the 3D world (`Track/<track>.fsh`)
    pub const TRACK: &str = "track";
    /// an entry of a car's page packs
    pub const CAR: &str = "car";
    /// a damage decal of `<model>D.fsh` / `<model>L.fsh`
    pub const CAR_DAMAGE: &str = "car-damage";
    /// a cloud layer (`hd=yes`) or a CPU-read colour map of the sky pack (`hd=no`)
    pub const SKY: &str = "sky";
    /// the horizon mountain strip
    pub const SKY_DETAIL: &str = "sky_detail";
    /// listed only: sprite sheets (and the car-shadow projector's map)
    pub const SPRITE: &str = "sprite";
    /// listed only: a track's `.env` reflection / sky maps
    pub const ENV: &str = "env";
    /// listed only: the cars' shared environment maps
    pub const CAR_ENV: &str = "car-env";

    /// Whether the game samples this class through a mip chain (track and sky textures); the car entries are blitted into pages
    /// on the CPU, where only level 0 is ever read.
    pub fn wants_chain(class: &str) -> bool {
        matches!(class, TRACK | SKY | SKY_DETAIL)
    }
}

/// `env/<track>/<entry>`: an entry of a track's `.env` (listed in the manifest only, never part of the pack)
pub fn key_env(track: &str, entry: &str) -> String {
    format!("env/{}/{}", safe(track), safe(entry))
}
/// `<class>/<stem>/<entry>` of a listed-only pack (`sprite`, `car-env`): `stem` the file name without extension
pub fn key_listed(class: &str, stem: &str, entry: &str) -> String {
    format!("{class}/{}/{}", safe(stem), safe(entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_safe_and_stable() {
        assert_eq!(key_track("Alps", "TR12 "), "track/alps/tr12");
        assert_eq!(key_track("alps", "a/b.c"), "track/alps/a~2fb~2ec");
        assert_eq!(key_track("alps", "  "), "track/alps/~");
        assert_eq!(key_car("993", 3, "Hood"), "car/993/f3/hood");
        assert_eq!(key_car_global("INTGLASS", "x"), "carglobal/intglass/x");
        assert_eq!(key_car_damage("993", true, 7, "0001"), "car/993/damage_l/007_0001");
        assert_eq!(key_env("Alps", "Sky 1"), "env/alps/sky~201");
        assert_eq!(key_listed(class::SPRITE, "common", "SPIK"), "sprite/common/spik");
        assert!(class::wants_chain("sky_detail") && !class::wants_chain("car") && !class::wants_chain("car-damage"));
    }
}
