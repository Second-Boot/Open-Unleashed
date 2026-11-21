use super::*;

const fn e(token: &'static str, levels: &'static [[&'static str; 2]], default_level: u8) -> Entry {
    Entry { token, name: ["n", "н"], help: ["h", "п"], levels, default_level, platforms: Platforms::All }
}
const L4: &[[&str; 2]] = &[["off", "в"], ["a", "а"], ["b", "б"], ["c", "в"]];
const TEST: &[Entry] = &[e("E1", ON_OFF, 1), e("E2", L4, 2), e("E3", L4, 3)];

/// The semantic level names point at the captions they are named after.
#[test]
fn the_level_names_match_the_captions() {
    let cap = |s: Switch, l: u8| REGISTRY[s.0 as usize].levels[l as usize][0];
    assert_eq!(cap(E1, Enhancements::E1_2X), "2x");
    assert!(cap(E6, Enhancements::E6_MAILBOX).starts_with("mailbox"));
    assert!(cap(E6, Enhancements::E6_IMMEDIATE).starts_with("immediate"));
    assert_eq!(cap(E16, Enhancements::E16_STRONGER), "stronger");
    assert_eq!(cap(E17, Enhancements::E17_STRONGER), "stronger");
    for (k, fps) in Enhancements::E22_FPS.iter().enumerate() {
        assert_eq!(cap(E22, k as u8 + 1), fps.to_string());
    }
    assert_eq!(cap(E22, Enhancements::E22_DISPLAY), "display");
    assert!(cap(E22, Enhancements::E22_AUTO).starts_with("auto"));
    assert_eq!(cap(E51, Enhancements::E51_WHERE_SUN), "where the sun reaches");
    assert_eq!(cap(E51, Enhancements::E51_ALWAYS), "always");
    // every switch constant is the entry of its own token
    for (i, e) in REGISTRY.iter().enumerate() {
        assert_eq!(index_of(e.token) as usize, i);
    }
}

fn lv(s: &Enhancements) -> (u8, u8, u8) {
    (s.level(Switch(0)), s.level(Switch(1)), s.level(Switch(2)))
}

#[test]
fn original_has_nothing_on_and_all_is_every_default() {
    let o = Enhancements::ORIGINAL;
    assert!(!o.any_on());
    let a = Enhancements::all_in(TEST);
    assert_eq!(lv(&a), (1, 2, 3));
    assert!((0..3).all(|i| a.is_on(Switch(i))));
    assert!(!a.is_on(Switch(3)) && a.level(Switch(200)) == 0);
    assert_eq!(a.describe_in(TEST), "all");
    assert_eq!(o.describe_in(TEST), "off");
    assert_eq!(Enhancements::default(), o);
}

#[test]
fn set_level_and_is_on() {
    let mut s = Enhancements::ORIGINAL;
    s.set_level(Switch(1), 3);
    assert!(s.is_on(Switch(1)) && s.any_on() && s.level(Switch(1)) == 3);
    s.set_level(Switch(1), 0);
    assert!(!s.is_on(Switch(1)) && !s.any_on() && s == Enhancements::ORIGINAL);
    s.set_level(Switch(99), 1); // out of range: ignored
    assert_eq!(s, Enhancements::ORIGINAL);
}

#[test]
fn the_empty_registry_has_all_equal_original() {
    assert_eq!(Enhancements::all_in(&[]), Enhancements::ORIGINAL);
    assert_eq!(Enhancements::all_in(&[]).describe_in(&[]), "off");
}

#[test]
fn override_forms() {
    let mut unk = Vec::new();
    assert_eq!(Enhancements::parse_override_in(TEST, "off", &mut unk), Some(Enhancements::ORIGINAL));
    assert_eq!(Enhancements::parse_override_in(TEST, "ALL", &mut unk), Some(Enhancements::all_in(TEST)));
    // a bare token = its default level, the rest off
    let l = Enhancements::parse_override_in(TEST, "E1, e3,E9", &mut unk).unwrap();
    assert_eq!(lv(&l), (1, 0, 3));
    assert_eq!(unk, vec!["E9".to_string()]);
    assert_eq!(l.describe_in(TEST), "E1,E3");
    // explicit levels, 0 = off, out of range / not a number skipped
    unk.clear();
    let x = Enhancements::parse_override_in(TEST, "E1=0,E2=1,e3=2, E2=9, E1=x", &mut unk).unwrap();
    assert_eq!(lv(&x), (0, 1, 2));
    assert_eq!(unk, vec!["E2=9".to_string(), "E1=x".to_string()]);
    assert_eq!(x.describe_in(TEST), "E2=1,E3=2");
    assert_eq!(Enhancements::parse_override_in(TEST, "", &mut unk), None);
}

#[test]
fn lite_preset_lists_the_given_levels_and_nothing_else() {
    // ENH-LITE: a listed token at its level (clamped), every other entry off, an unknown token skipped
    let l = Enhancements::lite_in(TEST, [("E2", 1), ("E3", 9), ("Enope", 1)]);
    assert_eq!(lv(&l), (0, 1, 3));
    assert_eq!(Enhancements::lite_in(TEST, []), Enhancements::ORIGINAL);
}

#[test]
fn the_real_lite_preset_is_valid_and_parses() {
    for (sw, lvl) in LITE_PRESET {
        let e = &REGISTRY[sw.0 as usize];
        assert!((*lvl as usize) < e.levels.len() && *lvl > 0, "{}", e.token);
    }
    let lite = Enhancements::lite();
    assert!(lite.any_on() && lite != Enhancements::all());
    // no entry above its default level in the light preset (light = less than the defaults)
    for (i, e) in REGISTRY.iter().enumerate() {
        assert!(lite.levels()[i] <= e.default_level.max(1), "{}", e.token);
    }
    let mut unk = Vec::new();
    assert_eq!(Enhancements::parse_override("lite", &mut unk), Some(lite));
    assert_eq!(Enhancements::parse_override("LIGHT", &mut unk), Some(lite));
    assert_eq!(Enhancements::lite().describe(), "lite");
    // `lite,E14=1`: the preset, then the override
    let x = Enhancements::parse_override("lite,E14=1", &mut unk).unwrap();
    assert_eq!(x.level(E14), 1);
    assert_eq!(x.level(E1), lite.level(E1));
    assert!(unk.is_empty());
    // the light preset sets nothing on a platform without the entry's backend
    assert_eq!(lite.level(E33) + lite.level(E34), 0);
}

#[test]
fn gpu_class_of_an_adapter() {
    use wgpu::{Backend::*, DeviceType::*};
    assert_eq!(classify_gpu(DiscreteGpu, Vulkan, "NVIDIA GeForce RTX 4060"), GpuClass::Strong);
    assert_eq!(classify_gpu(IntegratedGpu, Vulkan, "Intel(R) UHD Graphics (ICL GT1)"), GpuClass::Weak);
    assert_eq!(classify_gpu(IntegratedGpu, Dx12, "AMD Radeon(TM) Graphics"), GpuClass::Weak);
    assert_eq!(classify_gpu(Cpu, Vulkan, "llvmpipe (LLVM 15.0.6, 256 bits)"), GpuClass::Weak);
    // Apple Silicon: integrated but strong; an Intel Mac's integrated GPU is not
    assert_eq!(classify_gpu(IntegratedGpu, Metal, "Apple M1 Pro"), GpuClass::Strong);
    assert_eq!(classify_gpu(IntegratedGpu, Metal, "Intel(R) Iris(TM) Plus Graphics"), GpuClass::Weak);
    assert_eq!(classify_gpu(Other, Gl, "unknown"), GpuClass::Strong);
}

#[test]
fn describe_parses_back() {
    let mut unk = Vec::new();
    for s in ["off", "all", "E1", "E2=1,E3", "E1,E2=3,E3=1"] {
        let e = Enhancements::parse_override_in(TEST, s, &mut unk).unwrap();
        assert_eq!(Enhancements::parse_override_in(TEST, &e.describe_in(TEST), &mut unk), Some(e), "{s}");
    }
    assert!(unk.is_empty());
}

#[test]
fn stored_round_trip_and_defaults() {
    let mut s = Enhancements::all_in(TEST);
    s.set_level(Switch(1), 0);
    s.set_level(Switch(2), 1);
    let pairs = s.pairs_in(TEST);
    assert_eq!(pairs, vec![("E1", 1), ("E2", 0), ("E3", 1)]);
    assert_eq!(Enhancements::from_stored_in(TEST, pairs.into_iter().map(|(k, l)| (k, Stored::Level(l)))), s);
    // an older file that knows only E1 (off): E2 and E3, new since, are at their defaults
    let old = Enhancements::from_stored_in(TEST, [("E1", Stored::Bool(false)), ("Eold", Stored::Bool(false))]);
    assert_eq!(lv(&old), (0, 2, 3));
    // old booleans: true = default level, false = off
    let b = Enhancements::from_stored_in(TEST, [("E1", Stored::Bool(true)), ("E2", Stored::Bool(true)), ("E3", Stored::Bool(false))]);
    assert_eq!(lv(&b), (1, 2, 0));
    // a level past the entry's choices is clamped
    let c = Enhancements::from_stored_in(TEST, [("E1", Stored::Level(7))]);
    assert_eq!(c.level(Switch(0)), 1);
}

#[test]
fn the_real_registry_is_well_formed() {
    for (i, e) in REGISTRY.iter().enumerate() {
        assert!(e.levels.len() >= 2 && e.levels.len() <= MAX_LEVELS, "{}", e.token);
        assert!((e.default_level as usize) < e.levels.len(), "{}", e.token);
        assert!(REGISTRY[..i].iter().all(|o| o.token != e.token));
    }
    assert!(REGISTRY.len() <= MAX_SWITCHES);
    assert_eq!(REGISTRY[E1.0 as usize].token, "E1");
    assert_eq!(REGISTRY[E6.0 as usize].token, "E6");
    assert_eq!(REGISTRY[E1.0 as usize].default_level, 2); // MSAA 4x
    assert_eq!(REGISTRY[E2.0 as usize].default_level, 3); // 16x
    assert!([E3, E4, E6].iter().all(|s| REGISTRY[s.0 as usize].default_level == 0));
    assert_eq!(REGISTRY[E5.0 as usize].default_level, 2); // draw distance: +50 % (2026-10-03)
    assert_eq!(REGISTRY[E7.0 as usize].token, "E7");
    assert_eq!(REGISTRY[E7.0 as usize].default_level, 1); // as the original
    assert_eq!(REGISTRY[E8.0 as usize].token, "E8");
    assert_eq!(REGISTRY[E8.0 as usize].default_level, 2); // clear coat: normal (ENH-COAT)
    assert_eq!(REGISTRY[E9.0 as usize].token, "E9");
    assert_eq!(REGISTRY[E12.0 as usize].token, "E12");
    assert_eq!(REGISTRY[E12.0 as usize].default_level, 1); // soft particles: on
    assert_eq!(REGISTRY[E10.0 as usize].token, "E10");
    assert_eq!(REGISTRY[E10.0 as usize].default_level, 1); // bloom: light
    assert_eq!(REGISTRY[E11.0 as usize].token, "E11");
    assert_eq!(REGISTRY[E11.0 as usize].default_level, 1); // bright sun: on
    assert_eq!(REGISTRY[E16.0 as usize].token, "E16");
    assert_eq!(REGISTRY[E15.0 as usize].token, "E15");
    assert_eq!(REGISTRY[E15.0 as usize].default_level, 2); // headlights: with shadows
    assert_eq!(REGISTRY[E14.0 as usize].token, "E14");
    assert_eq!(REGISTRY[E14.0 as usize].default_level, 1); // AO: weak
    assert_eq!(REGISTRY[E18.0 as usize].token, "E18");
    assert_eq!(REGISTRY[E18.0 as usize].default_level, 1); // particles in HDR: on
    assert_eq!(REGISTRY[E19.0 as usize].token, "E19");
    assert_eq!(REGISTRY[E19.0 as usize].default_level, 1); // fog and sky: on
    // 2026-11-13: E20 (lightning lights the scene) removed; an old "E20" is unknown
    assert!(!REGISTRY.iter().any(|e| e.token == "E20"));
    let mut unknown = Vec::new();
    assert_eq!(Enhancements::parse_override("E20,E21=1", &mut unknown).map(|e| e.level(E21)), Some(1));
    assert_eq!(unknown, vec!["E20".to_string()]);
    assert_eq!(REGISTRY[E21.0 as usize].token, "E21");
    assert_eq!(REGISTRY[E21.0 as usize].default_level, 1); // eye adaptation: on (2026-10-03)
    assert_eq!(REGISTRY[E23.0 as usize].token, "E23");
    assert_eq!(REGISTRY[E23.0 as usize].default_level, 2); // street lamps: normal
    assert_eq!(REGISTRY[E24.0 as usize].token, "E24");
    assert_eq!(REGISTRY[E24.0 as usize].default_level, 1); // headlights in tunnels: on
    // 2026-11-16: E25 (soft shadows when overcast) removed; an old "E25" is unknown
    assert!(!REGISTRY.iter().any(|e| e.token == "E25"));
    let mut unknown = Vec::new();
    assert_eq!(Enhancements::parse_override("E25=1,E22=2", &mut unknown).map(|e| e.level(E22)), Some(2));
    assert_eq!(unknown, vec!["E25=1".to_string()]);
    assert_eq!(REGISTRY[E22.0 as usize].token, "E22");
    assert_eq!(REGISTRY[E22.0 as usize].default_level, 6); // frame rate limit: auto (2026-10-04)
    assert_eq!(REGISTRY[E27.0 as usize].token, "E27");
    assert_eq!(REGISTRY[E27.0 as usize].default_level, 0); // motion blur: off (taste)
    assert_eq!(REGISTRY[E28.0 as usize].token, "E28");
    assert_eq!(REGISTRY[E29.0 as usize].token, "E29");
    assert_eq!(REGISTRY[E29.0 as usize].default_level, 1); // camera shake: light
    assert_eq!(REGISTRY[E26.0 as usize].token, "E26");
    assert_eq!(REGISTRY[E26.0 as usize].default_level, 1); // wet road and rain: on
    // 2026-10-27: photo mode is always on (master's PORT-DEVIATION D46), no switch; an old "E30" is unknown
    assert!(!REGISTRY.iter().any(|e| e.token == "E30"));
    let mut unknown = Vec::new();
    assert_eq!(Enhancements::parse_override("E30,E31=1", &mut unknown).map(|e| (e.level(E31), e.any_on())), Some((1, true)));
    assert_eq!(unknown, vec!["E30".to_string()]);
    assert_eq!(REGISTRY[E31.0 as usize].token, "E31");
    assert_eq!(REGISTRY[E31.0 as usize].default_level, 0); // depth of field in replays: off
    assert_eq!(REGISTRY[E32.0 as usize].token, "E32");
    assert_eq!(REGISTRY[E32.0 as usize].default_level, 2); // global illumination: medium (2026-10-04)
    assert_eq!(REGISTRY[E35.0 as usize].token, "E35");
    assert_eq!(REGISTRY[E35.0 as usize].default_level, 0); // SMAA: off (cost vs a barely visible gain)
    assert_eq!(REGISTRY[E36.0 as usize].token, "E36");
    assert_eq!(REGISTRY[E36.0 as usize].default_level, 2); // haze and beams: medium (2026-10-04)
    // ENHANCEMENT E34: on every platform (indices stable), off by default, hidden / forced off where it has no backend
    assert_eq!(REGISTRY[E34.0 as usize].token, "E34");
    assert_eq!(REGISTRY[E34.0 as usize].default_level, 0);
    assert_eq!(REGISTRY[E34.0 as usize].levels.len(), 8);
    assert_eq!(REGISTRY[E34.0 as usize].levels.len() as u8, 4 + E34_TEMPORAL_LEVELS);
    assert_eq!(REGISTRY[E34.0 as usize].available(), cfg!(target_os = "macos"));
    let mut u = Vec::new();
    let set = Enhancements::parse_override("all,E34=2", &mut u).unwrap();
    assert_eq!(set.level(E34), if cfg!(target_os = "macos") { 2 } else { 0 });
    assert_eq!(u.is_empty(), cfg!(target_os = "macos"));
    assert_eq!(REGISTRY[E33.0 as usize].token, "E33");
    assert_eq!(REGISTRY[E33.0 as usize].default_level, 0); // HDR display: off until judged on the real screen
    assert_eq!(REGISTRY[E33.0 as usize].available(), cfg!(target_os = "macos"));
    // ENHANCEMENT E37: off by default, five levels (off, AA only, quality, balanced, performance); on every platform since ENH-UPSCALER
    assert_eq!(REGISTRY[E37.0 as usize].token, "E37");
    assert_eq!(REGISTRY[E37.0 as usize].default_level, 0);
    assert_eq!(REGISTRY[E37.0 as usize].levels.len(), 5);
    assert!(REGISTRY[E37.0 as usize].available());
    // ENHANCEMENT ENH-UPSCALER: E38 / E39 follow E37; E34 / E37 keep their levels (every index after 28 moved down by one when
    // E30 left the registry 2026-10-27, every index after 18 when E20 left 2026-11-13, every index after 21 when E25 left
    // 2026-11-16: nothing stores an index)
    assert_eq!((E34.0, E37.0, E38.0, E39.0), (31, 33, 34, 35));
    assert_eq!(REGISTRY[E38.0 as usize].token, "E38");
    assert_eq!(REGISTRY[E39.0 as usize].token, "E39");
    assert_eq!(E52.0 as usize, REGISTRY.len() - 1); // ENHANCEMENT E52 is the last entry
    assert_eq!(REGISTRY[E52.0 as usize].token, "E52");
    assert_eq!(REGISTRY[E52.0 as usize].default_level, 1); // the picture wall without seams: on
    assert_eq!(REGISTRY[E52.0 as usize].levels.len(), 2);
    assert_eq!((E39.0, E40.0, E41.0, E42.0, E43.0, E44.0, E47.0), (35, 36, 37, 38, 39, 40, 41));
    // ENHANCEMENT E48: the steady slope lift of the chase cameras, index 43, on by default, off / on
    assert_eq!(E48.0, 42);
    assert_eq!(REGISTRY[E48.0 as usize].token, "E48");
    assert_eq!(REGISTRY[E48.0 as usize].default_level, 1);
    assert_eq!(REGISTRY[E48.0 as usize].levels.len(), 2);
    assert_eq!((E50.0, E51.0), (44, 45));
    // ENHANCEMENT E49: reflections of nearby objects, index 44, on by default; needs E8 (its cube)
    assert_eq!(E49.0, 43);
    assert_eq!(REGISTRY[E49.0 as usize].token, "E49");
    assert_eq!(REGISTRY[E49.0 as usize].default_level, 1);
    assert_eq!(REGISTRY[E49.0 as usize].levels.len(), 2);
    let mut no_e8 = Enhancements::all();
    no_e8.set_level(E8, 0);
    assert_eq!(no_e8.resolved().level(E49), 0);
    assert!(no_e8.blocked("E49").is_some());
    assert_eq!(REGISTRY[E50.0 as usize].token, "E50");
    assert_eq!(REGISTRY[E50.0 as usize].default_level, 2); // moonlight: normal
    assert_eq!(REGISTRY[E50.0 as usize].levels.len(), 3);
    assert_eq!(REGISTRY[E51.0 as usize].token, "E51");
    assert_eq!(REGISTRY[E51.0 as usize].default_level, 1); // the cars' shadow on dim roads: where the sun reaches the car
    assert_eq!(REGISTRY[E51.0 as usize].levels.len(), 3); // ALPS-CARSH: off / where the sun reaches the car / always
    assert_eq!(REGISTRY[E47.0 as usize].token, "E47");
    assert_eq!(REGISTRY[E47.0 as usize].default_level, 1); // lamp shadows: on
    assert_eq!(REGISTRY[E47.0 as usize].levels.len(), 2);
    assert_eq!(REGISTRY[E44.0 as usize].token, "E44");
    assert_eq!(REGISTRY[E44.0 as usize].default_level, 1); // lighthouse beam: on
    assert_eq!(REGISTRY[E44.0 as usize].levels.len(), 2);
    assert_eq!(REGISTRY[E43.0 as usize].token, "E43");
    assert_eq!(REGISTRY[E43.0 as usize].default_level, 1); // contact shadow: on
    assert_eq!(REGISTRY[E43.0 as usize].levels.len(), 2);
    assert_eq!(REGISTRY[E42.0 as usize].token, "E42");
    assert_eq!(REGISTRY[E42.0 as usize].default_level, 1); // rounded details: soft
    assert_eq!(REGISTRY[E42.0 as usize].levels.len(), 3);
    assert_eq!(REGISTRY[E41.0 as usize].token, "E41");
    assert_eq!(REGISTRY[E41.0 as usize].default_level, 1); // textures: on where a pack is found; off in the light preset
    assert_eq!(REGISTRY[E41.0 as usize].levels.len(), 2);
    assert_eq!(REGISTRY[E40.0 as usize].token, "E40");
    assert_eq!(REGISTRY[E40.0 as usize].default_level, 1); // raindrops: on, subtle
    assert_eq!(REGISTRY[E38.0 as usize].default_level, 0); // the upscaler is off by default
    assert_eq!(REGISTRY[E38.0 as usize].levels.len(), 3);
    assert_eq!(REGISTRY[E39.0 as usize].levels.len(), 5);
    assert_eq!(fsr_sharpness(REGISTRY[E39.0 as usize].default_level), Some(0.3)); // the old constant
}

#[test]
fn upscaler_choice_and_effective_set() {
    // the stored levels of the two backends count only for the chosen one; the stored set keeps both
    let mut s = Enhancements::all();
    s.set_level(E34, 3);
    s.set_level(E37, 4);
    assert_eq!(s.effective().level(E34), 0);
    assert_eq!(s.effective().level(E37), 0); // E38 = off: no upscaler
    s.set_level(E38, UP_FSR);
    assert_eq!((s.effective().level(E34), s.effective().level(E37)), (0, 4));
    s.set_level(E38, UP_METALFX);
    assert_eq!(s.effective().level(E37), 0);
    assert_eq!(s.effective().level(E34), if cfg!(target_os = "macos") { 3 } else { 0 });
    assert_eq!((s.level(E34), s.level(E37)), (3, 4)); // the stored set is untouched
    // a chosen backend with no stored level takes the default (67 % temporal / 1.5x)
    let mut d = Enhancements::all();
    d.set_level(E38, UP_FSR);
    assert_eq!(d.effective().level(E37), 2);
    // the old tokens choose their backend: `E37=3` alone is FSR 1.7x with the old sharpness, `all,E37=2` too
    let mut u = Vec::new();
    let a = Enhancements::parse_override("E37=3", &mut u).unwrap();
    assert_eq!((a.level(E38), a.level(E37), a.level(E39)), (UP_FSR, 3, 2));
    let b = Enhancements::parse_override("all,E37=2", &mut u).unwrap();
    assert_eq!((b.level(E38), b.effective().level(E37)), (UP_FSR, 2));
    // an explicit E38 wins over the inference, and E39 named stays as named
    let c = Enhancements::parse_override("E37=3,E38=0,E39=0", &mut u).unwrap();
    assert_eq!((c.level(E38), c.effective().level(E37), c.level(E39)), (UP_OFF, 0, 0));
    // UPSCALE-SOFT: `E34=n` alone takes the sharpness default too (MetalFX is sharpened since)
    let ms = Enhancements::parse_override("E34=2", &mut u).unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!((ms.level(E38), ms.level(E39)), (UP_METALFX, 2));
    }
    let m = Enhancements::parse_override("E38=1", &mut u).unwrap();
    assert_eq!(m.effective().level(E34), if cfg!(target_os = "macos") { 2 } else { 0 });
    // defaults: no upscaler
    assert_eq!(Enhancements::all().level(E38), UP_OFF);
    assert!(!Enhancements::all().temporal_upscaler());
    // `describe` parses back with an upscaler on
    let e = Enhancements::parse_override("E37=2", &mut u).unwrap();
    assert_eq!(Enhancements::parse_override(&e.describe(), &mut u), Some(e));
    // E34 (MetalFX) is a macOS-only row: elsewhere its token is reported as unknown, as any unavailable one
    let expected: &[&str] = if cfg!(target_os = "macos") { &[] } else { &["E34=2"] };
    assert_eq!(u, expected);
}

#[test]
fn mfx_rows_map_onto_the_stored_level() {
    // mode + scale <-> E34's one level, all seven levels, both ways
    for level in 1..=7u8 {
        let (m, s) = (mfx_mode(level), mfx_scale_index(level));
        assert!(s < mfx_scale_count(m), "{level}");
        assert_eq!(mfx_level(m, s), level);
    }
    assert_eq!(mfx_mode(1), MfxMode::Temporal); // 1:1 is temporal only
    assert_eq!(mfx_level(MfxMode::Temporal, 0), 1);
    assert_eq!(mfx_mode(5), MfxMode::Simple);
    // switching the mode keeps the scale; the temporal 1:1 goes to simple 67 %
    assert_eq!([2, 3, 4].map(|l| mfx_switch_mode(l, MfxMode::Simple)), [5, 6, 7]);
    assert_eq!([5, 6, 7].map(|l| mfx_switch_mode(l, MfxMode::Temporal)), [2, 3, 4]);
    assert_eq!(mfx_switch_mode(1, MfxMode::Simple), 5);
    assert_eq!(mfx_switch_mode(3, MfxMode::Temporal), 3);
    // FSR's scale row: 1:1 / 1.5x / 1.7x / 2x = E37 levels 1..=4
    assert_eq!([1, 2, 3, 4].map(fsr_scale_index), [0, 1, 2, 3]);
    assert_eq!(fsr_sharpness(0), None);
    assert_eq!(fsr_sharpness(4), Some(0.8));
}

#[test]
fn upscaler_choices_per_platform() {
    let c = upscaler_choices();
    assert_eq!(c.contains(&UP_METALFX), cfg!(target_os = "macos"));
    assert!(c.contains(&UP_FSR) && c[0] == UP_OFF);
}

/// the level maps give the values the renderer's and the app's selectors had inline, for every level
/// of their entry (and level 0 = the original where it is asked).
#[test]
fn level_maps_are_the_old_selectors() {
    let lv = |s: Switch| 0..REGISTRY[s.0 as usize].levels.len() as u8;
    assert_eq!(lv(E1).map(msaa_samples_wanted).collect::<Vec<_>>(), [1, 2, 4, 8]);
    assert_eq!(lv(E3).map(render_scale).collect::<Vec<_>>(), [1.0, 0.5, 0.75, 1.25, 1.5, 2.0]);
    assert_eq!(lv(E4).map(fov_bonus_deg).collect::<Vec<_>>(), [0.0, 5.0, 10.0, 15.0, 20.0]);
    assert_eq!(lv(E5).map(draw_reach).collect::<Vec<_>>(), [1.0, 1.25, 1.5, 2.0]);
    assert_eq!(lv(E7).map(tonemap_mode).collect::<Vec<_>>(), [0, 0, 1]);
    assert_eq!((1..lv(E13).end).map(shaft_strength).collect::<Vec<_>>(), [0.45, 0.8]);
    assert_eq!((1..lv(E26).end).map(rain_gain).collect::<Vec<_>>(), [1.0, 1.5]);
    assert_eq!((1..lv(E23).end).map(street_lamp_pool).collect::<Vec<_>>(), [(0.75, 0.7), (1.0, 1.0)]);
}

/// The rows of [`DEPENDS`] name real entries, and [`Enhancements::resolved`] zeroes exactly what [`Enhancements::blocked`]
/// shows inactive through a `Needs` row. Data only: it replaced a test that searched gpu.rs's source text for the gates.
#[test]
fn resolved_zeroes_the_dependents() {
    let known = |t: &str| REGISTRY.iter().any(|e| e.token == t);
    for d in DEPENDS {
        assert!(known(d.entry), "{}", d.entry);
        if let Cond::Needs(t) = d.cond {
            assert!(known(t) && t != d.entry, "{} needs {t}", d.entry);
        }
    }
    let sw = |t: &str| Switch(REGISTRY.iter().position(|e| e.token == t).unwrap() as u8);
    // the full set acts as stored: nothing it has on is blocked by a `Needs` row
    let all = Enhancements::all();
    assert_eq!(all.resolved(), all.effective());
    // the original, the light preset, the full set, and the full set with each prerequisite off in turn
    let mut sets = vec![Enhancements::ORIGINAL, Enhancements::lite(), all];
    for d in DEPENDS {
        if let Cond::Needs(t) = d.cond {
            let mut s = all;
            s.set_level(sw(t), 0);
            sets.push(s);
        }
    }
    for s in sets {
        let r = s.resolved();
        for d in DEPENDS {
            let e = sw(d.entry);
            if let Cond::Needs(t) = d.cond {
                // a prerequisite at 0 zeroes the dependent
                if r.level(sw(t)) == 0 {
                    assert_eq!(r.level(e), 0, "{} needs {t} in {}", d.entry, s.describe());
                }
            }
            // what resolved() turns off is exactly what the panel shows blocked by a `Needs` row
            if s.effective().level(e) > 0 {
                let needs_blocked = s.blocked(d.entry).is_some_and(|b| matches!(DEPENDS[b.dep].cond, Cond::Needs(_)));
                assert_eq!(r.level(e) == 0, needs_blocked, "{} in {}", d.entry, s.describe());
            }
        }
        // it never turns anything on and is stable
        assert!(r.levels().iter().zip(s.effective().levels()).all(|(a, b)| a == b || *a == 0));
        assert_eq!(r.resolved(), r);
    }
    // the chain: E7 off takes the lens dirt (E28) through the bloom (E10) too; the stored level is kept
    let mut s = all;
    s.set_level(E7, 0);
    assert_eq!((s.resolved().level(E10), s.resolved().level(E28)), (0, 0));
    assert_eq!(s.level(E28), all.level(E28));
}

#[test]
fn blocked_rows_follow_their_cause() {
    let mut s = Enhancements::all();
    assert_eq!(s.blocked("E10"), None);
    // HDR off blocks the bloom and the lens dirt (both rules), the stored levels stay
    s.set_level(E7, 0);
    let b = s.blocked("E10").expect("bloom needs HDR");
    assert_eq!(DEPENDS[b.dep].cond, Cond::Needs("E7"));
    assert!(s.blocked("E28").is_some() && s.blocked("E21").is_some() && s.blocked("E27").is_some() && s.blocked("E33").is_some());
    assert_eq!(s.level(E10), REGISTRY[E10.0 as usize].default_level);
    s.set_level(E7, 1);
    assert_eq!(s.blocked("E28"), None);
    // the lens dirt needs the bloom
    s.set_level(E10, 0);
    assert_eq!(DEPENDS[s.blocked("E28").unwrap().dep].cond, Cond::Needs("E10"));
    // the car ambient needs the body reflections
    s.set_level(E9, 0);
    s.set_level(E8, 0);
    assert!(s.blocked("E16").is_some());
    // ENHANCEMENT E50 / E51: the moonlight and the cars' dim-road shadow are lights / maps of E9's cascades
    assert!(s.blocked("E50").is_some() && s.blocked("E51").is_some());
    assert_eq!(DEPENDS[s.blocked("E50").unwrap().dep].cond, Cond::Needs("E9"));
    s.set_level(E9, 1);
    assert!(s.blocked("E50").is_none() && s.blocked("E51").is_none());
    let mut r = Enhancements::all();
    r.set_level(E9, 0);
    assert_eq!((r.resolved().level(E50), r.resolved().level(E51)), (0, 0));
    // an upscaler: render scale is overridden by any; MSAA / SMAA only by a temporal one
    let mut u = Enhancements::all();
    assert!(u.blocked("E3").is_none() && u.blocked("E1").is_none());
    u.set_level(E38, UP_FSR);
    assert_eq!(u.blocked("E3").map(|b| b.by), Some(UP_FSR));
    assert!(u.blocked("E1").is_some() && u.blocked("E35").is_some());
    if cfg!(target_os = "macos") {
        u.set_level(E38, UP_METALFX);
        u.set_level(E34, 6); // simple, 59 %
        assert!(u.blocked("E3").is_some());
        assert!(u.blocked("E1").is_none() && u.blocked("E35").is_none());
        u.set_level(E34, 3); // temporal
        assert!(u.blocked("E1").is_some() && u.blocked("E35").is_some());
    }
    // UPSCALE-SOFT: a temporal backend with E7 off runs nothing: the choice and its sharpness are inactive, and it blocks
    // neither MSAA / SMAA nor the render scale; the spatial MetalFX mode works without E7
    let mut h = Enhancements::all();
    h.set_level(E38, UP_FSR);
    h.set_level(E7, 0);
    assert_eq!(h.blocked("E38").map(|b| DEPENDS[b.dep].cond), Some(Cond::TemporalNeedsHdr));
    assert!(h.blocked("E39").is_some() && !h.temporal_upscaler() && !h.upscaler_running());
    assert!(h.blocked("E1").is_none() && h.blocked("E35").is_none() && h.blocked("E3").is_none());
    h.set_level(E7, 1);
    assert!(h.blocked("E38").is_none() && h.blocked("E39").is_none() && h.blocked("E1").is_some() && h.blocked("E3").is_some());
    if cfg!(target_os = "macos") {
        h.set_level(E38, UP_METALFX);
        h.set_level(E34, 6); // simple
        h.set_level(E7, 0);
        assert!(h.blocked("E38").is_none() && h.blocked("E39").is_none() && h.blocked("E3").is_some() && h.blocked("E1").is_none());
        h.set_level(E34, 2); // temporal
        assert!(h.blocked("E38").is_some() && h.blocked("E3").is_none());
    }
}
