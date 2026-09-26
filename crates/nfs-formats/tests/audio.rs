//! Audio decoder tests against the original game data (skipped with a message when the
//! installation is absent). Decoded clips are written to /tmp/nfs-audio-check/ for listening.
use nfs_formats::bigf::Bigf;
use nfs_formats::bnk::{decode_ea_xa_frame, Bnk, Codec, EaXaState, PcmSample, PtHeader};
use nfs_formats::schl::{Block, SchlStream};
use std::path::{Path, PathBuf};

fn game_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
        return Some(PathBuf::from(p));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let p = home.join("Games/NFS-Porsche-Unleashed/game_info/data");
    p.join("GameData").is_dir().then_some(p)
}

macro_rules! need_game {
    () => {
        match game_root() {
            Some(r) => r,
            None => {
                eprintln!("skipping: original game data not found");
                return;
            }
        }
    };
}

const OUT_DIR: &str = "/tmp/nfs-audio-check";

/// Minimal RIFF/WAVE PCM16 writer (test-only helper).
fn wav(name: &str, pcm: &PcmSample) -> PathBuf {
    std::fs::create_dir_all(OUT_DIR).unwrap();
    let path = Path::new(OUT_DIR).join(name);
    let ch = pcm.channels as u32;
    let data_len = (pcm.samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(ch as u16).to_le_bytes());
    b.extend_from_slice(&pcm.rate.to_le_bytes());
    b.extend_from_slice(&(pcm.rate * ch * 2).to_le_bytes());
    b.extend_from_slice(&((ch * 2) as u16).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in &pcm.samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&path, b).unwrap();
    eprintln!("wrote {} ({} Hz, {} ch, {} samples/ch)", path.display(), pcm.rate, pcm.channels, pcm.samples_per_channel());
    path
}

fn check_plausible(pcm: &PcmSample, what: &str) {
    // engine banks use arbitrary rates (e.g. 36000 Hz); SFX/speech/music use 11025/16000/22050/32000/44100
    assert!((8000..=48000).contains(&pcm.rate), "{what}: rate {}", pcm.rate);
    assert!((1..=2).contains(&pcm.channels), "{what}: channels {}", pcm.channels);
    let saturated = pcm.samples.iter().filter(|&&s| s.unsigned_abs() >= 32767).count();
    let peak = pcm.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    eprintln!("{what}: peak {peak}, saturated {saturated} of {}", pcm.samples.len());
    assert!((saturated as f64) < 0.001 * pcm.samples.len() as f64, "{what}: {saturated} saturated samples");
    assert!(peak > 1000, "{what}: clip is (near) silent, peak {peak}");
}

#[test]
fn ea_xa_hand_made_frame() {
    // predictor 0, shift 0: sample = nib << 12, high nibble first
    let mut f = [0u8; 15];
    f[0] = 0x00;
    f[1] = 0x1F; // +4096, -4096
    f[2] = 0x70; // +28672, 0
    let mut st = EaXaState::default();
    let mut out = Vec::new();
    decode_ea_xa_frame(&f, &mut st, &mut out);
    assert_eq!(out.len(), 28);
    assert_eq!(&out[..4], &[4096, -4096, 28672, 0]);
    assert!(out[4..].iter().all(|&s| s == 0));
    // shift 2 halves twice: 0x1 -> 1024
    f[0] = 0x02;
    f[1] = 0x10;
    let mut out = Vec::new();
    decode_ea_xa_frame(&f, &mut EaXaState::default(), &mut out);
    assert_eq!(out[0], 1024);
    // predictor 1 (K0 = 0.9375) with zero nibbles decays the seeded history geometrically
    let f = [0x10u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut out = Vec::new();
    decode_ea_xa_frame(&f, &mut EaXaState::seeded(16000, 0), &mut out);
    assert_eq!(out[0], 15000);
    assert_eq!(out[1], (15000.0f64 * 0.9375f64).round() as i16);
    // predictor 2 (K0 1.796875, K1 -0.8125): s = 1.796875*s1 - 0.8125*s2 with s1 = 1000, s2 = 2000
    let f = [0x20u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut out = Vec::new();
    decode_ea_xa_frame(&f, &mut EaXaState::seeded(1000, 2000), &mut out);
    assert_eq!(out[0], (1796.875f64 - 1625.0).round() as i16);
    // saturation of hostile input: predictor 3 with maximal nibbles grows and must clamp, never wrap
    let f = [0x30u8; 15];
    let mut out = Vec::new();
    let mut st = EaXaState::default();
    for _ in 0..8 {
        decode_ea_xa_frame(&f, &mut st, &mut out);
    }
    assert!(out.iter().all(|&s| s == i16::MAX || s == i16::MIN || s.unsigned_abs() < 32767));
}

#[test]
fn pt_header_from_snd_coll_slot0() {
    // bytes of SND_coll.bnk slot 0 at 0x210 (captured from the shipped file)
    let hdr: [u8; 41] = [
        0x50, 0x54, 0x00, 0x00, 0x06, 0x01, 0x56, 0x13, 0x01, 0x7f, 0xfd, 0x80, 0x01, 0x01, 0x85, 0x02, 0x10, 0x25, 0x84, 0x02, 0x3e, 0x80, 0x86, 0x00, 0x87, 0x00, 0x88, 0x04, 0x00, 0x00, 0x27, 0x10, 0x8a, 0x04,
        0x00, 0x00, 0x00, 0x00, 0xff, 0xAA, 0xBB,
    ];
    let h = PtHeader::parse(&hdr, 0).unwrap();
    assert_eq!(h.len, 39);
    assert_eq!(h.platform, 0);
    assert_eq!(h.patch_params, vec![(0x06, 0x56), (0x13, 0x7f)]);
    assert!(h.saw_fd);
    assert_eq!(h.split, Some(1));
    assert_eq!(h.sample_count, Some(0x1025));
    assert_eq!(h.sample_rate, 16000);
    assert_eq!(h.channels, 1);
    assert_eq!((h.loop_start, h.loop_end), (Some(0), Some(0)));
    assert_eq!(h.loop_range(), None);
    assert_eq!(h.data_offset, [Some(0x2710), None]);
    assert_eq!(h.unknown_8a, [Some(0), None]);
    assert_eq!(h.codec().unwrap(), Codec::EaXa);
    // speech-style header: no 0x80, class 7 => EA-XA; 3-byte values
    let sp = [0x50, 0x54, 0, 0, 0x06, 0x01, 0x64, 0xfd, 0x83, 0x01, 0x07, 0x84, 0x02, 0x2b, 0x11, 0x85, 0x03, 0x00, 0x25, 0x60, 0x88, 0x04, 0, 0, 0, 0x38, 0xff];
    let h = PtHeader::parse(&sp, 0).unwrap();
    assert_eq!(h.codec().unwrap(), Codec::EaXa);
    assert_eq!(h.sample_rate, 11025);
    assert_eq!(h.sample_count, Some(0x2560));
    // truncated header is an explicit error
    assert!(matches!(PtHeader::parse(&sp[..20], 0), Err(nfs_formats::FormatError::Truncated { .. })));
}

#[test]
fn snd_coll_bank() {
    let r = need_game!();
    let d = std::fs::read(r.join("GameData/Sounds/SND_coll.bnk")).unwrap();
    let b = Bnk::parse(&d).unwrap();
    assert_eq!(b.slot_count(), 127);
    assert_eq!(b.used_slots().count(), 111);
    assert_eq!(b.dir_size as usize + b.data_size as usize, d.len());
    let slot = if b.header(0).is_some() { 0 } else { b.used_slots().next().unwrap() };
    let h = b.header(slot).unwrap().clone();
    let pcm = b.decode_slot(&d, slot).unwrap();
    assert_eq!(pcm.samples_per_channel(), h.sample_count.unwrap() as usize);
    assert_eq!(pcm.padded_tail, 0);
    assert_eq!(pcm.rate, 16000);
    check_plausible(&pcm, "SND_coll slot 0");
    wav(&format!("SND_coll_slot{slot}.wav"), &pcm);
    // every used slot decodes with the declared sample count
    for i in b.used_slots() {
        let p = b.decode_slot(&d, i).unwrap_or_else(|e| panic!("slot {i}: {e}"));
        assert_eq!(p.samples_per_channel(), b.header(i).unwrap().sample_count.unwrap() as usize, "slot {i}");
        assert_eq!(p.padded_tail, 0, "slot {i}");
    }
    assert!(matches!(b.decode_slot(&d, 200), Err(nfs_formats::FormatError::UnexpectedValue { .. })));
}

#[test]
fn speech_bank() {
    let r = need_game!();
    let v = std::fs::read(r.join("GameData/Speech/zzzxeng.viv")).unwrap();
    let big = Bigf::parse(&v).unwrap();
    assert_eq!(big.entries.len(), 161);
    let mut padded_banks = 0;
    let mut banks = 0;
    let mut first: Option<(String, PcmSample)> = None;
    for e in &big.entries {
        if !e.name.ends_with(".bnk") || e.size < 20 {
            continue;
        }
        let d = big.data(&v, e);
        let b = Bnk::parse(d).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert_eq!(b.slot_count(), 1, "{}", e.name);
        let h = b.header(0).unwrap();
        assert_eq!(h.codec().unwrap(), Codec::EaXa);
        assert_eq!(h.codec_class, Some(7));
        assert_eq!(h.sample_rate, 11025);
        let pcm = b.decode_slot(d, 0).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert_eq!(pcm.samples_per_channel(), h.sample_count.unwrap() as usize, "{}", e.name);
        assert!(pcm.padded_tail < 28, "{}", e.name);
        banks += 1;
        if pcm.padded_tail > 0 {
            padded_banks += 1;
        }
        if first.is_none() {
            first = Some((e.name.clone(), pcm));
        }
    }
    let (name, pcm) = first.unwrap();
    eprintln!("speech: {banks} banks decoded, {padded_banks} with a missing trailing partial frame (padded)");
    assert_eq!(banks, 154);
    assert_eq!(pcm.rate, 11025);
    check_plausible(&pcm, &format!("speech {name}"));
    wav(&format!("speech_{}.wav", name.trim_end_matches(".bnk")), &pcm);
}

#[test]
fn engine_bank_sd6a() {
    let r = need_game!();
    let v = std::fs::read(r.join("GameData/Sounds/sd6a.viv")).unwrap();
    let big = Bigf::parse(&v).unwrap();
    let d = big.get(&v, "sd6a.bnk").expect("sd6a.bnk entry");
    let b = Bnk::parse(d).unwrap();
    assert_eq!(b.slot_count(), 116);
    assert_eq!(b.used_slots().count(), 34);
    assert_eq!(b.dir_size as usize + b.data_size as usize, d.len());
    let mut looped = 0;
    let mut pcm16 = 0;
    for i in b.used_slots() {
        let h = b.header(i).unwrap();
        let pcm = b.decode_slot(d, i).unwrap_or_else(|e| panic!("slot {i}: {e}"));
        assert_eq!(pcm.samples_per_channel(), h.sample_count.unwrap() as usize, "slot {i}");
        assert_eq!(pcm.padded_tail, 0, "slot {i}");
        check_plausible(&pcm, &format!("sd6a slot {i}"));
        if let Some((s, e)) = pcm.loop_range {
            looped += 1;
            assert!(s < e && e as usize <= pcm.samples_per_channel(), "slot {i} loop {s}..{e}");
        }
        if h.codec().unwrap() == Codec::Pcm16 {
            pcm16 += 1;
        }
        if i == 2 || i == 82 {
            wav(&format!("sd6a_slot{i}_{:?}.wav", h.codec().unwrap()), &pcm);
        }
    }
    eprintln!("sd6a: {looped} looped slots, {pcm16} PCM16 slots");
    assert_eq!(looped, 27);
    assert!(pcm16 >= 1);
    // slot 2: 17605 EA-XA samples (629 frames) per the notes
    assert_eq!(b.header(2).unwrap().sample_count, Some(17605));
}

#[test]
fn engine_bank_sd6a16_pcm8() {
    let r = need_game!();
    let v = std::fs::read(r.join("GameData/Sounds/sd6a16.viv")).unwrap();
    let big = Bigf::parse(&v).unwrap();
    let d = big.get(&v, "sd6a16.bnk").expect("sd6a16.bnk entry");
    let b = Bnk::parse(d).unwrap();
    let i = b.used_slots().next().unwrap();
    let h = b.header(i).unwrap();
    assert_eq!(h.codec().unwrap(), Codec::Pcm8);
    assert_eq!(h.sample_rate, 16000);
    let pcm = b.decode_slot(d, i).unwrap();
    assert_eq!(pcm.samples_per_channel(), h.sample_count.unwrap() as usize);
    check_plausible(&pcm, &format!("sd6a16 slot {i}"));
    wav(&format!("sd6a16_slot{i}_Pcm8.wav"), &pcm);
}

#[test]
fn music_stream_first_two_seconds() {
    let r = need_game!();
    let v = std::fs::read(r.join("GameData/Music/zzzymus.viv")).unwrap();
    let big = Bigf::parse(&v).unwrap();
    assert_eq!(big.entries.len(), 28);
    let e = &big.entries[0];
    let d = big.data(&v, e);
    let s = SchlStream::parse(d).unwrap();
    assert_eq!(s.header.channels, 2);
    assert_eq!(s.header.sample_rate, 32000);
    assert_eq!(s.header.codec().unwrap(), Codec::EaXa);
    let total = s.header.sample_count.unwrap() as usize;
    let declared = s.declared_blocks.expect("SCCl");
    // stream the first 2 seconds
    let want = 2 * s.header.sample_rate as usize;
    let mut samples = Vec::new();
    let mut blocks = 0;
    for b in s.blocks(d) {
        match b.unwrap() {
            Block::Data(db) => {
                db.decode_into(&mut samples).unwrap();
                blocks += 1;
                if samples.len() / 2 >= want {
                    break;
                }
            }
            Block::End => break,
            other => panic!("unexpected block {other:?}"),
        }
    }
    samples.truncate(want * 2);
    let pcm = PcmSample { rate: s.header.sample_rate, channels: 2, samples, loop_range: None, padded_tail: 0, gain: 1.0, bend_cents: 0, fx_send: 0, priority: 0, pan: 64, pan_scale: 1, records: Vec::new(), parts: Vec::new(), tables: Vec::new() };
    assert_eq!(pcm.samples_per_channel(), want);
    check_plausible(&pcm, &format!("music {} ({blocks} blocks)", e.name));
    wav(&format!("music_{}_2s.wav", e.name.split('-').next().unwrap()), &pcm);
    // walk the whole stream: block count and total sample count must match SCCl / PT 0x85
    let mut per_channel = 0usize;
    let mut n = 0u32;
    let mut ended = false;
    for b in s.blocks(d) {
        match b.unwrap() {
            Block::Data(db) => {
                per_channel += db.samples_per_channel as usize;
                n += 1;
            }
            Block::End => ended = true,
            other => panic!("unexpected block {other:?}"),
        }
    }
    assert!(ended);
    assert_eq!(n, declared);
    assert_eq!(per_channel, total);
}

#[test]
fn music_stream_decode_all_short_track() {
    let r = need_game!();
    let v = std::fs::read(r.join("GameData/Music/zzzymus.viv")).unwrap();
    let big = Bigf::parse(&v).unwrap();
    // the shortest stream (vic01, 258 blocks) keeps the full decode quick
    let e = big.entries.iter().min_by_key(|e| e.size).unwrap();
    let d = big.data(&v, e);
    let pcm = nfs_formats::schl::decode_all(d).unwrap();
    let s = SchlStream::parse(d).unwrap();
    assert_eq!(pcm.samples_per_channel(), s.header.sample_count.unwrap() as usize);
    check_plausible(&pcm, &format!("music {} (full)", e.name));
    wav(&format!("music_{}_full.wav", e.name.split('-').next().unwrap()), &pcm);
}

#[test]
fn fesfx_planar_stereo() {
    let r = need_game!();
    let d = std::fs::read(r.join("GameData/Sounds/fesfx.bnk")).unwrap();
    let b = Bnk::parse(&d).unwrap();
    assert_eq!(b.slot_count(), 54);
    let stereo: Vec<usize> = b.used_slots().filter(|&i| b.header(i).unwrap().channels == 2).collect();
    assert_eq!(stereo.len(), 9);
    let i = stereo[0];
    let h = b.header(i).unwrap();
    // planar: channel 1 starts right after channel 0's frames (0x89 = 0x88 + frames*15)
    let frames = (h.sample_count.unwrap() as usize).div_ceil(28);
    assert_eq!(h.data_offset[1].unwrap() as usize, h.data_offset[0].unwrap() as usize + frames * 15);
    let pcm = b.decode_slot(&d, i).unwrap();
    assert_eq!(pcm.channels, 2);
    assert_eq!(pcm.samples.len(), 2 * h.sample_count.unwrap() as usize);
    check_plausible(&pcm, &format!("fesfx stereo slot {i}"));
    wav(&format!("fesfx_slot{i}_stereo.wav"), &pcm);
}

#[test]
fn collision_bank_slots() {
    use nfs_formats::bnk::Bnk;
    let Some(root) = game_root() else { eprintln!("skipping"); return };
    let data = std::fs::read(root.join("GameData/Sounds/SND_coll.bnk")).unwrap();
    let bank = Bnk::parse(&data).unwrap();
    assert_eq!(bank.slot_count(), 127);
    // loops 0x28..=0x2F carry loop points; one-shots 0x30..=0x7E do not (RE/notes/sfx.md §3)
    for slot in [0x28usize, 0x2A, 0x2D] {
        let pcm = bank.decode_slot(&data, slot).unwrap();
        assert!(pcm.loop_range.is_some(), "slot {slot:#x} should loop");
        assert!(pcm.rate >= 11025);
    }
    for slot in [0x20usize, 0x30, 0x3C, 0x40, 0x60, 0x6C, 0x7C] {
        let pcm = bank.decode_slot(&data, slot).unwrap();
        assert!(pcm.loop_range.is_none(), "slot {slot:#x} is a one-shot");
        assert!(pcm.samples.len() > 1000, "slot {slot:#x} has {} samples", pcm.samples.len());
    }
    // 48 slots of more than one record (91 with LilSND_coll's 43), each record decoded as a
    // sample of its own (0x599100 per record)
    let mut multi = 0;
    for slot in bank.used_slots() {
        let pcm = bank.decode_slot(&data, slot).unwrap();
        if pcm.parts.is_empty() {
            continue;
        }
        multi += 1;
        assert_eq!(pcm.parts.len(), pcm.records.len());
        for (k, part) in pcm.parts.iter().enumerate() {
            assert_eq!(part.records, vec![pcm.records[k]]);
            assert!(part.samples.len() > 100, "slot {slot:#x} record {k}");
        }
    }
    assert_eq!(multi, 48);
    // the wind slot (the header at 0x454, two records): the envelope and the volume step table
    // at their self-relative pointers (codes 0x19 / 0x1D), code 0x09 segments and code 0x1E steps
    let wind = bank.slots.iter().flatten().find(|s| s.header_offset == 0x454).unwrap();
    let t = &wind.header.tables[0];
    assert_eq!(t.envelope, [(8, 50), (8, 120), (0x7FFF_FFFF, 0), (244, 0)]);
    let v = t.volume.as_ref().unwrap();
    assert_eq!((v.len(), &v[..8]), (100, &[0, 0, 0, 0, 47, 51, 56, 63][..]));
    assert!(t.pitch.is_none() && t.curve.is_none());
    assert_eq!(wind.header.patch_records()[0].env_level, 0);
    assert_eq!(wind.header.tables[1].volume.as_ref().unwrap().len(), 128);
    // a record without code 0x19 holds the image's default at 0x5E3498
    let plain = bank.header(0x30).unwrap();
    assert_eq!(plain.tables[0].envelope, [(0x7FFF_FFFF, 127)]);
}

/// The PT patch codes of `RE/notes/audio.md` §2: the engine bank gives nearly every sample its own
/// volume and root key, while the track ambience banks carry neither and take the parser's defaults.
#[test]
fn pt_patch_parameters() {
    let r = need_game!();
    let v = std::fs::read(r.join("GameData/Sounds/sd6a.viv")).unwrap();
    let big = Bigf::parse(&v).unwrap();
    let d = big.get(&v, "sd6a.bnk").expect("sd6a.bnk entry");
    let b = Bnk::parse(d).unwrap();
    let mut with_volume = 0;
    for i in b.used_slots() {
        let h = b.header(i).unwrap();
        let vol = h.patch_volume();
        assert!(vol <= 127, "slot {i} volume {vol}");
        if h.patch_params.iter().any(|(c, _)| *c == 0x0E) {
            with_volume += 1;
            assert!((34..=127).contains(&vol), "slot {i} volume {vol}");
        }
        // every engine sample is tuned, and the root keys stay inside the MIDI range
        assert!((24..=96).contains(&h.root_key()), "slot {i} root {}", h.root_key());
        assert_eq!(h.patch_pan(), 64, "slot {i} is not centred");
        assert_eq!(b.decode_slot(d, i).unwrap().gain, vol as f32 / 127.0);
    }
    assert_eq!(with_volume, 33, "33 of the 34 sd6a slots carry code 0x0E");

    // an ambience bank has no patch parameters at all, so it plays at full volume, centred
    let t = std::fs::read(r.join("GameData/Sounds/track.viv")).unwrap();
    let dir = Bigf::parse(&t).unwrap();
    let amb = dir.get(&t, "bang1_001.bnk").expect("bang1_001.bnk");
    let ab = Bnk::parse(amb).unwrap();
    let h = ab.header(ab.used_slots().next().unwrap()).unwrap();
    assert!(!h.patch_params.iter().any(|(c, _)| matches!(c, 0x0C..=0x0F)), "ambience banks set no volume or pan");
    assert_eq!((h.patch_volume(), h.patch_pan(), h.root_key()), (127, 64, 60));
}
