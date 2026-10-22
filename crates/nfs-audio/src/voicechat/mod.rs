//! The network voice chat (`RE/notes/network.md` §11.40, `RE/notes/voice-codec.md`): the sound
//! library's 8 kHz codec ([`codec`]), the microphone ([`capture`]) and the voice object the network
//! session owns (`[net+0x14]`, built by 0x4931C0, ticked by the library-service callback 0x493320 →
//! **0x493350**, fed by the receiver 0x493270). The `0xC1` message itself is `nfs_net::voice`.

pub mod capture;
pub mod codec;

use crate::mixer::{AudioOutput, StopHandle, StreamSource, VoiceId};
use codec::{Decoder, Encoder, FRAME, PACKET};
use std::collections::VecDeque;
use std::sync::Arc;

/// Packets per `0xC1` message (0x493432).
pub const BATCH: usize = 5;
/// The playback starts once this many packets are queued (0x49346D `cmp [esi+0xC], 0xA`).
pub const START_AT: usize = 10;
/// The playback request's pan byte `+7` (0x493496 `mov [esp+0x1F], bl`, bl = 0): the voice is heard
/// on the second output channel only ([`AudioOutput::play_stream_panned`]).
const PAN: u8 = 0;
/// The request's volume `+8`, 0x7F from 0x561960 (0x561998), untouched by 0x493350.
const VOLUME: f32 = 1.0;

/// A playing stream (`+8` the handle, `+0x10` the playing flag) and its decoder, whose state is
/// the stream codec object's (zeroed when the stream is opened, 0x59B830).
struct Playback {
    id: VoiceId,
    stop: StopHandle,
    src: Arc<StreamSource>,
    dec: Decoder,
}

/// The voice object `[net+0x14]`.
pub struct VoiceChat {
    /// `+0x1C`: the capture's buffer — set once the capture was tried in this connection, whether
    /// it opened or not (0x4933AF…0x4933C2: a failed 0x561A40 is only reported).
    tried: bool,
    capture: Option<capture::Capture>,
    enc: Encoder,
    /// `+0x2B`: the message being filled; `+0x24` how many of its packets are new.
    out: [[u8; PACKET]; BATCH],
    n_out: usize,
    /// `+0x28`: "a voice is present" — set when the slot about to be filled held a packet whose
    /// gain byte is above 7 (0x49340A reads the slot **before** 0x493421 copies the new packet
    /// into it, i.e. the previous message's packet at that index), cleared when the message goes.
    /// The object's own code only writes it (0x493209, 0x493413, 0x493447); no reader elsewhere was searched for.
    pub present: bool,
    /// `+4` / `+0xC`: the received packets not yet handed to the stream.
    queue: VecDeque<[u8; PACKET]>,
    play: Option<Playback>,
}

impl Default for VoiceChat {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceChat {
    /// 0x4931C0. The encoder's state is built on the first capture open (0x58ACA0 → 0x597560) and
    /// kept for the process (`[0x6B17DC]` is only freed by 0x5977F0, and rebuilt by the next
    /// 0x597560 — which redoes the window and the filter but not the history); the port keeps
    /// one per voice object.
    pub fn new() -> Self {
        VoiceChat {
            tried: false,
            capture: None,
            enc: Encoder::new(),
            out: [[0; PACKET]; BATCH],
            n_out: 0,
            present: false,
            queue: VecDeque::new(),
            play: None,
        }
    }

    /// **0x493270**: the five packets of a received message join the playback list.
    pub fn receive(&mut self, packets: &[[u8; PACKET]; BATCH]) {
        self.queue.extend(packets.iter().copied());
    }

    /// Packets waiting for the stream (`+0xC`).
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether a playback stream is open (`+0x10`).
    pub fn playing(&self) -> bool {
        self.play.is_some()
    }

    /// **0x493350**, one run. `established` = the five tests of 0x493350 (a session, `+8`,
    /// `+0xBF == 0`, `+0xBE != 0`, `+0xDC >= 0`). Returns the messages' packets to send (each
    /// batch of five is one `0xC1`).
    pub fn tick(&mut self, established: bool, audio: Option<&AudioOutput>) -> Vec<[[u8; PACKET]; BATCH]> {
        let mut send = Vec::new();
        if !established {
            // 0x49339E…: the capture closed (0x561B50) and its buffer freed; nothing else
            if self.tried {
                self.capture = None;
                self.tried = false;
            }
            return send;
        }
        if !self.tried {
            self.tried = true;
            self.capture = capture::Capture::open();
        }
        // 0x4933F2…0x493452: every packet the capture service has encoded since the last run
        let mut pcm = [0i16; FRAME];
        while let Some(c) = &self.capture {
            if !c.read(&mut pcm) {
                break;
            }
            let i = self.n_out;
            if self.out[i][2] > 7 {
                self.present = true;
            }
            self.enc.encode_into(&pcm, &mut self.out[i]);
            self.n_out += 1;
            if self.n_out == BATCH {
                send.push(self.out);
                self.present = false;
                self.n_out = 0;
            }
        }
        self.play_step(audio);
        send
    }

    /// 0x493452…0x493565: the playback half.
    fn play_step(&mut self, audio: Option<&AudioOutput>) {
        if self.queue.is_empty() {
            // 0x493537…0x49355F: stopped once the stream has played everything it was given
            if let Some(p) = &self.play
                && p.src.queued_frames() == 0
            {
                p.stop.stop(p.id);
                self.play = None;
            }
            return;
        }
        if self.play.is_none() {
            if self.queue.len() < START_AT {
                return;
            }
            // 0x493477…0x4934BB: a stream of the format {8000 Hz, mono, code 0x40}
            let Some(a) = audio else {
                // no sound device: the library is closed and 0x5612A0 refuses (−10); the list
                // is still handed over below and dropped (0x5615C0 refuses too)
                self.queue.clear();
                return;
            };
            let src = StreamSource::new(capture::RATE, 1);
            let id = a.play_stream_panned(src.clone(), VOLUME, PAN);
            self.play = Some(Playback { id, stop: a.stop_handle(), src, dec: Decoder::new() });
        }
        // 0x4934D6…0x493533: every queued packet goes to the stream, decoded as it is queued here
        // (the original decodes it when the pump reaches it, 0x59B73E — the same samples)
        let p = self.play.as_mut().expect("opened above");
        let mut f = [0.0f32; FRAME];
        let mut pcm = [0i16; FRAME];
        while let Some(pk) = self.queue.pop_front() {
            p.dec.decode(&pk, &mut f);
            // PORT-DEVIATION D38: the port's stream ring holds 16-bit samples; the original mixes the
            // decoder's float samples (±32767) as they are
            for (o, &v) in pcm.iter_mut().zip(f.iter()) {
                *o = v.round() as i16;
            }
            p.src.push(&pcm);
        }
    }
}

/// The teardown 0x4935C0 (0x48D950, 0x48DBBD, 0x48E74D, 0x48EA41): the stream stops with the object.
impl Drop for VoiceChat {
    fn drop(&mut self) {
        if let Some(p) = self.play.take() {
            p.stop.stop(p.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_runs_until_the_session_is_established() {
        let mut v = VoiceChat::new();
        v.receive(&[[0; PACKET]; BATCH]);
        assert!(v.tick(false, None).is_empty());
        assert_eq!(v.queued(), 5);
        assert!(!v.tried);
    }

    #[test]
    fn the_playback_waits_for_ten_packets_then_takes_them_all() {
        let audio = AudioOutput::offline();
        let mut v = VoiceChat::new();
        // a muted test run opens no microphone
        v.tried = true;
        v.receive(&[[0; PACKET]; BATCH]);
        v.tick(true, Some(&audio));
        assert!(!v.playing());
        assert_eq!(v.queued(), 5);
        v.receive(&[[0; PACKET]; BATCH]);
        v.tick(true, Some(&audio));
        assert!(v.playing());
        assert_eq!(v.queued(), 0);
        assert_eq!(v.play.as_ref().unwrap().src.queued_frames(), 10 * FRAME);
        // once the stream is dry and nothing is queued, it stops
        v.play.as_ref().unwrap().src.drain();
        v.tick(true, Some(&audio));
        assert!(!v.playing());
    }
}
