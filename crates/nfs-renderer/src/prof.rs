//! TEST GLUE `NFS_GPU_PROFILE=1`: a per-pass GPU timer (diagnostic, off by default; not the original's).
//!
//! `NFS_GPU_PROFILE=minimal` is the other value of the variable: the plain device (no optional feature, so no timestamp
//! queries), which the start-up crash guard asks for in safe mode. It never turns the profiler on.
//!
//! Every render / compute pass of the renderer asks [`rp`] / [`cp`] for its `timestamp_writes`; with the env var
//! unset (or an adapter without `TIMESTAMP_QUERY`) both return `None` and the pass is exactly what it was. With it
//! set, each pass writes its begin / end tick into a query set; the encoder that holds the passes resolves them
//! ([`resolve`]) and [`end_frame`] reads them back (waiting for the device) and logs, every 120 frames, one line per
//! pass label: the mean ms per frame and the mean number of runs per frame, plus the frame's GPU total. The wait
//! makes the frame slower than it is - read the ms of a pass, not the fps. Nothing here touches what is drawn.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

const MAX_PASSES: u32 = 512;

/// Nanoseconds a millisecond.
const NS_PER_MS: f64 = 1.0e6;

/// The milliseconds since `t0`.
pub(crate) fn elapsed_ms(t0: std::time::Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

struct Prof {
    qs: &'static wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    /// `NFS_GPU_PROFILE` is set: the blocking per-pass log
    log: bool,
    /// the live read-back's `map_async` has completed (set by its callback)
    mapped: std::sync::Arc<AtomicBool>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    labels: Vec<String>,
    ctx: &'static str,
    resolved: u32,
    frames: u32,
    acc: BTreeMap<String, (f64, u32)>,
    /// the submits of the frames in `acc` ([`SUBMITS`])
    submits: u32,
    /// live mode: a read-back of `pending.len()` passes is mapped (or being mapped); no pass is timed meanwhile
    pending: Vec<String>,
}

static PROF: OnceLock<Prof> = OnceLock::new();

/// PORT-DEVIATION D63 (the race panel): the panel is open. While it is, the passes are timed and the draws counted, without
/// waiting for the GPU; closed, none of it runs (a pass asks for its `timestamp_writes` and gets `None`, a draw costs one load).
static LIVE: AtomicBool = AtomicBool::new(false);
static DRAWS: AtomicU32 = AtomicU32::new(0);
static TRIS: AtomicU64 = AtomicU64::new(0);
/// Every `Queue::submit` that carries the frames' command buffers ([`submit`]), for the profile's "submits per frame" line.
static SUBMITS: AtomicU32 = AtomicU32::new(0);

/// What the panel shows of the renderer: the last frame's draw calls and triangles submitted, and the last GPU timing read back.
#[derive(Debug, Clone, Default)]
pub struct LiveStats {
    pub draws: u32,
    pub tris: u64,
    /// the sum of the frame's timed passes, milliseconds; `None` = the adapter has no timestamp queries (or no read-back has come yet)
    pub gpu_total_ms: Option<f64>,
    /// the passes by label (runs of one label added up), milliseconds
    pub gpu_passes: Vec<(String, f64)>,
}

static LAST: Mutex<LiveStats> = Mutex::new(LiveStats { draws: 0, tris: 0, gpu_total_ms: None, gpu_passes: Vec::new() });

/// Turn the panel's live statistics on or off.
pub fn live_set(on: bool) {
    if LIVE.swap(on, Relaxed) != on {
        DRAWS.store(0, Relaxed);
        TRIS.store(0, Relaxed);
        if let Some(p) = PROF.get() && let Ok(mut s) = p.state.lock() && s.pending.is_empty() {
            s.labels.clear();
            s.resolved = 0;
        }
        if !on && let Ok(mut l) = LAST.lock() {
            *l = LiveStats::default();
        }
    }
}

/// Whether the GPU can time passes (the device was created with timestamp queries).
pub fn timestamps_supported() -> bool {
    PROF.get().is_some()
}

/// The last frame's numbers (see [`LiveStats`]).
pub fn live_stats() -> LiveStats {
    LAST.lock().map(|l| l.clone()).unwrap_or_default()
}

/// One draw call of `tris` triangles (only counted while the panel is open).
#[inline]
pub(crate) fn count_draw(tris: u64) {
    if LIVE.load(Relaxed) {
        DRAWS.fetch_add(1, Relaxed);
        TRIS.fetch_add(tris, Relaxed);
    }
}

/// `NFS_GPU_PROFILE=minimal`: a device with no optional feature.
pub(crate) fn minimal() -> bool {
    crate::dbg::flags().gpu_profile.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("minimal"))
}

/// Whether the feature is to be requested at device creation: only with the env var set to something but `minimal` / `0`.
pub(crate) fn wanted() -> bool {
    crate::dbg::flags().gpu_profile.as_deref().is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("minimal"))
}

/// Whether the adapter's timestamp queries are to be requested at device creation: the profiler wants them, or the race panel may
/// (PORT-DEVIATION D63); never in safe mode. Requesting the feature changes nothing a draw does: a pass gets `timestamp_writes` only
/// while the log is on or the panel is open.
pub(crate) fn request_timestamps() -> bool {
    !minimal()
}

pub(crate) fn init(device: &wgpu::Device, queue: &wgpu::Queue) {
    if minimal() || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return;
    }
    let qs = Box::leak(Box::new(device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("prof"), ty: wgpu::QueryType::Timestamp, count: MAX_PASSES * 2 })));
    let bytes = MAX_PASSES as u64 * 2 * 8;
    let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof resolve"), size: bytes, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof readback"), size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let _ = PROF.set(Prof { qs, resolve, readback, period_ns: queue.get_timestamp_period() as f64, log: wanted(), mapped: Default::default(), state: Mutex::new(State::default()) });
    if wanted() {
        tracing::info!("NFS_GPU_PROFILE: per-pass timestamps on");
    }
}

fn slot(label: &'static str) -> Option<(&'static wgpu::QuerySet, u32, u32)> {
    let p = PROF.get()?;
    if !p.log && !LIVE.load(Relaxed) {
        return None;
    }
    let mut s = p.state.lock().ok()?;
    // live mode: the read-back buffer is still on its way, so this frame is not timed
    if !s.pending.is_empty() {
        return None;
    }
    let i = s.labels.len() as u32;
    if i >= MAX_PASSES {
        return None;
    }
    let l = format!("{label}{}", s.ctx);
    s.labels.push(l);
    Some((p.qs, i * 2, i * 2 + 1))
}

/// What the passes recorded from now on are for (a suffix of their label): "" the main view.
pub(crate) fn ctx(c: &'static str) {
    if let Some(p) = PROF.get() && let Ok(mut s) = p.state.lock() {
        s.ctx = c;
    }
}

/// `timestamp_writes` for a render pass.
pub(crate) fn rp(label: &'static str) -> Option<wgpu::RenderPassTimestampWrites<'static>> {
    slot(label).map(|(q, b, e)| wgpu::RenderPassTimestampWrites { query_set: q, beginning_of_pass_write_index: Some(b), end_of_pass_write_index: Some(e) })
}

/// ENHANCEMENT (the enhancements' compute passes): `timestamp_writes` for a compute pass.
pub(crate) fn cp(label: &'static str) -> Option<wgpu::ComputePassTimestampWrites<'static>> {
    slot(label).map(|(q, b, e)| wgpu::ComputePassTimestampWrites { query_set: q, beginning_of_pass_write_index: Some(b), end_of_pass_write_index: Some(e) })
}

/// Resolve the passes recorded since the last call; call before `encoder.finish()` of every encoder with timed passes.
pub(crate) fn resolve(encoder: &mut wgpu::CommandEncoder) {
    let Some(p) = PROF.get() else { return };
    if !p.log && !LIVE.load(Relaxed) {
        return;
    }
    let Ok(mut s) = p.state.lock() else { return };
    let (from, to) = (s.resolved, s.labels.len() as u32);
    if from >= to {
        return;
    }
    encoder.resolve_query_set(p.qs, from * 2..to * 2, &p.resolve, 0);
    encoder.copy_buffer_to_buffer(&p.resolve, 0, &p.readback, from as u64 * 16, (to - from) as u64 * 16);
    s.resolved = to;
}

/// The frame's end: read the resolved timestamps (the device is waited for) and add them up.
pub(crate) fn end_frame(device: &wgpu::Device) {
    if LIVE.load(Relaxed) {
        let mut l = LAST.lock();
        if let Ok(l) = l.as_mut() {
            l.draws = DRAWS.swap(0, Relaxed);
            l.tris = TRIS.swap(0, Relaxed);
        }
    }
    let Some(p) = PROF.get() else { return };
    if !p.log && !LIVE.load(Relaxed) {
        return;
    }
    let Ok(mut s) = p.state.lock() else { return };
    if !p.log {
        end_frame_live(device, p, &mut s);
        return;
    }
    s.submits += SUBMITS.swap(0, Relaxed);
    let n = s.resolved;
    if n == 0 {
        return;
    }
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let slice = p.readback.slice(..n as u64 * 16);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let Ok(view) = slice.get_mapped_range() else { return };
    let data: Vec<u64> = bytemuck::cast_slice::<u8, u64>(&view).to_vec();
    drop(view);
    p.readback.unmap();
    let labels = std::mem::take(&mut s.labels);
    let mut total = 0.0;
    for (i, l) in labels.iter().take(n as usize).enumerate() {
        let (b, e) = (data[i * 2], data[i * 2 + 1]);
        let ms = if e > b { (e - b) as f64 * p.period_ns / NS_PER_MS } else { 0.0 };
        total += ms;
        let a = s.acc.entry(l.clone()).or_default();
        a.0 += ms;
        a.1 += 1;
    }
    s.acc.entry("TOTAL (sum of passes)".to_string()).or_default().0 += total;
    s.resolved = 0;
    s.frames += 1;
    if s.frames >= 120 {
        let f = s.frames as f64;
        let mut line = String::new();
        for (l, (ms, runs)) in &s.acc {
            line.push_str(&format!("\n  {:<40} {:7.3} ms/frame  x{:.2}", l, ms / f, *runs as f64 / f));
        }
        line.push_str(&format!("\n  queue submits per frame: {:.2}", s.submits as f64 / f));
        s.submits = 0;
        tracing::info!("GPU profile, mean of {} frames:{}", s.frames, line);
        s.acc.clear();
        s.frames = 0;
    }
}

/// The live read-back, without a wait: collect the one that has come (a poll that does not block), then start the next.
fn end_frame_live(device: &wgpu::Device, p: &Prof, s: &mut State) {
    if !s.pending.is_empty() {
        let _ = device.poll(wgpu::PollType::Poll);
        if !p.mapped.load(Relaxed) {
            return;
        }
        let labels = std::mem::take(&mut s.pending);
        let slice = p.readback.slice(..labels.len() as u64 * 16);
        if let Ok(view) = slice.get_mapped_range() {
            let data: &[u64] = bytemuck::cast_slice(&view);
            let mut passes: BTreeMap<String, f64> = BTreeMap::new();
            let mut total = 0.0;
            for (i, l) in labels.iter().enumerate() {
                let (b, e) = (data[i * 2], data[i * 2 + 1]);
                let ms = if e > b { (e - b) as f64 * p.period_ns / NS_PER_MS } else { 0.0 };
                total += ms;
                *passes.entry(l.clone()).or_default() += ms;
            }
            if let Ok(mut l) = LAST.lock() {
                l.gpu_total_ms = Some(total);
                l.gpu_passes = passes.into_iter().collect();
            }
        }
        p.readback.unmap();
        p.mapped.store(false, Relaxed);
        return;
    }
    let n = s.resolved;
    if n == 0 {
        s.labels.clear();
        return;
    }
    p.mapped.store(false, Relaxed);
    let mapped = p.mapped.clone();
    p.readback.slice(..n as u64 * 16).map_async(wgpu::MapMode::Read, move |r| {
        if r.is_ok() {
            mapped.store(true, Relaxed);
        }
    });
    s.pending = std::mem::take(&mut s.labels);
    s.pending.truncate(n as usize);
    s.resolved = 0;
}

// ---------------------------------------------------------------------------------------------------------------------
// TEST GLUE (`--perf-bench`, not the original's): the bench's stage marks and its per-frame numbers.
//
// A mark is an EMPTY compute pass whose beginning writes a timestamp: the render passes keep their descriptors (no
// `timestamp_writes` of their own, which on Apple GPUs time a pass that overlaps its neighbours), and a stage's cost is the
// difference of two marks on one timeline. Every `Queue::submit` of a frame goes through [`submit`]: with the marks on it
// puts a mark command buffer before and after the ones it is given, so a submit is one segment of the timeline whose span
// (end - begin) is GPU work only (a command buffer recorded elsewhere, MetalFX / FSR, falls into the stage of the mark
// that follows it). The frame's GPU time is the union of its segments' spans: the idle GPU between two submits (the CPU
// still encoding) is not counted, and the overlap of two (a begin mark runs as soon as it is submitted) not twice. [`finish`] / [`submit`] also add up the CPU time of `CommandEncoder::finish` and
// `Queue::submit`. Off (the default) each of them costs one atomic load and records nothing.
// Measured on an M1 Pro: a mark pass touches no resource, so the GPU may start it before the render pass recorded ahead of it
// has finished (nothing orders them); a stage's own number is then too small and the time shows in a later stage's (the
// first that waits for the image, typically the post chain). The submit's span, and so the frame's total, is exact; for
// the cost of one pass, A/B the frame total, or time the passes themselves with `NFS_GPU_PROFILE=1`.

const MAX_MARKS: u32 = 1024;

struct Marks {
    qs: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    /// a one-thread no-op dispatched in each mark pass (a pass with no work in it is not run, and writes no timestamp)
    nop: wgpu::ComputePipeline,
    state: Mutex<MarkState>,
}

#[derive(Default)]
struct MarkState {
    /// the marks recorded this frame by query index: `(label, context)`
    labels: Vec<(&'static str, &'static str)>,
    /// the marks recorded since the last submit (they run in it)
    pending: Vec<usize>,
    /// the query indices in the order the GPU runs them: per submit `SEG_BEGIN`, the submit's marks, `SEG_END`
    order: Vec<usize>,
    finish_ns: u64,
    submit_ns: u64,
}

const SEG_BEGIN: &str = "\u{1}begin";
const SEG_END: &str = "\u{1}end";

static MARKS: OnceLock<Marks> = OnceLock::new();
static MARKS_ON: AtomicBool = AtomicBool::new(false);

/// One bench frame's numbers (see [`bench_frame_end`]).
#[derive(Debug, Clone, Default)]
pub struct BenchSample {
    /// the GPU's busy time: the sum of the frame's submit segments, ms (`None`: no timestamp queries)
    pub gpu_ms: Option<f64>,
    /// the CPU's: the frame's start to its end before the wait for the device, ms
    pub cpu_ms: f64,
    /// ... of which in `CommandEncoder::finish` / `Queue::submit`, ms
    pub finish_ms: f64,
    pub submit_ms: f64,
    /// the CPU time + the wait for the GPU (what `NFS_GPU_TIME` reports), ms
    pub wall_ms: f64,
    /// each stage's GPU ms (the time from the mark before it to its own; label + context), in first-seen order
    pub stages: Vec<(String, f64)>,
}

/// Turn the marks on (built at first use); `false` when the device has no timestamp queries (the bench then times the CPU only).
pub(crate) fn marks_begin(device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
    if minimal() || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return false;
    }
    let m = MARKS.get_or_init(|| {
        let bytes = MAX_MARKS as u64 * 8;
        Marks {
            qs: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("bench marks"), ty: wgpu::QueryType::Timestamp, count: MAX_MARKS }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor { label: Some("bench marks resolve"), size: bytes, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false }),
            readback: device.create_buffer(&wgpu::BufferDescriptor { label: Some("bench marks readback"), size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
            period_ns: queue.get_timestamp_period() as f64,
            nop: {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bench mark"), source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()) });
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("bench mark"), layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None })
            },
            state: Mutex::new(MarkState::default()),
        }
    });
    if let Ok(mut s) = m.state.lock() {
        *s = MarkState::default();
    }
    MARKS_ON.store(true, Relaxed);
    true
}

/// Turn the marks off.
pub(crate) fn marks_end() {
    MARKS_ON.store(false, Relaxed);
}

/// Record a mark pass into `encoder`; its query index (`None`: the frame's marks are used up).
fn mark_raw(encoder: &mut wgpu::CommandEncoder, label: &'static str, ctx: &'static str) -> Option<usize> {
    let m = MARKS.get()?;
    let mut s = m.state.lock().ok()?;
    let i = s.labels.len();
    if i >= MAX_MARKS as usize {
        return None;
    }
    s.labels.push((label, ctx));
    drop(s);
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("bench mark"),
        timestamp_writes: Some(wgpu::ComputePassTimestampWrites { query_set: &m.qs, beginning_of_pass_write_index: Some(i as u32), end_of_pass_write_index: None }),
    });
    pass.set_pipeline(&m.nop);
    pass.dispatch_workgroups(1, 1, 1);
    Some(i)
}

/// TEST GLUE (`--perf-bench`): the stage marks of a frame ([`mark`]), and the profiler's names of the frame core's passes
/// ([`rp`]); [`Stage::label`] is the name the bench prints (`PERFSTAGE`) and `NFS_GPU_PROFILE` logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Uploads,
    SunShadow,
    LampPairShadow,
    StreetLamps,
    ContactShadow,
    LensReference,
    MainPass,
    SoftDepthAo,
    MainPassBlended,
    DepthConversion2,
    Shafts,
    LighthouseBeam,
    Haze,
    LensTest,
    Overlay,
    Sprites,
    TemporalPass,
    PostChain,
    ToneMap,
    Upscaler,
    UpscalePresent,
}

impl Stage {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Stage::Uploads => "uploads",
            Stage::SunShadow => "E9 sun shadow",
            Stage::LampPairShadow => "E15 lamp-pair shadow",
            Stage::StreetLamps => "E23/E47 street lamps",
            Stage::ContactShadow => "E43 contact shadow",
            Stage::LensReference => "lens reference",
            Stage::MainPass => "main pass",
            Stage::SoftDepthAo => "E12/E14 soft depth + AO",
            Stage::MainPassBlended => "main pass blended (split)",
            Stage::DepthConversion2 => "depth conversion 2 (E13/E36)",
            Stage::Shafts => "E13 shafts",
            Stage::LighthouseBeam => "E44 lighthouse beam",
            Stage::Haze => "E36 haze",
            Stage::LensTest => "lens test",
            Stage::Overlay => "overlay",
            Stage::Sprites => "sprites",
            Stage::TemporalPass => "temporal pass",
            Stage::PostChain => "post chain",
            Stage::ToneMap => "tone map (+E35 SMAA)",
            Stage::Upscaler => "E34 upscaler",
            Stage::UpscalePresent => "upscale present (+E39 RCAS)",
        }
    }
}

/// A stage mark: the GPU work recorded since the mark before it is `stage`'s (in the context [`ctx`] set, e.g. the mirror).
#[inline]
pub(crate) fn mark(encoder: &mut wgpu::CommandEncoder, stage: Stage) {
    let label = stage.label();
    if MARKS_ON.load(Relaxed) {
        let ctx = PROF.get().and_then(|p| p.state.lock().ok().map(|s| s.ctx)).unwrap_or("");
        if let Some(i) = mark_raw(encoder, label, ctx)
            && let Some(m) = MARKS.get()
            && let Ok(mut s) = m.state.lock()
        {
            s.pending.push(i);
        }
    }
}

/// `encoder.finish()`, its CPU time added up while the marks are on.
#[inline]
pub(crate) fn finish(encoder: wgpu::CommandEncoder) -> wgpu::CommandBuffer {
    if !MARKS_ON.load(Relaxed) {
        return encoder.finish();
    }
    let t = std::time::Instant::now();
    let cb = encoder.finish();
    if let Some(m) = MARKS.get() && let Ok(mut s) = m.state.lock() {
        s.finish_ns += t.elapsed().as_nanos() as u64;
    }
    cb
}

/// `queue.submit(cbs)`; while the marks are on, between a segment's begin and end marks (and its CPU time added up).
pub(crate) fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(device: &wgpu::Device, queue: &wgpu::Queue, cbs: I) -> wgpu::SubmissionIndex {
    SUBMITS.fetch_add(1, Relaxed);
    if !MARKS_ON.load(Relaxed) {
        return queue.submit(cbs);
    }
    let seg = |label| {
        let mut e = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bench mark") });
        let i = mark_raw(&mut e, label, "");
        (e.finish(), i)
    };
    let (b, bi) = seg(SEG_BEGIN);
    let mut all = vec![b];
    all.extend(cbs);
    let (e, ei) = seg(SEG_END);
    all.push(e);
    if let (Some(bi), Some(ei), Some(m)) = (bi, ei, MARKS.get())
        && let Ok(mut s) = m.state.lock()
    {
        let pending = std::mem::take(&mut s.pending);
        s.order.push(bi);
        s.order.extend(pending);
        s.order.push(ei);
    }
    let t = std::time::Instant::now();
    let idx = queue.submit(all);
    if let Some(m) = MARKS.get() && let Ok(mut s) = m.state.lock() {
        s.submit_ns += t.elapsed().as_nanos() as u64;
    }
    idx
}

/// The bench frame's start: the frame's marks reset, the CPU clock started.
pub(crate) fn bench_frame_begin() -> std::time::Instant {
    if let Some(m) = MARKS.get() && let Ok(mut s) = m.state.lock() {
        *s = MarkState::default();
    }
    std::time::Instant::now()
}

/// The bench frame's end: the CPU time, then the wait for the device, the marks read back and added up per stage.
pub(crate) fn bench_frame_end(device: &wgpu::Device, queue: &wgpu::Queue, t0: std::time::Instant) -> BenchSample {
    let cpu_ms = elapsed_ms(t0);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let wall_ms = elapsed_ms(t0);
    let mut out = BenchSample { cpu_ms, wall_ms, ..Default::default() };
    let Some(m) = MARKS.get().filter(|_| MARKS_ON.load(Relaxed)) else { return out };
    let (labels, order, finish_ns, submit_ns) = match m.state.lock() {
        Ok(mut s) => (std::mem::take(&mut s.labels), std::mem::take(&mut s.order), s.finish_ns, s.submit_ns),
        Err(_) => return out,
    };
    out.finish_ms = finish_ns as f64 / NS_PER_MS;
    out.submit_ms = submit_ns as f64 / NS_PER_MS;
    let n = labels.len() as u32;
    if n == 0 {
        out.gpu_ms = Some(0.0);
        return out;
    }
    let mut e = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bench marks resolve") });
    e.resolve_query_set(&m.qs, 0..n, &m.resolve, 0);
    e.copy_buffer_to_buffer(&m.resolve, 0, &m.readback, 0, n as u64 * 8);
    queue.submit(Some(e.finish()));
    let slice = m.readback.slice(..n as u64 * 8);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let Ok(view) = slice.get_mapped_range() else { return out };
    let ts: Vec<u64> = bytemuck::cast_slice::<u8, u64>(&view).to_vec();
    drop(view);
    m.readback.unmap();
    let ms = |a: u64, b: u64| if b > a { (b - a) as f64 * m.period_ns / NS_PER_MS } else { 0.0 };
    let mut stages: Vec<(String, f64)> = Vec::new();
    let mut add = |name: String, v: f64| match stages.iter_mut().find(|(n, _)| *n == name) {
        Some(s) => s.1 += v,
        None => stages.push((name, v)),
    };
    // the submits run in order, but a submit's begin mark can run while the one before is still busy (it waits for
    // nothing): a segment starts at its begin mark or the end of the one before, whichever is later (the union of the
    // spans: nothing is counted twice)
    let (mut total, mut begin, mut last, mut prev_end) = (0.0, None::<u64>, 0u64, 0u64);
    for &i in &order {
        let ((label, ctx), t) = (labels[i], ts[i]);
        if label == SEG_BEGIN {
            let b = t.max(prev_end);
            begin = Some(b);
            last = b;
            continue;
        }
        let Some(b) = begin else { continue };
        // a mark the GPU reached before the one recorded ahead of it (overlapping passes) costs 0, the later one takes it
        let d = ms(last, t);
        last = last.max(t);
        if label == SEG_END {
            add(format!("(rest){ctx}"), d);
            total += ms(b, last);
            prev_end = prev_end.max(last);
            begin = None;
        } else {
            add(format!("{label}{ctx}"), d);
        }
    }
    out.gpu_ms = Some(total);
    out.stages = stages;
    out
}
