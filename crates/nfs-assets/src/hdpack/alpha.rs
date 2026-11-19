//! How an HD image's alpha is made to behave like the original's: [`AlphaPolicy`].

/// How the HD image's alpha is made to behave like the original's (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlphaPolicy {
    /// every original texel opaque: the HD alpha is forced to 255
    Opaque,
    /// the original's alpha is 0 / 255 only: the HD alpha is thresholded at 128
    OneBit,
    /// a few distinct values (paint-slot selectors, 4444 levels): snapped to the nearest
    Levels(Vec<u8>),
    /// a smooth alpha: kept as the model made it
    Smooth,
}

impl AlphaPolicy {
    /// The policy of an original RGBA8 image.
    pub fn of(rgba: &[u8]) -> Self {
        let mut seen = [false; 256];
        for p in rgba.chunks_exact(4) {
            seen[p[3] as usize] = true;
        }
        let vals: Vec<u8> = (0..=255u8).filter(|&a| seen[a as usize]).collect();
        match vals.as_slice() {
            [] | [255] => Self::Opaque,
            v if v.iter().all(|&a| a == 0 || a == 255) => Self::OneBit,
            v if v.len() <= 24 => Self::Levels(v.to_vec()),
            _ => Self::Smooth,
        }
    }

    /// Apply to an HD image's pixels.
    pub fn apply(&self, rgba: &mut [u8]) {
        match self {
            Self::Opaque => force_opaque(rgba),
            Self::OneBit => threshold_1bit(rgba),
            Self::Levels(v) => {
                // a lookup table: 256 entries once, not a search per texel
                let lut: Vec<u8> = (0..=255u8).map(|a| *v.iter().min_by_key(|&&m| (m as i32 - a as i32).abs()).unwrap_or(&a)).collect();
                rgba.chunks_exact_mut(4).for_each(|p| p[3] = lut[p[3] as usize]);
            }
            Self::Smooth => {}
        }
    }
}

/// Every texel's alpha 255 (RGBA8 pixels).
pub fn force_opaque(rgba: &mut [u8]) {
    rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
}

/// Every texel's alpha 255 where it is >= 128, else 0 (RGBA8 pixels).
pub fn threshold_1bit(rgba: &mut [u8]) {
    rgba.chunks_exact_mut(4).for_each(|p| p[3] = if p[3] >= 128 { 255 } else { 0 });
}

/// The alpha kind of a texture as the manifest of `asset-extractor hd-export` words it (`alpha` column) and the pack tool
/// reads it back: [`AlphaPolicy`] plus the cars' paint-slot entries (`slot`, a tinted entry whose alpha selects a paint slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaClass {
    /// `none`: every original texel opaque
    Opaque,
    /// `1bit`: alpha 0 / 255 only
    OneBit,
    /// `levels:N`: N distinct values
    Levels(usize),
    /// `slot`: a tinted car entry, the alpha is paint-slot DATA
    Slot,
    /// `smooth`
    Smooth,
}

impl AlphaClass {
    /// The class of an original RGBA8 image (`tinted`: a tinted car entry, always `Slot`).
    pub fn of(rgba: &[u8], tinted: bool) -> Self {
        if tinted {
            return Self::Slot;
        }
        match AlphaPolicy::of(rgba) {
            AlphaPolicy::Opaque => Self::Opaque,
            AlphaPolicy::OneBit => Self::OneBit,
            AlphaPolicy::Levels(v) => Self::Levels(v.len()),
            AlphaPolicy::Smooth => Self::Smooth,
        }
    }

    /// The class of an HD image that has no manifest row: from its own alpha (opaque, 0 / 255, else smooth).
    pub fn of_image(rgba: &[u8]) -> Self {
        if rgba.chunks_exact(4).all(|p| p[3] == 255) {
            Self::Opaque
        } else if rgba.chunks_exact(4).all(|p| p[3] == 0 || p[3] == 255) {
            Self::OneBit
        } else {
            Self::Smooth
        }
    }

    /// The manifest's word: `none`, `1bit`, `levels:N`, `slot`, `smooth`.
    pub fn to_manifest(self) -> String {
        match self {
            Self::Opaque => "none".into(),
            Self::OneBit => "1bit".into(),
            Self::Levels(n) => format!("levels:{n}"),
            Self::Slot => "slot".into(),
            Self::Smooth => "smooth".into(),
        }
    }

    /// The class a manifest word names (`None` for a word that is none of them). Any `levels...` is [`Self::Levels`] (the count
    /// after `:` when it parses, else 0: the pack tool treats every levels / slot alpha alike, as data).
    pub fn from_manifest(word: &str) -> Option<Self> {
        match word {
            "none" => Some(Self::Opaque),
            "1bit" => Some(Self::OneBit),
            "smooth" => Some(Self::Smooth),
            "slot" => Some(Self::Slot),
            w => w.strip_prefix("levels").map(|n| Self::Levels(n.strip_prefix(':').and_then(|n| n.parse().ok()).unwrap_or(0))),
        }
    }

    /// Whether the alpha is DATA (paint-slot selectors or a few levels: the game snaps it to the original's values, so the pack tool
    /// filters it like a colour and never thresholds it).
    pub fn is_data(self) -> bool {
        matches!(self, Self::Levels(_) | Self::Slot)
    }

    /// The pack tool's alpha rule on an HD image's pixels: opaque -> 255, 1-bit -> threshold 128, every other kind untouched.
    pub fn apply(self, rgba: &mut [u8]) {
        match self {
            Self::Opaque => force_opaque(rgba),
            Self::OneBit => threshold_1bit(rgba),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_policies() {
        let opaque = [1u8, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(AlphaPolicy::of(&opaque), AlphaPolicy::Opaque);
        let one = [0u8, 0, 0, 0, 9, 9, 9, 255];
        let p = AlphaPolicy::of(&one);
        assert_eq!(p, AlphaPolicy::OneBit);
        let mut hd = [1u8, 1, 1, 127, 2, 2, 2, 128];
        p.apply(&mut hd);
        assert_eq!([hd[3], hd[7]], [0, 255]);
        let slots = [0u8, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0, 255];
        let p = AlphaPolicy::of(&slots);
        assert_eq!(p, AlphaPolicy::Levels(vec![0, 100, 255]));
        let mut hd = [0u8, 0, 0, 70, 0, 0, 0, 200];
        p.apply(&mut hd);
        assert_eq!([hd[3], hd[7]], [100, 255]);
        let mut hd = [0u8, 0, 0, 5];
        AlphaPolicy::Opaque.apply(&mut hd);
        assert_eq!(hd[3], 255);
    }

    #[test]
    fn alpha_classes_round_trip_the_manifest_words() {
        for (w, c) in [("none", AlphaClass::Opaque), ("1bit", AlphaClass::OneBit), ("levels:7", AlphaClass::Levels(7)), ("slot", AlphaClass::Slot), ("smooth", AlphaClass::Smooth)] {
            assert_eq!(AlphaClass::from_manifest(w), Some(c), "{w}");
            assert_eq!(c.to_manifest(), w);
        }
        assert_eq!(AlphaClass::from_manifest("levels"), Some(AlphaClass::Levels(0)));
        assert_eq!(AlphaClass::from_manifest("binary"), None);
        let one = [0u8, 0, 0, 0, 9, 9, 9, 255];
        assert_eq!(AlphaClass::of(&one, false), AlphaClass::OneBit);
        assert_eq!(AlphaClass::of(&one, true), AlphaClass::Slot);
        assert_eq!(AlphaClass::of(&[0u8, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0, 255], false), AlphaClass::Levels(3));
        assert_eq!(AlphaClass::of_image(&[1, 2, 3, 255]), AlphaClass::Opaque);
        assert_eq!(AlphaClass::of_image(&[1, 2, 3, 0, 1, 2, 3, 255]), AlphaClass::OneBit);
        assert_eq!(AlphaClass::of_image(&[1, 2, 3, 7]), AlphaClass::Smooth);
    }

    /// the apply rule on four texels per kind: opaque forces 255, 1-bit thresholds at 128, the rest is untouched
    #[test]
    fn alpha_class_apply_matches_the_policy_and_the_old_tool_rule() {
        let px = [9u8, 9, 9, 0, 9, 9, 9, 127, 9, 9, 9, 128, 9, 9, 9, 255];
        let alphas = |c: AlphaClass| {
            let mut v = px;
            c.apply(&mut v);
            [v[3], v[7], v[11], v[15]]
        };
        assert_eq!(alphas(AlphaClass::Opaque), [255; 4]);
        assert_eq!(alphas(AlphaClass::OneBit), [0, 0, 255, 255]);
        for c in [AlphaClass::Smooth, AlphaClass::Slot, AlphaClass::Levels(3)] {
            assert_eq!(alphas(c), [0, 127, 128, 255]);
        }
        for (p, c) in [(AlphaPolicy::Opaque, AlphaClass::Opaque), (AlphaPolicy::OneBit, AlphaClass::OneBit), (AlphaPolicy::Smooth, AlphaClass::Smooth)] {
            let mut v = px;
            p.apply(&mut v);
            assert_eq!([v[3], v[7], v[11], v[15]], alphas(c));
        }
    }
}
