//! The front end's 3D rooms (`RE/notes/frontend-3d.md`).
//!
//! A `.lay` screen's `room=` names one of six room classes, each of which loads a model from
//! `FEData/Models` and turns the player's car on a turntable in front of the camera. The rig — where
//! the room sits, how fast the car turns, the camera's vertical angle and the whole lighting set —
//! is a table of numbers in each class's constructor, and the class's own tuning overlay
//! (0x4E7950) names every one of them, so the meanings below are the game's own labels.

/// One room's rig, exactly as its constructor sets it. CONFIRMED.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// the `room=` id a `.lay` screen names
    pub id: &'static str,
    /// the models in `FEData/Models`, drawn together (only the dealership has more than one)
    pub models: &'static [&'static str],
    /// `X` `Y` `Z` — where the room and the car sit in front of the camera
    pub pos: [f32; 3],
    /// `XRot` `YRot` `ZRot`, in turns
    pub rot: [f32; 3],
    /// `Spin` — seconds per turn of the turntable; 0 stands still
    pub spin: f32,
    /// `V.A.` — the camera's vertical angle in degrees
    pub view_angle: f32,
    /// `AR` `AG` `AB`, 0…255
    pub ambient: [f32; 3],
    /// `SR` `SG` `SB`, 0…255
    pub sun: [f32; 3],
    /// `SDirX` `SDirY` `SDirZ`
    pub sun_dir: [f32; 3],
    /// `SStr` and `SAng`
    pub sun_strength: f32,
    pub sun_angle: f32,
    /// `cardif` — the car's own distance from the room. **Dead** in the shipped build: only the
    /// class's tuning overlay reads it (0x4E7C31), so the car stands exactly where the room does.
    pub car_distance: f32,
    /// How far round the room's own models are turned. The base class turns them with the car
    /// (`None`, 0x4EA0E3); the dealership pins its three sections at half a turn and spins only the
    /// car (0x4E8D24, 0x4E8D46, 0x4E8D68).
    pub models_turn: Option<f32>,
}

/// The six registered rooms ([0x6909BC]); `ID_TESTROOM` is what an empty `room=` reads back as
/// (0x4E0BE0) and shares `ID_NONE`'s rig.
pub const ROOMS: [Room; 6] = [
    Room {
        id: "ID_GARAGE",
        models: &["garage"],
        pos: [0.0, -1.1, 4.8],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 37.5,
        ambient: [55.0, 65.0, 55.0],
        sun: [120.0, 120.0, 120.0],
        sun_dir: [-0.14, -0.98, 0.114],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 3.5,
        models_turn: None,
    },
    Room {
        id: "ID_DEALER",
        models: &["dealer2a", "dealer2b", "dealer2c"],
        pos: [0.0, -1.1, 4.8],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 37.5,
        ambient: [110.0, 110.0, 110.0],
        sun: [110.0, 110.0, 110.0],
        sun_dir: [0.0, -0.99, 0.15],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 5.0,
        models_turn: Some(0.5),
    },
    Room {
        id: "ID_FACTORY",
        models: &["factory"],
        pos: [0.0, -1.1, 4.8],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 37.5,
        ambient: [110.0, 120.0, 110.0],
        sun: [50.0, 50.0, 50.0],
        sun_dir: [0.0, -0.99, 0.10],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 5.0,
        models_turn: None,
    },
    Room {
        id: "ID_VICTORY",
        models: &["victory"],
        pos: [0.0, -1.1, 5.2],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 32.0,
        ambient: [75.0, 75.0, 75.0],
        sun: [110.0, 110.0, 110.0],
        sun_dir: [0.0, -0.98, 0.20],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 4.5,
        models_turn: None,
    },
    Room {
        id: "ID_TRACK",
        models: &["trackwall"],
        // the track-select room takes its position from code rather than from a literal, so the
        // port keeps the base class's own (UNKNOWN — 0x4EA390 loads it from registers)
        pos: [0.0, -1.2, 4.2],
        rot: [0.0, 0.0, 0.0],
        spin: 20.0,
        view_angle: 25.0,
        ambient: [55.0, 65.0, 76.0],
        sun: [115.0, 115.0, 115.0],
        sun_dir: [-0.29, -1.5, 0.29],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 1.2,
        models_turn: None,
    },
    Room {
        id: "ID_NONE",
        models: &[],
        pos: [0.0, -1.2, 4.2],
        rot: [-0.01, 0.0, 0.0],
        spin: 30.0,
        // the empty room sets no `V.A.` of its own, so the screen keeps the last one; the port
        // falls back to the garage's
        view_angle: 37.5,
        ambient: [64.0, 64.0, 64.0],
        sun: [159.0, 159.0, 159.0],
        sun_dir: [0.0, -0.99, 0.10],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 1.0,
        models_turn: None,
    },
];

/// The room a screen's `room=` names; an empty id is `ID_TESTROOM`, which shares `ID_NONE`'s rig.
pub fn room(id: &str) -> Option<&'static Room> {
    let id = if id.trim().is_empty() { "ID_NONE" } else { id.trim() };
    let id = if id == "ID_TESTROOM" { "ID_NONE" } else { id };
    ROOMS.iter().find(|r| r.id == id)
}

impl Room {
    /// The turntable angle in turns at `ms` milliseconds: one whole turn every `spin` seconds
    /// (`YRot = now / (ticksPerSecond · Spin)`, 0x4EA0AE, and the model spin setter multiplies by
    /// −360 to make degrees).
    pub fn spin_turns(&self, ms: u64) -> f32 {
        if self.spin <= 0.0 {
            return 0.0;
        }
        -(ms as f32) / (1000.0 * self.spin)
    }

    /// Where the room sits while the screen is `fade` of the way out (0 in place, 1 gone):
    /// `(X·f, Y·f − fade·0.8, Z·f + fade·0.3)` with `f = 1 − fade` (0x4E9FFF…0x4EA051).
    pub fn faded_pos(&self, fade: f32) -> [f32; 3] {
        let f = 1.0 - fade;
        [self.pos[0] * f, self.pos[1] * f - fade * 0.8, self.pos[2] * f + fade * 0.3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_room_id_resolves() {
        for id in ["ID_GARAGE", "ID_DEALER", "ID_FACTORY", "ID_VICTORY", "ID_TRACK", "ID_NONE", "ID_TESTROOM", ""] {
            assert!(room(id).is_some(), "{id}");
        }
        assert_eq!(room("ID_GARAGE").unwrap().models, ["garage"]);
        assert_eq!(room("").unwrap().id, "ID_NONE");
    }

    #[test]
    fn the_turntable_takes_spin_seconds_per_turn() {
        let g = room("ID_GARAGE").unwrap();
        assert_eq!(g.spin, 30.0);
        assert!((g.spin_turns(30_000) + 1.0).abs() < 1e-6, "one turn in thirty seconds");
        assert!((g.spin_turns(15_000) + 0.5).abs() < 1e-6);
        // the track wall turns faster
        assert_eq!(room("ID_TRACK").unwrap().spin, 20.0);
    }

    #[test]
    fn the_room_pulls_toward_the_camera_as_the_screen_fades() {
        let g = room("ID_GARAGE").unwrap();
        assert_eq!(g.faded_pos(0.0), g.pos);
        let out = g.faded_pos(1.0);
        assert_eq!(out, [0.0, -0.8, 0.3]);
    }
}
