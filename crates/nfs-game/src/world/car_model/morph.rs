//! The car's morph parts (the roof and the animated panels): the vertex blend between key frames. Moved out of `car_model.rs` as it was.

use super::*;

/// One mesh of a morphing article: its frame-0 vertices and every frame's positions.
#[derive(Clone)]
pub(super) struct MorphPart {
    pub(super) mesh: MeshId,
    /// The vertices last blended and queued for the mesh (the frame-0 vertices at first, which is
    /// what the upload holds): the next blend rewrites only the positions, in place.
    pub(super) scratch: Vec<Vertex>,
    /// `Renderer::mesh_writes` of the mesh right after this part's last write (0 at the upload):
    /// the mesh still holds `scratch` while the count is the same.
    pub(super) seen: u32,
    pub(super) frames: Vec<Vec<[f32; 3]>>,
}

pub(super) fn mm_positions(m: &MeshData) -> Vec<[f32; 3]> {
    m.vertices.iter().map(|v| v.position).collect()
}

impl CarModel {
    /// **0x4D1990(1, state)**: the roof animation to `length · state / 255` frames (0x4D199F…
    /// 0x4D19BA), which 0x4380D0 splits as `frame = fistp(t − 0.5)`, the fraction the rest, and
    /// the draw (0x43E2FD…0x43E35B) blends each vertex `A + (B − A)·frac` between that frame and
    /// the next — the frame alone where there is no next. Past the last frame the part keeps it.
    pub fn set_roof(&mut self, r: &mut Renderer, state: i32) {
        if self.anim_length[1] == 0 || state == self.roof_state {
            return;
        }
        self.roof_state = state;
        let t = self.anim_length[1] as f32 * state as f32 * (1.0 / 255.0);
        self.set_anim(r, 1, t);
    }

    /// The roof state [`set_roof`](Self::set_roof) last took — what the front end's car object
    /// keeps at `+4` and 0x4D19E0(1) answers.
    pub fn roof_state(&self) -> i32 {
        self.roof_state
    }

    /// `0x438190(n)`: animation `n`'s length in frames, 0 when the car has none.
    pub fn anim_length(&self, n: usize) -> u32 {
        self.anim_length.get(n).copied().unwrap_or(0)
    }

    /// **0x4380D0(n, t)**: animation `n` to time `t` frames, clamped to `[0, length]`.
    pub fn set_anim(&mut self, r: &mut Renderer, n: usize, t: f32) {
        let len = self.anim_length(n);
        if len == 0 {
            return;
        }
        let t = t.clamp(0.0, len as f32);
        if t == self.anim_time[n] {
            return;
        }
        self.anim_time[n] = t;
        let frame = (t - 0.5).round_ties_even().max(0.0) as usize;
        let frac = t - frame as f32;
        self.blend(r, n, frame, frame + 1, frac);
    }

    /// Write animation `n`'s `{frame, next, frac}` straight into its record, as slot 9's keyframe
    /// copies do (0x439035…0x43908D for the right arm, 0x4392F1…0x43933C for the body): the draw
    /// (0x43E2FD…0x43E35B) blends each vertex from `frame` toward `next` by `frac`.
    pub(super) fn set_anim_keys(&mut self, r: &mut Renderer, n: usize, frame: u32, next: u32, frac: f32) {
        if self.anim_length(n) == 0 {
            return;
        }
        // a key write is not a time: forget the last one so a later 0x4380D0 re-uploads
        self.anim_time[n] = -1.0;
        self.blend(r, n, frame as usize, next as usize, frac);
    }

    /// The draw's blend of animation `n`'s parts (0x43E2FD…0x43E35B). Each part keeps the vertices it
    /// last queued: the positions are blended in place (the same `A + (B − A)·frac` per vertex, the
    /// other fields are the frame-0 ones, which nothing rewrites) and the mesh is rewritten only when
    /// a position changed bit for bit or someone else (the crush, another instance sharing the mesh)
    /// has written it since — a write of the bytes the mesh already holds is not made.
    pub(super) fn blend(&mut self, r: &mut Renderer, n: usize, frame: usize, next: usize, frac: f32) {
        if self.blended[n] == Some((frame, next, frac)) {
            return;
        }
        self.blended[n] = Some((frame, next, frac));
        for p in &mut self.morph[n] {
            let Some(a) = p.frames.get(frame.min(p.frames.len() - 1)) else { continue };
            let b = p.frames.get(next);
            let frac = if frame >= p.frames.len() { 0.0 } else { frac };
            let mut changed = p.seen != r.mesh_writes(p.mesh);
            for (i, out) in p.scratch.iter_mut().enumerate() {
                let pa = Vec3::from(a[i]);
                let position = match b {
                    Some(b) => (pa + (Vec3::from(b[i]) - pa) * frac).to_array(),
                    None => a[i],
                };
                if position.map(f32::to_bits) != out.position.map(f32::to_bits) {
                    out.position = position;
                    changed = true;
                }
            }
            if changed {
                r.update_mesh_vertices(p.mesh, &p.scratch);
                p.seen = r.mesh_writes(p.mesh);
            }
        }
    }
}
