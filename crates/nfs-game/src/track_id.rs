//! The 14 shipped tracks and the car family keys the AI tables are keyed on.
//!
//! `0x4A2950` (steering factor), `0x4A2BC0` (class tier) and `0x4A2DC0` (curve factor, speed
//! zones) compare the current track name `[0x657444]` and the family key `nfs5.car +0x115` with
//! `strcmp` / `_stricmp` chains; the port used to repeat those string compares on every call. The
//! names are parsed here **once** (ASCII case-insensitive, exact — a prefix or a longer name is
//! *not* a track, as in the original's compares) and the tables are indexed by the enums. A name
//! that matches nothing is `None` / [`FamilyKey::Other`], which every table maps to the
//! original's fall-through (1.0 / tier 5).

/// A shipped track, in the order of the tables that index by it ([`index`](Self::index)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackId {
    Monaco1,
    Monaco2,
    Monaco3,
    Monaco4,
    Monaco5,
    Alps,
    Autobahn,
    Canyon,
    Castle,
    Coastal,
    Farmland,
    Foothills,
    Forest,
    Industrial,
}

impl TrackId {
    /// every track, `index()` order
    pub const ALL: [TrackId; 14] = [
        TrackId::Monaco1,
        TrackId::Monaco2,
        TrackId::Monaco3,
        TrackId::Monaco4,
        TrackId::Monaco5,
        TrackId::Alps,
        TrackId::Autobahn,
        TrackId::Canyon,
        TrackId::Castle,
        TrackId::Coastal,
        TrackId::Farmland,
        TrackId::Foothills,
        TrackId::Forest,
        TrackId::Industrial,
    ];

    /// The track a name (the `nfs5.trk` file name) denotes: exact, ASCII case-insensitive.
    pub fn from_name(name: &str) -> Option<TrackId> {
        Self::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(name))
    }

    /// the lower-case file name
    pub fn name(self) -> &'static str {
        match self {
            TrackId::Monaco1 => "monaco1",
            TrackId::Monaco2 => "monaco2",
            TrackId::Monaco3 => "monaco3",
            TrackId::Monaco4 => "monaco4",
            TrackId::Monaco5 => "monaco5",
            TrackId::Alps => "alps",
            TrackId::Autobahn => "autobahn",
            TrackId::Canyon => "canyon",
            TrackId::Castle => "castle",
            TrackId::Coastal => "coastal",
            TrackId::Farmland => "farmland",
            TrackId::Foothills => "foothills",
            TrackId::Forest => "forest",
            TrackId::Industrial => "industrial",
        }
    }

    /// position in [`ALL`](Self::ALL), for the per-track tables
    pub fn index(self) -> usize {
        self as usize
    }
}

/// The car family key (`nfs5.car +0x115`) as far as the AI tables tell families apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyKey {
    F356,
    F356A,
    F356B,
    F935,
    Gt1,
    Gt2,
    Gt3,
    F550A,
    F928,
    F930T,
    F964,
    F914,
    F944,
    F959,
    F965T,
    F993,
    F993T,
    F996,
    F996T,
    Boxster,
    /// any other key: the chains' fall-through
    Other,
}

impl FamilyKey {
    /// The family a key denotes: exact, ASCII case-insensitive (`_stricmp`).
    pub fn from_name(name: &str) -> FamilyKey {
        const NAMES: [(&str, FamilyKey); 20] = [
            ("356", FamilyKey::F356),
            ("356A", FamilyKey::F356A),
            ("356B", FamilyKey::F356B),
            ("935", FamilyKey::F935),
            ("GT1", FamilyKey::Gt1),
            ("GT2", FamilyKey::Gt2),
            ("GT3", FamilyKey::Gt3),
            ("550A", FamilyKey::F550A),
            ("928", FamilyKey::F928),
            ("930T", FamilyKey::F930T),
            ("964", FamilyKey::F964),
            ("914", FamilyKey::F914),
            ("944", FamilyKey::F944),
            ("959", FamilyKey::F959),
            ("965T", FamilyKey::F965T),
            ("993", FamilyKey::F993),
            ("993T", FamilyKey::F993T),
            ("996", FamilyKey::F996),
            ("996T", FamilyKey::F996T),
            ("BOXSTER", FamilyKey::Boxster),
        ];
        NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(FamilyKey::Other, |(_, f)| *f)
    }

    /// `935` `GT1` `GT2` `GT3` — the racing families `0x4A2950` groups
    pub fn is_race(self) -> bool {
        matches!(self, FamilyKey::F935 | FamilyKey::Gt1 | FamilyKey::Gt2 | FamilyKey::Gt3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_exact() {
        for (i, t) in TrackId::ALL.into_iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(TrackId::from_name(t.name()), Some(t));
            assert_eq!(TrackId::from_name(&t.name().to_ascii_uppercase()), Some(t));
        }
        for n in ["", "skidpad", "castle2", "alp", "monaco6", "forestx"] {
            assert_eq!(TrackId::from_name(n), None, "{n}");
        }
        assert_eq!(FamilyKey::from_name("boxster"), FamilyKey::Boxster);
        assert_eq!(FamilyKey::from_name("gt2"), FamilyKey::Gt2);
        assert_eq!(FamilyKey::from_name("3560"), FamilyKey::Other);
        assert!(FamilyKey::from_name("935").is_race() && !FamilyKey::from_name("928").is_race());
    }
}
