//! Network play on the application side, by topic: the net state types (`types`), the lobby callbacks (`callbacks`), the front end's lobby
//! screens (`lobby`) and the internet lobby (`internet`), the race-start sync (`launch`), the race itself (`race`: lag arm, pause,
//! lock-step exchange, network replay) and the test-glue dumps (`dump`).

mod callbacks;
mod dump;
mod internet;
mod launch;
pub(crate) mod lobby;
mod race;
mod types;

pub(crate) use dump::remote_dump;
pub(crate) use types::*;
