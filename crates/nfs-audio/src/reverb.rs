//! Tunnel reverb — `RE/notes/engine-audio.md` §4.7 (MEDIUM).
//!
//! Each audio update the original asks the track whether something drivable hangs over the car
//! (0x473330, `race-logic.md` §12) and drives one number, the environment level, from it:
//!
//! ```text
//! if reverbDisabled and reverbOn: setEnvParam(0, 0); reverb = 0; reverbOn = 0
//! if inTunnel and not split screen:
//!     if reverb < 100: reverb = 100; env(0x80, 50); env(0x20, 100)
//! else:
//!     if reverb < 8:  reverb = 8;  env(0x80, 4);  env(0x20, 8)
//!     elif reverb > 8: reverb = max(reverb − 32, 8); env(0x80, reverb/2); env(0x20, reverb)
//! ```
//!
//! so it snaps to 100 on entry and decays 100 → 68 → 36 → 8 over three updates on the way out.
//! The two parameters are the environment buses `[0x6B4A0C]` / `[0x6B4A0D]` (0x565CD0) every
//! voice's send is taken from (`AudioOutput::set_env`, 0x589D70): the resting level 8 keeps a
//! small send outside tunnels too.

/// The environment level and the two parameters the original sends with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reverb {
    level: i32,
    /// set while the level is anything but the 0 of a disabled reverb
    on: bool,
}

/// What an update sends to the sound driver, when it sends anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvParams {
    /// `Mixer_setEnvParam(0x80, ·)` — half the level
    pub p80: i32,
    /// `Mixer_setEnvParam(0x20, ·)` — the level
    pub p20: i32,
}

impl Reverb {
    /// One audio update. `in_tunnel` is the enclosure test, `disabled` the setting that turns the
    /// environment off. Returns the parameters when this update changed them.
    pub fn update(&mut self, in_tunnel: bool, disabled: bool, split_screen: bool) -> Option<EnvParams> {
        // 0x4AA177…0x4AA19D: the reset does not return — it falls into the test below with the
        // level 0 (`jmp 0x4AA1A4` with `al = 0`), which sets 100 or 8 again in the same update, so
        // the setting only cuts the fade short. *(Until 2026-09-26 the port returned here.)*
        let mut sent = None;
        if disabled && self.on {
            self.level = 0;
            self.on = false;
            sent = Some(EnvParams { p80: 0, p20: 0 });
        }
        self.step(in_tunnel, split_screen).or(sent)
    }

    fn step(&mut self, in_tunnel: bool, split_screen: bool) -> Option<EnvParams> {
        if in_tunnel && !split_screen {
            if self.level < 100 {
                self.level = 100;
                // 0x4AA1C8 `push 0x32 ; jmp 0x4AA224`: the tunnel's entry runs the same tail as
                // the decay, `[0x656868] = 1` (0x4AA23F) included
                self.on = true;
                return Some(EnvParams { p80: 50, p20: 100 });
            }
            None
        } else if self.level < 8 {
            self.level = 8;
            self.on = true;
            Some(EnvParams { p80: 4, p20: 8 })
        } else if self.level > 8 {
            self.level = (self.level - 32).max(8);
            self.on = true;
            Some(EnvParams { p80: self.level / 2, p20: self.level })
        } else {
            None
        }
    }

    /// The current level, 0..100.
    pub fn level(&self) -> i32 {
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_in_and_decays_out() {
        let mut r = Reverb::default();
        // outside: settles at the resting level 8 and then sends nothing
        assert_eq!(r.update(false, false, false), Some(EnvParams { p80: 4, p20: 8 }));
        assert_eq!(r.update(false, false, false), None);
        assert_eq!(r.level(), 8);

        // entering a tunnel snaps to 100 once
        assert_eq!(r.update(true, false, false), Some(EnvParams { p80: 50, p20: 100 }));
        assert_eq!(r.update(true, false, false), None);
        assert_eq!(r.level(), 100);

        // leaving decays 100 → 68 → 36 → 8 over three updates
        assert_eq!(r.update(false, false, false), Some(EnvParams { p80: 34, p20: 68 }));
        assert_eq!(r.update(false, false, false), Some(EnvParams { p80: 18, p20: 36 }));
        assert_eq!(r.update(false, false, false), Some(EnvParams { p80: 4, p20: 8 }));
        assert_eq!(r.update(false, false, false), None);
    }

    #[test]
    fn split_screen_and_the_setting_keep_it_off() {
        let mut r = Reverb::default();
        r.update(true, false, true);
        assert_eq!(r.level(), 8, "split screen never enters");

        // the setting resets an armed environment and the same update sets it again (0x4AA19D →
        // 0x4AA1A4): in a tunnel back to 100, outside straight to 8 — the fade is cut short
        let mut r = Reverb::default();
        assert_eq!(r.update(true, true, false), Some(EnvParams { p80: 50, p20: 100 }), "never armed: straight in");
        assert_eq!(r.update(true, true, false), Some(EnvParams { p80: 50, p20: 100 }), "armed: reset, then 100 again");
        assert_eq!(r.update(false, true, false), Some(EnvParams { p80: 4, p20: 8 }), "no 68 / 36 on the way out");
        assert_eq!(r.level(), 8);
        // straight into a tunnel arms it too (0x4AA1C8 → 0x4AA224 → 0x4AA23F)
        let mut r = Reverb::default();
        r.update(true, false, false);
        assert!(r.on);
    }
}
