//! The `.mad` movie player behind `NFS5.MOVIEBOX` (a move out of the parent module, no logic changed).

use super::*;

/// A `.mad` playing inside a `NFS5.MOVIEBOX`.
///
/// The frame period is the video header's `+0x0C` as **16.16 fixed point**: 0x4DCB01 masks its low
/// sixteen bits off as the fraction and 0x4DCB06 shifts the rest down to whole units, which it adds
/// to the player's own clock. Every shipped movie carries 4369067, so 66.67 ms a frame — fifteen a
/// second.
pub(super) struct MoviePlayer {
    /// the whole file, and where its video chunks are
    data: Vec<u8>,
    chunks: Vec<(usize, usize)>,
    decoder: nfs_formats::mad::MadDecoder,
    /// the next chunk to decode
    next: usize,
    period_q16: u64,
    clock_q16: u64,
    pub(super) tex: TextureId,
    pub(super) size: (f32, f32),
    /// The clip has played to its last frame at least once — what `NFS5.MOVIECIRCLE`'s tick asks
    /// the player (0x4E2960) before `careerfirst` moves on (0x4CD608).
    pub(super) finished: bool,
    /// `FEMovie`'s own `[+0x75]`, `data.audio` (`MovieBox.ctr` / `MovieCircle.ctr` default 1):
    /// with it clear the tick (0x4E25A0, shared by every `FEMovie`, `frontend-3d.md` §11) may
    /// advance **at most one frame a call**, the clock pulled back to it (0x4E261D…0x4E2631) —
    /// the same rule `room::VictoryMovie::tick` already carries for the victory room's own
    /// silent `MOVIE`. Without it a movie that misses ticks (a stall) does not skip frames to
    /// catch up; the skipped time is dropped, not queued. **Also** (`mad.md` §14): whether the
    /// movie has an audio track to play at all — the same flag, in the same field, `0x4E1F07`.
    catchup: bool,
    /// The whole `SC**` track, decoded once (`mad.md` §14, 0x4E1F9A…0x4E1FB4's registration):
    /// `None` when `catchup` is clear, the `.mad` carries no audio chunks, or the file could not
    /// be decoded. Kept so a loop restart ([`Self::restart_voice`]) does not re-decode.
    clip: Option<Arc<nfs_audio::mixer::Clip>>,
    /// The playing track and a handle that can stop it without the whole `AudioOutput`
    /// (`mad.md` §14: `FEMovie`'s destructor, 0x4E2250, does the same thing at the same point —
    /// `[+0x60]` (the player) stopped via 0x567690 first, before any other field is released —
    /// which [`Drop`] mirrors with `stop()`; the destructor's own further frees (the decode
    /// buffer, the stream at `[+0x2c]` via 0x56B3B0, `[+0x1c]`/`[+0x24]`/`[+0x40]`, the shared
    /// movie-subsystem refcount at `[0x6907A0]`) have no port-side counterpart — Rust's own
    /// `Vec`/`Arc` drops cover them).
    voice: Option<(nfs_audio::VoiceId, nfs_audio::StopHandle)>,
    /// The `MUSICLEVEL` (`[0x657CA0]`) byte last applied to [`Self::voice`] (`FEMovie+0x6C`,
    /// 0x4E27BF's cache) — set again only when the setting changes.
    last_level: Option<u8>,
}

/// `FEMovie`'s destructor, 0x4E2250 (`mad.md` §14): stops `[+0x60]` via 0x567690 first, before
/// anything else is freed. [`Drop::drop`] does the equivalent — `stop()` the voice — and leaves
/// the rest of the struct to Rust's own drop glue.
impl Drop for MoviePlayer {
    fn drop(&mut self) {
        if let Some((id, stop)) = self.voice.take() {
            stop.stop(id);
        }
    }
}

/// `0x566340` is `Voice_setVolume(handle, level: 0..127)` (`engine-audio.md`'s own CONFIRMED
/// reading), and the tick's own call site (0x4E27BA…0x4E27CE) passes it the raw `MUSICLEVEL` byte
/// straight through — unlike `MusicSetVolume` (0x4AE8C0), which first rescales through
/// `MusicLevelToVolume` (0x4A82F0, `(0x50·v) >> 7`), `FEMovie`'s own path never calls that
/// function. Read to the instruction (0x5662B0/0x589DF0, `mad.md` §14): `Voice_setVolume`
/// multiplies the level in as one more independent linear gain factor alongside the channel's
/// own cached volume and two per-channel/global bytes, normalized by `/127³` (the `0x4186143D`
/// magic, `sar 0x13`, round-toward-zero — the same divide `sfx.md` §"Voice_play" already
/// documents); no logarithmic curve or lookup table applies to this argument itself (a
/// per-channel table at `+0x54`/`+0x5C` exists in the mixer but is unset for a plain `SC**`
/// stream — wind loops and patch-defined instruments are the only users, `sfx.md`). So `level/127`
/// is the exact mapping, not an approximation: it was mislabelled before this byte's path was
/// traced past 0x567810.
fn movie_gain(music_level: u8) -> f32 {
    music_level as f32 / 127.0
}

impl MoviePlayer {
    /// The whole file kept, its video chunks indexed, the first frame decoded into a texture, and
    /// (`mad.md` §14) its own `SC**` audio track decoded (not yet started — see
    /// [`Self::advance`]). `FEMovie`'s constructor 0x4E1EB0 decodes frame 0 (0x4E2127) and
    /// registers the track, but the sound library's player only starts on the first ready tick
    /// (0x56CA20, then 0x56CA00) — the port's own `open` is never mid-tick, so the two collapse
    /// into `advance`'s own first call rather than here, which also fixes the case this port adds
    /// that the original never has: the sound device can finish initialising strictly later than
    /// a screen's widgets are bound (`self.snd.audio`'s own lazy open in `open-unleashed`). `audio_out`
    /// is `None` exactly when `[0x657C94]` (no sound device) forces the original's own `[+0x75]`
    /// off (`self.snd.audio.is_none()` in the port) — checked again every `advance`.
    pub(super) fn open(r: &mut Renderer, paths: &GamePaths, dir: &str, file: &str, audio: bool) -> Option<Self> {
        let path = GamePaths::find_file(&paths.fe_data().join(dir), file)?;
        let Ok(data) = std::fs::read(&path) else { return None };
        let tables = nfs_formats::mad::MadTables::load(&paths.root).ok()?;
        let chunks: Vec<(usize, usize)> = {
            let mut at = 0usize;
            let mut out = Vec::new();
            for c in nfs_formats::mad::chunks(&data) {
                if c.is_video() {
                    out.push((at, c.data.len()));
                }
                at += c.data.len();
            }
            out
        };
        let &(off, len) = chunks.first()?;
        let header = nfs_formats::mad::MadHeader::parse(&data[off..off + len]).ok()?;
        let mut decoder = nfs_formats::mad::MadDecoder::new(tables, &header);
        let f = decoder.decode_frame(&data[off..off + len]);
        if f.width == 0 || f.height == 0 {
            return None;
        }
        let tex = r.upload_texture(&TextureData::single(f.width as u32, f.height as u32, f.rgba));
        let clip = audio.then(|| nfs_formats::schl::decode_all(&nfs_formats::mad::audio_track(&data)).ok()).flatten().map(|pcm| {
            Arc::new(nfs_audio::mixer::Clip {
                rate: pcm.rate,
                channels: pcm.channels,
                samples: pcm.samples,
                loop_range: None,
                gain: 1.0,
                bend_cents: 0,
                fx_send: pcm.fx_send,
                priority: nfs_audio::mixer::STREAM_PRIORITY,
                pan: pcm.pan,
                pan_scale: pcm.pan_scale,
                records: Vec::new(),
                layers: Vec::new(),
                tables: Vec::new(),
            })
        });
        Some(MoviePlayer {
            data,
            chunks,
            decoder,
            next: 1,
            period_q16: header.period.max(1) as u64,
            clock_q16: 0,
            tex,
            size: (f.width as f32, f.height as f32),
            finished: false,
            catchup: audio,
            clip,
            voice: None,
            last_level: None,
        })
    }

    /// (Re)start the audio track from its own beginning — `FEMovie`'s constructor's own start
    /// (0x4E212C…0x4E2178) and, at a loop restart, 0x4E2380's `0x567690` (stop the old player)
    /// followed by a fresh 0x56CA20 (`mad.md` §14). No `looping` flag on the voice itself: the
    /// original never lets the sound library loop the stream on its own — it hard-restarts a
    /// fresh player at the exact tick the video also wraps, which this mirrors, so drift between
    /// the two cannot accumulate across a long-running loop (0x4E25A0 carries no drift-sync — the
    /// video's own wall-clock pacing is the whole story, `mad.md` §14).
    fn restart_voice(&mut self, audio_out: Option<&nfs_audio::AudioOutput>, music_level: u8) {
        if let Some((id, stop)) = self.voice.take() {
            tracing::info!(frame = self.next.saturating_sub(1), "FEMovie audio: stop (0x567690)");
            stop.stop(id);
        }
        if let (Some(clip), Some(a)) = (&self.clip, audio_out) {
            let gain = movie_gain(music_level);
            let id = a.play_word(clip.clone(), gain, nfs_audio::resample::PITCH_UNIT, 0x40, 0, false);
            tracing::info!(frame = self.next.saturating_sub(1), rate = clip.rate, channels = clip.channels, music_level, gain, "FEMovie audio: start (0x56CA20/0x56CA00)");
            self.voice = Some((id, a.stop_handle()));
            self.last_level = Some(music_level);
        }
    }

    /// Step by `dt_ms`, decoding and uploading the frames the period calls for; a movie that has
    /// run out starts again (and, with an audio track, restarts it — [`Self::restart_voice`]).
    /// `music_level` is `MUSICLEVEL` (`[0x657CA0]`), applied to a playing track only when it
    /// changed since the last call (0x4E27BA…0x4E27CE's own cache). A track that has not started
    /// yet — every widget movie's own first call (`mad.md` §14: the original starts it on the
    /// first *ready* tick, which the port's always-ready file read collapses into this one), the
    /// end-of-era movie's own deferred start once its show reaches `movie_started()`, and any
    /// movie whose `audio_out` was `None` at `open` because the port's own sound device had not
    /// finished initialising yet — starts here too, the same call as a loop restart.
    pub(super) fn advance(&mut self, r: &mut Renderer, dt_ms: u32, audio_out: Option<&nfs_audio::AudioOutput>, music_level: u8) {
        self.clock_q16 += (dt_ms as u64) << 16;
        let (clock_q16, frames) = movie_advance_frames(self.clock_q16, self.period_q16, self.catchup);
        self.clock_q16 = clock_q16;
        let mut drawn = false;
        let mut looped = false;
        for _ in 0..frames {
            if self.next >= self.chunks.len() {
                self.next = 0;
                self.finished = true;
                looped = true;
            }
            let (off, len) = self.chunks[self.next];
            self.next += 1;
            let _ = self.decoder.decode_chunk(&self.data[off..off + len]);
            drawn = true;
        }
        if self.catchup && (looped || self.voice.is_none()) {
            self.restart_voice(audio_out, music_level);
        } else if let (Some((id, _)), Some(a)) = (&self.voice, audio_out)
            && self.last_level != Some(music_level)
        {
            let gain = movie_gain(music_level);
            tracing::debug!(music_level, gain, "FEMovie audio: MUSICLEVEL changed (0x567810)");
            a.set_word(*id, gain, nfs_audio::resample::PITCH_UNIT, 0x40);
            self.last_level = Some(music_level);
        }
        if drawn {
            let (w, h) = (self.size.0 as u32, self.size.1 as u32);
            r.update_texture(self.tex, &TextureData::single(w, h, self.decoder.rgba()));
        }
    }
}

/// `FEStringAccessor::insert` **0x4BCA90**'s own forbidden set, active only on the one shipped
/// instance whose ctor argument enables it (`UserList.NameInput`, `LayoutScreen::type_char`'s own
/// doc) — nine bytes byte-for-byte the set Windows refuses in a file name: `\ / : * ? " < > |`.
/// The shared key handler's own `%` refusal (0x4CCC20, `type_char`'s doc) is unconditional and
/// already checked separately; this is the *second*, per-instance layer 0x4BCA90 adds on top.
pub(super) fn filename_illegal_char(code: u32) -> bool {
    matches!(code, 0x5c | 0x2f | 0x3a | 0x2a | 0x3f | 0x22 | 0x3c | 0x3e | 0x7c)
}

/// The tick's own frame count, 0x4E25A0: with `catchup` (`data.audio` set) as many periods as
/// `clock_q16` covers. Without it, **only a genuine backlog — more than one frame due — is
/// clamped** to one, dropping the rest (0x4E261D…0x4E2631 resets the accumulator to exactly the
/// one frame taken); the ordinary case of one frame due (`ebp == edi + 1`, `jle 0x4e2634` skips
/// the clamp) carries its own true remainder just as the audio path does. Returns the clock left
/// over and how many chunks to decode.
pub(super) fn movie_advance_frames(clock_q16: u64, period_q16: u64, catchup: bool) -> (u64, u32) {
    let frames = clock_q16 / period_q16;
    if frames == 0 {
        return (clock_q16, 0);
    }
    if catchup || frames <= 1 {
        (clock_q16 - frames * period_q16, frames as u32)
    } else {
        (0, 1)
    }
}

#[cfg(test)]
mod movie_gain_tests {
    use super::movie_gain;

    /// 0x566340 (`Voice_setVolume`) takes the raw `MUSICLEVEL` byte as one linear 0..127 gain
    /// factor (0x4E27CE's own call, no `MusicLevelToVolume` rescale) — `level/127` end to end.
    #[test]
    fn linear_0_64_127() {
        assert_eq!(movie_gain(0), 0.0);
        assert!((movie_gain(64) - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(movie_gain(127), 1.0);
    }
}
