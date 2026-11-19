//! ENHANCEMENT E44 / ENH-BEAM: the translucent-sheet test and the search for a lighthouse beam in the track's data.
//! Moved out of `world.rs` as it was.

use super::*;

/// ENHANCEMENT ENH-BEAM (E9): a texture (RGBA8) whose alpha is faint: some texels above the test reference (10), none
/// near opaque (maximum below 192, three quarters) and a low mean (below 64, a quarter): a translucent sheet (glow,
/// haze, foam, painted god rays), which neither casts nor takes a sun shadow. A cut-out (foliage, fence, sign) has
/// opaque texels or a high mean and is not one; a fully transparent texture is not either. FOREST-BEAMSHEET: forest's
/// god-ray cards `bem2` peak at 136 (1.7 % of texels at or above one half, mean 36), so the cut is no longer "all
/// below one half".
pub(super) fn is_translucent_sheet(rgba: &[u8]) -> bool {
    const REF: u8 = 10;
    const MAX_BELOW: u8 = 192;
    const MEAN_BELOW: u64 = 64;
    let n = rgba.len() / 4;
    if n == 0 {
        return false;
    }
    let (mut max, mut sum) = (0u8, 0u64);
    for p in rgba.chunks_exact(4) {
        max = max.max(p[3]);
        sum += p[3] as u64;
    }
    max > REF && max < MAX_BELOW && sum < MEAN_BELOW * n as u64
}

/// ENHANCEMENT E44: a lighthouse beam found in the track's data: the articles (animated, translucent sheets) that make it, its
/// apex (the lamp), axis, length and cross-section in the first article's own frame, and the draw items (`track_lods` entry, LOD,
/// part) of its sheets.
pub(super) struct LBeamSrc {
    pub(super) articles: Vec<usize>,
    pub(super) apex: Vec3,
    pub(super) axis: Vec3,
    pub(super) length: f32,
    pub(super) sigma0: f32,
    pub(super) spread: f32,
    pub(super) items: Vec<(usize, usize, usize)>,
}

/// How wide the soft core is against the sheets' half-height: the original's texture alpha falls to one half at about a fifth of
/// the sheet's half-height off its spine (`lths`, measured on the texture: column 2 of 16 reads 114 -> 52 over 3 of 16 rows).
pub(super) const LBEAM_CORE: f32 = 0.17;

/// ENHANCEMENT E44: the beams of the candidate articles (`(article, vertices in its own frame)`, one entry per translucent-sheet
/// article that has an animation; `frame0` = each one's matrix at frame 0): articles whose apexes coincide in the world are one
/// beam (the coastal lighthouse's `BOX01` / `OBJECT01` are two crossed sheets of one beam). The apex is the vertex the sheets share
/// most (the lamp end of a wedge), the axis points to the middle of the far end, the length is the far end's reach along it and
/// the cross-section follows the sheets' half-height (local Y) near and far.
pub(super) fn lbeam_sources(cands: &[(usize, Vec<Vec3>)], frame0: &HashMap<usize, Mat4>) -> Vec<LBeamSrc> {
    // (article, apex in its frame, apex in the world at frame 0)
    let mut placed: Vec<(usize, Vec3, Vec3)> = Vec::new();
    for (article, verts) in cands {
        // the vertices the sheets share most are the two ends of the wedge's spine; the lamp end is the one nearer the article's
        // own origin (the pivot the lantern's rotation turns about)
        let share = |a: &Vec3| verts.iter().filter(|v| v.distance(*a) < 0.05).count();
        let top = verts.iter().map(share).max().unwrap_or(0);
        let apex = verts.iter().copied().filter(|a| share(a) == top).min_by(|a, b| a.length().total_cmp(&b.length())).unwrap_or(Vec3::ZERO);
        let world = frame0.get(article).map_or(apex, |m| m.transform_point3(apex));
        tracing::debug!(article, ?apex, ?world, "E44 candidate");
        placed.push((*article, apex, world));
    }
    let mut out: Vec<LBeamSrc> = Vec::new();
    let mut used = vec![false; placed.len()];
    for i in 0..placed.len() {
        if used[i] {
            continue;
        }
        let members: Vec<usize> = (i..placed.len()).filter(|&j| !used[j] && placed[j].2.distance(placed[i].2) < 1.0).collect();
        for &j in &members {
            used[j] = true;
        }
        let a0 = placed[i].1;
        // the union of the members' vertices in the first member's frame (their frames are the same up to the animation's own phase)
        let verts: Vec<Vec3> = members.iter().flat_map(|&j| {
            let off = a0 - placed[j].1;
            cands[j].1.iter().map(move |v| *v + off)
        }).collect();
        let dist = |v: &Vec3| v.distance(a0);
        let max = verts.iter().map(dist).fold(0.0f32, f32::max);
        if max < 5.0 {
            continue;
        }
        let far: Vec<Vec3> = verts.iter().copied().filter(|v| dist(v) >= 0.9 * max).collect();
        let centre = far.iter().fold(Vec3::ZERO, |a, v| a + *v) / far.len() as f32;
        let axis = (centre - a0).normalize_or_zero();
        if axis == Vec3::ZERO {
            continue;
        }
        let length = far.iter().map(|v| (*v - a0).dot(axis)).sum::<f32>() / far.len() as f32;
        // the sheets' half-height (local Y) at the far end and at the near rim; the soft core's sigma follows it linearly
        let half = |set: &[Vec3]| set.iter().map(|v| (v.y - a0.y).abs()).fold(0.0f32, f32::max);
        let h_far = half(&far);
        let near: Vec<Vec3> = verts.iter().copied().filter(|v| (*v - a0).dot(axis) > 0.02 * length && (*v - a0).dot(axis) < 0.3 * length).collect();
        let (sigma0, spread) = if near.is_empty() {
            (0.7 * LBEAM_CORE * h_far, 0.3 * LBEAM_CORE * h_far / length)
        } else {
            let s_near = near.iter().map(|v| (*v - a0).dot(axis)).sum::<f32>() / near.len() as f32;
            let h_near = half(&near);
            let slope = (h_far - h_near) / (length - s_near).max(1.0);
            ((h_near - slope * s_near).max(0.2 * h_far) * LBEAM_CORE, slope.max(0.0) * LBEAM_CORE)
        };
        // a narrow core at the glass that widens toward the end (the far end's sigma is kept)
        let (sigma0, spread) = {
            let far_sigma = sigma0 + spread * length;
            let s0 = 0.3 * sigma0;
            (s0, (far_sigma - s0) / length)
        };
        out.push(LBeamSrc { articles: members.iter().map(|&j| placed[j].0).collect(), apex: a0, axis, length, sigma0, spread, items: Vec::new() });
    }
    out
}

#[cfg(test)]
mod sheet_tests {
    use super::is_translucent_sheet;

    fn tex(alphas: &[u8]) -> Vec<u8> {
        alphas.iter().flat_map(|&a| [255, 255, 255, a]).collect()
    }

    /// ENHANCEMENT E44: the coastal lighthouse's two crossed sheets (`BOX01` / `OBJECT01`, local vertices as in the `.crp`) are one
    /// beam: apex at the lamp end (nearer the pivot), axis to the far end, length about 102 m, a cross-section of a few metres
    #[test]
    fn lighthouse_sheets_are_one_beam() {
        use super::{lbeam_sources, Mat4, Vec3};
        let a = Vec3::new(-2.187, 8.409, 1.798);
        let box01 = vec![Vec3::new(-63.76, 21.81, 81.71), a, Vec3::new(-7.26, 18.12, 7.95), Vec3::new(-63.76, 21.81, 81.71), Vec3::new(-58.43, 8.41, 87.92), a, Vec3::new(-7.26, -1.30, 7.95), Vec3::new(-58.43, 8.41, 87.92), Vec3::new(-63.76, -4.99, 81.71), Vec3::new(-7.26, -1.30, 7.95), a, Vec3::new(-58.43, 8.41, 87.92)];
        let obj = vec![a, Vec3::new(-64.05, 21.81, 81.49), Vec3::new(-7.29, 18.12, 7.92), a, Vec3::new(-70.57, 8.41, 77.18), Vec3::new(-64.05, 21.81, 81.49), Vec3::new(-7.29, -1.30, 7.92), Vec3::new(-70.57, 8.41, 77.18), Vec3::new(-64.05, -4.99, 81.49), Vec3::new(-7.29, -1.30, 7.92), a, Vec3::new(-70.57, 8.41, 77.18)];
        let f0: std::collections::HashMap<usize, Mat4> = [(16, Mat4::IDENTITY), (456, Mat4::IDENTITY)].into_iter().collect();
        let b = lbeam_sources(&[(16, box01), (456, obj)], &f0);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].articles, vec![16, 456]);
        assert!(b[0].apex.distance(a) < 0.1);
        assert!((b[0].length - 102.0).abs() < 3.0, "{}", b[0].length);
        assert!(b[0].axis.dot(Vec3::new(-0.61, 0.0, 0.79)) > 0.99);
        assert!(b[0].sigma0 > 0.2 && b[0].sigma0 < 3.0 && b[0].spread >= 0.0);
    }

    /// ENHANCEMENT ENH-BEAM: the lighthouse beam's alpha (0..=119) and forest's god rays (max 136) are sheets; a cut-out
    /// with opaque texels, a fully transparent texture, a half-opaque fence (231 max) and a high-mean one are not
    #[test]
    fn translucent_sheet_rule() {
        assert!(is_translucent_sheet(&tex(&[0, 34, 80, 119])));
        assert!(!is_translucent_sheet(&tex(&[0, 0, 255, 255])));
        assert!(!is_translucent_sheet(&tex(&[0, 0, 0, 5])));
        assert!(!is_translucent_sheet(&tex(&[0, 151, 231])));
        assert!(!is_translucent_sheet(&tex(&[0, 100, 128])));
        // FOREST-BEAMSHEET: `bem2`-like (max 136, mean ~36, mostly 0 and a few texels past one half) is a sheet
        let mut bem: Vec<u8> = vec![0; 226];
        bem.extend(std::iter::repeat(30).take(400));
        bem.extend(std::iter::repeat(60).take(340));
        bem.extend(std::iter::repeat(100).take(17));
        bem.extend(std::iter::repeat(136).take(17));
        assert!(is_translucent_sheet(&tex(&bem)));
        // faint but with a bright core (max 192) or a high mean is not
        assert!(!is_translucent_sheet(&tex(&[0, 0, 0, 0, 0, 192])));
        assert!(!is_translucent_sheet(&tex(&[60, 70, 80, 90])));
    }
}
