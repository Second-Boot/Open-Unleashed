//! Race/game state reconstructed from the original (see `RE/ARCHITECTURE.md`).
//! Currently: start-grid extraction and scene assembly for tracks and cars.
//! Race logic (checkpoints, laps, positions) follows M5.
pub mod ai;
pub mod aidrive;
pub mod camera;
pub mod cameras;
pub mod career;
pub mod junctions;
pub mod lanes;
pub mod layout;
pub mod particles;
pub mod police;
pub mod race;
pub mod replay;
pub mod rng;
pub mod room;
pub mod racecar;
pub mod records;
pub mod save;
pub mod scripts;
pub mod skill;
pub mod sky;
pub mod start;
pub mod text;
pub mod traffic;
pub mod world;
pub mod zones;
