//! The post chain of the float scene: the motion blur (E27), the depth of field (the photo mode, D46 / E31), the
//! windscreen drops (E40), the bloom (E10) and the eye adaptation's measure (E21), in that order, each taking the scene
//! image the passes before it left ([`PostChain`]) and the frame's [`PostCtx`]. Which of them run this frame is decided
//! first ([`PostPlan`], from the switches and the scene: the selectors `enh_blur`, `enh_dof`, `bloom_key` and `post_plan`
//! below); the tone map after them reads [`PostChain::current`]. `PostPasses::ensure_tone_layouts` builds the bloom and eye
//! passes early when the tone map needs their layouts.

use crate::gfx::{self, DepthSrc, PostCtx, RtImage};
use super::*;

/// The post chain's passes (each built at its first use) and the lens dirt.
#[derive(Default)]
pub(super) struct PostPasses {
    /// ENHANCEMENT E10: the bloom chain (built at first use; its pipelines per level)
    pub bloom: Option<crate::bloom::BloomPass>,
    /// ENHANCEMENT E21: the eye adaptation's state (built at first use)
    pub eye: Option<crate::adapt::EyePass>,
    /// ENHANCEMENT E27: the motion blur pass (built at first use)
    pub mblur: Option<crate::camfx::MotionBlur>,
    /// ENHANCEMENTS E31 / the photo mode (D46): the depth-of-field pass, built at the first use
    pub dof: Option<crate::dof::DofPass>,
    /// ENHANCEMENT E40: the windscreen drops' pass and the water on the glass (built on first use)
    pub drops: Option<crate::drops::DropsPass>,
    /// ENHANCEMENT E28: the generated lens dirt (built at first use)
    pub dirt: Option<crate::camfx::Dirt>,
    /// ENHANCEMENT E28: the dirt's mask being generated off the frame (started at the chain's first run)
    dirt_job: Option<std::thread::JoinHandle<Vec<f32>>>,
    /// PERF: the float images the passes above write the scene into, in turn (a pass reads the one the pass before it wrote and
    /// writes the other): two full-size images however many of the passes run
    pool: [Option<RtImage>; 2],
}

/// The pool's image a pass writes next: not the one the chain carries now (which that pass reads), of `size`.
fn pool_target<'p>(pool: &'p mut [Option<RtImage>; 2], chain: &PostChain, cx: &PostCtx, size: (u32, u32)) -> &'p RtImage {
    let cur = chain.out.as_ref().map(|o| &o.1);
    let i = usize::from(pool[0].as_ref().is_some_and(|img| Some(&img.view) == cur));
    RtImage::ensure(&mut pool[i], cx.device, "post chain image", cx.layout, cx.sampler, size.0, size.1, HDR_FORMAT, gfx::RT_USAGE)
}

/// Which passes of the chain run this frame, and with what.
pub(super) struct PostPlan {
    /// ENHANCEMENT E27: the blur's level and the previous frame
    pub blur: Option<(u8, crate::MotionPrev)>,
    /// ENHANCEMENTS E31 / the photo mode (D46): the lens
    pub dof: Option<crate::Dof>,
    /// ENHANCEMENT E40: the windscreen, its glass on the screen and the frame's look
    pub drops: Option<(crate::drops::Drops, crate::drops::Glass, crate::drops::Look)>,
    /// ENHANCEMENT E10: the bloom's level (0 = none) and E28's dirt on its last add
    pub bloom_level: u8,
    pub dirt: bool,
    /// ENHANCEMENT E21: the eye adaptation measures the finished image
    pub eye: bool,
}

/// The image the chain carries as the scene: the scaled 3-D image's own (`base`, its bind group `base_bg`) until a pass
/// writes one of its own (`out`).
pub(super) struct PostChain {
    base_bg: Option<wgpu::BindGroup>,
    base_view: wgpu::TextureView,
    pub out: Option<(wgpu::BindGroup, wgpu::TextureView)>,
}

impl PostChain {
    pub fn new(base_bg: Option<&wgpu::BindGroup>, base_view: &wgpu::TextureView) -> Self {
        Self { base_bg: base_bg.cloned(), base_view: base_view.clone(), out: None }
    }

    /// The scene's bind group a pass reads (`None`: no image to read, the pass is skipped).
    fn src(&self) -> Option<wgpu::BindGroup> {
        self.out.as_ref().map(|o| o.0.clone()).or_else(|| self.base_bg.clone())
    }

    /// A pass wrote the scene into an image of its own: the passes after it take that one.
    fn set(&mut self, (bg, view): (&wgpu::BindGroup, &wgpu::TextureView)) {
        self.out = Some((bg.clone(), view.clone()));
    }

    /// The scene as it stands: its bind group and view.
    pub fn current(&self) -> (Option<&wgpu::BindGroup>, &wgpu::TextureView) {
        match self.out.as_ref() {
            Some((bg, view)) => (Some(bg), view),
            None => (self.base_bg.as_ref(), &self.base_view),
        }
    }
}

/// ENHANCEMENT E1: the depth the chain reads — the multisampled one of a multisampled frame, else the single-sample `depth`.
pub(super) fn depth_src<'a>(ms: Option<&'a Ms>, depth: &'a wgpu::TextureView) -> DepthSrc<'a> {
    match ms {
        Some(m) => DepthSrc { view: &m.depth, multisampled: true },
        None => DepthSrc { view: depth, multisampled: false },
    }
}

impl Core {
    /// ENHANCEMENT E27: the motion blur's level, the previous frame and whether this frame can have it (the float
    /// scene of the race's main view, the switch on).
    pub(super) fn enh_blur(&self, scene: &Scene, hdr: bool) -> Option<(u8, crate::MotionPrev)> {
        if !hdr || !self.enhancements.is_on(crate::E27) || scene.items.is_empty() {
            return None;
        }
        scene.motion.map(|m| (self.enhancements.level(crate::E27), m))
    }

    /// ENHANCEMENTS E7 / E31: this frame's depth of field, when the scene brings a lens (the float scene only): the photo
    /// mode's (PORT-DEVIATION D46, set by the application only with E7 on) or a replay's (E31).
    pub(super) fn enh_dof(&self, scene: &Scene, hdr: bool) -> Option<crate::Dof> {
        if !hdr || scene.items.is_empty() {
            return None;
        }
        scene.dof
    }

    /// ENHANCEMENTS E10 / E28: the bloom's level and the lens dirt on its last add (the tone map's key), `None` = no bloom
    /// (`hdr`: the float scene; `dbg_only`: a temporal debug view, no chain).
    pub(super) fn bloom_key(&self, hdr: bool, dbg_only: bool) -> Option<(u8, bool)> {
        let level = if hdr && !dbg_only { self.enhancements.level(crate::E10) } else { 0 };
        (level > 0).then(|| (level, self.enhancements.is_on(crate::E28)))
    }

    /// ENHANCEMENTS E27 / the depth of field (photo mode, E31) / E40 / E10 / E28: which passes of the post chain run this frame (`size`: the chain's;
    /// `hdr`: the float scene; `dbg_only`: a temporal debug view, no chain). The eye adaptation (E21) is the caller's.
    pub(super) fn post_plan(&self, scene: &Scene, camera: &Camera, aspect: f32, hdr: bool, dbg_only: bool, size: (u32, u32)) -> chain::PostPlan {
        // ENHANCEMENT E40: the driver view's windscreen of a rainy track, its glass on the screen
        let drops = scene.drops.filter(|_| self.enhancements.is_on(crate::E40) && hdr && !scene.mirror && !scene.items.is_empty() && !dbg_only).and_then(|dr| {
            let look = crate::drops::Look { strength: 1.0, sky: [scene.clear_color[0] as f32, scene.clear_color[1] as f32, scene.clear_color[2] as f32], width: size.0, height: size.1 };
            crate::drops::glass(&dr, camera, aspect).map(|gl| (dr, gl, look))
        });
        chain::PostPlan {
            blur: self.enh_blur(scene, hdr).filter(|_| !dbg_only),
            dof: self.enh_dof(scene, hdr).filter(|_| !dbg_only),
            drops,
            bloom_level: self.bloom_key(hdr, dbg_only).map_or(0, |b| b.0),
            dirt: self.enhancements.is_on(crate::E28),
            eye: false,
        }
    }
}

impl PostPasses {
    /// ENHANCEMENT E21 / E10 (PERF): the eye adaptation and the bloom made ahead of the tone map, which takes their groups' layouts.
    pub fn ensure_tone_layouts(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, eye: bool, bloom: bool) {
        if eye && self.eye.is_none() {
            self.eye = Some(crate::adapt::EyePass::new(device, layout));
        }
        if bloom && self.bloom.is_none() {
            self.bloom = Some(crate::bloom::BloomPass::new(device, layout));
        }
    }

    /// Run the planned passes in order over `chain`; `scene` / `scene_view`: the scaled 3-D image itself (the drops draw
    /// into it in place while no pass before them made an image of its own).
    pub fn run(&mut self, cx: &mut PostCtx, plan: PostPlan, chain: &mut PostChain, scene: &wgpu::Texture, scene_view: &wgpu::TextureView) {
        // ENHANCEMENT E28: the dirt's mask is made on a thread from the first run on, so the frame that first needs it does
        // not generate it (it waits only for what is left)
        if self.dirt.is_none() && self.dirt_job.is_none() {
            self.dirt_job = Some(crate::camfx::Dirt::generate());
        }
        // ENHANCEMENT E27: the camera-motion blur of the float scene, into an image of its own, which the
        // bloom, the eye adaptation and the tone map then take as the scene
        // PERF: not while the camera stands still (every pixel would come out as it went in)
        if let (Some((level, prev)), Some(src)) = (plan.blur, chain.src())
            && !crate::camera::camera_still(cx.camera, &prev.camera, cx.aspect, cx.mirror)
        {
            let m = self.mblur.get_or_insert_with(|| crate::camfx::MotionBlur::new(cx.device, cx.layout));
            let out = pool_target(&mut self.pool, chain, cx, cx.size);
            m.run(cx, &src, &prev, level, &out.view);
            chain.set((&out.bg, &out.view));
        }
        // ENHANCEMENTS E31 / the photo mode (D46): the depth of field of the photo mode / a replay, on whatever the blur left
        if let (Some(lens), Some(src)) = (plan.dof, chain.src()) {
            let d = self.dof.get_or_insert_with(|| crate::dof::DofPass::new(cx.device, cx.layout));
            let out = pool_target(&mut self.pool, chain, cx, cx.size);
            d.run(cx, &src, lens, &out.view);
            chain.set((&out.bg, &out.view));
        }
        // ENHANCEMENT E40: the raindrops on the windscreen of the driver view, over whatever the passes above left (before
        // the bloom, so a lamp seen through a drop blooms)
        let mut drops_on = false;
        if let Some((dr, gl, look)) = plan.drops {
            let d = self.drops.get_or_insert_with(|| crate::drops::DropsPass::new(cx.device, cx.layout));
            if d.prepare(&dr, &gl).is_some() {
                match chain.out.as_ref().map(|b| b.0.clone()) {
                    // an earlier pass left the scene in an image of its own: a new image the size of the frame
                    Some(src) => {
                        let out = pool_target(&mut self.pool, chain, cx, (look.width, look.height));
                        d.run(cx, &src, &gl, look, &out.view);
                        chain.set((&out.bg, &out.view));
                    }
                    // the scene's own image (the usual case): only the glass's box is copied and drawn over
                    None => d.run_in_place(cx, scene, scene_view, &gl, look),
                }
                drops_on = true;
            }
        }
        if !drops_on && let Some(d) = self.drops.as_mut() {
            // off the windscreen (or a clean glass): the water is forgotten, the next look starts from a fresh pre-roll
            d.sim = None;
        }
        let (img_bg, _) = chain.current();
        // ENHANCEMENT E10: the bloom of the float scene, added in the tone map
        let mut bloom_group = None;
        if plan.bloom_level > 0 {
            // ENHANCEMENT E28: the last add through the lens dirt
            if plan.dirt && self.dirt.is_none() {
                let mask = match self.dirt_job.take().map(|j| j.join()) {
                    Some(Ok(mask)) => mask,
                    // the thread could not finish (it panicked): made here instead
                    _ => crate::camfx::generate_dirt(crate::camfx::DIRT_SIZE.0, crate::camfx::DIRT_SIZE.1),
                };
                self.dirt = Some(crate::camfx::Dirt::new(cx.device, cx.queue, &mask));
            }
            let bloom = self.bloom.get_or_insert_with(|| crate::bloom::BloomPass::new(cx.device, cx.layout));
            if let Some(bg) = img_bg {
                // PERF: the chain up to its first mip; the last add is the tone map's (through `bloom_group`)
                bloom.encode(cx, plan.bloom_level, bg, self.dirt.as_ref().filter(|_| plan.dirt).map(|d| &d.view));
                bloom_group = bloom.tone();
            }
        }
        // ENHANCEMENT E21: measure the finished float image (the bloom's add included: the measure adds it itself), then the
        // tone map reads the exposure
        if plan.eye && let (Some(e), Some(bg)) = (self.eye.as_mut(), img_bg) {
            e.encode(cx, bg, bloom_group.map(|(layout, group)| crate::adapt::Bloom { layout, group, level: plan.bloom_level, dirt: plan.dirt && self.dirt.is_some() }));
        }
    }
}
