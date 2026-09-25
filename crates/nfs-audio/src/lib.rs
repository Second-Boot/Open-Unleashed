//! Native audio: a small mixer on a cpal output stream playing decoded original samples.
//!
//! Engine sound (`engine`): the car's sound set (`Sounds/<set>.viv`) holds an EA-XA bank plus
//! `CRDl` tables that map a 512-bin engine variable to per-sample volume windows
//! (`RE/notes/audio.md` §4, tables HYPOTHESIS). The native model plays every loop of the load
//! (`.elt`) or coast (`.ect`) table with volume = table A at the current bin and pitch =
//! current bin / window centre bin, with bin = rpm / 20 (HYPOTHESIS: 512 bins ≈ 0..10240 rpm,
//! which places the top window 271..511 at 5.4–10.2 krpm). Not the original DirectSound path.

pub mod ai_engine;
pub mod ambience;
pub mod engine;
pub mod fesfx;
pub mod mixer;
pub mod music;
pub mod reverb;
pub mod sfx;
pub mod slots;
pub mod speech;

pub use mixer::{AudioOutput, VoiceId};
