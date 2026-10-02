//! Bounded lab telemetry. Wall clocks are diagnostics only and never choose
//! fracture outcomes or enter a deterministic state digest.
use crate::{
    contact_fracture::ContactFractureHost, physics::DynamicFragments, sim_lab::SimulationLab,
};
use bevy::prelude::*;
use std::collections::VecDeque;

const MAX_SAMPLES: usize = 12_000;

#[derive(Resource, Default)]
pub struct PerformanceCapture {
    frames_ms: VecDeque<f64>,
    contact_ms: VecDeque<f64>,
    active_frames_ms: VecDeque<f64>,
    previous_frame_had_step: bool,
    last_fixed_tick: u64,
    max_fragments: usize,
    max_awake: usize,
    written: bool,
}
impl PerformanceCapture {
    pub fn contact(&mut self, ms: f64) {
        if self.contact_ms.len() == MAX_SAMPLES {
            self.contact_ms.pop_front();
        }
        self.contact_ms.push_back(ms);
    }
}
fn distribution(values: &VecDeque<f64>) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    let mut sorted: Vec<_> = values.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: usize| sorted[(sorted.len() * p).div_ceil(100).saturating_sub(1)];
    serde_json::json!({ "count": sorted.len(), "median_ms": percentile(50), "p95_ms": percentile(95), "p99_ms": percentile(99), "max_ms": sorted[sorted.len()-1], "frames_over_33ms": sorted.iter().filter(|v| **v > 33.3).count(), "frames_over_50ms": sorted.iter().filter(|v| **v > 50.).count() })
}
pub fn record(
    real: Res<Time<Real>>,
    lab: Res<SimulationLab>,
    fragments: Res<DynamicFragments>,
    contacts: Res<ContactFractureHost>,
    mut capture: ResMut<PerformanceCapture>,
    mut events: ParamSet<(MessageReader<AppExit>, MessageWriter<AppExit>)>,
) {
    if !lab.enabled {
        return;
    }
    // Exclude initial shader/asset startup. First scripted impact is after 5s.
    if real.elapsed_secs_f64() >= 5. {
        if capture.frames_ms.len() == MAX_SAMPLES {
            capture.frames_ms.pop_front();
        }
        capture.frames_ms.push_back(real.delta_secs_f64() * 1000.);
        // A frame-start interval contains the PREVIOUS frame's work.
        if capture.previous_frame_had_step {
            if capture.active_frames_ms.len() == MAX_SAMPLES {
                capture.active_frames_ms.pop_front();
            }
            capture
                .active_frames_ms
                .push_back(real.delta_secs_f64() * 1000.);
        }
        capture.max_fragments = capture.max_fragments.max(fragments.len());
        capture.max_awake = capture.max_awake.max(fragments.stats().dynamic as usize);
    }
    capture.previous_frame_had_step = lab.fixed_ticks != capture.last_fixed_tick;
    capture.last_fixed_tick = lab.fixed_ticks;
    let stop = std::env::var("MICROLOGY_PROFILE_SECONDS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 10.);
    let timed_out = stop.is_some_and(|s| real.elapsed_secs_f64() >= s);
    if capture.written || (!timed_out && events.p0().read().next().is_none()) {
        return;
    }
    let report = serde_json::json!({
        "schema": 1, "elapsed_seconds": real.elapsed_secs_f64(), "warmup_seconds": 5,
        "scope": "Real frame intervals, VSync may cap throughput; no GPU timer; recording affects timings",
        "frames": distribution(&capture.frames_ms), "physics_active_frames": distribution(&capture.active_frames_ms), "contact_batches": distribution(&capture.contact_ms),
        "maximum_fragments": capture.max_fragments, "maximum_awake": capture.max_awake,
        "interaction": lab.interaction.snapshot(),
        "contacts": contacts.snapshot(), "frame_intervals_ms": capture.frames_ms, "physics_active_intervals_ms": capture.active_frames_ms, "contact_batches_ms": capture.contact_ms,
    });
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/diagnostics");
    let written = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::write(
            dir.join("integrated-profile.json"),
            serde_json::to_vec_pretty(&report).expect("finite diagnostic data"),
        )
    });
    if let Err(error) = written {
        error!("profile write failed: {error}");
    }
    capture.written = true;
    if timed_out {
        events.p1().write(AppExit::Success);
    }
}

/// Optional renderer-owned evidence, unaffected by other desktop windows.
/// At most one readback is outstanding. This is off during timing runs.
pub fn capture_snapshot(
    mut commands: Commands,
    lab: Res<SimulationLab>,
    captures: Query<(), With<bevy::render::view::screenshot::Screenshot>>,
    mut last: Local<Option<std::path::PathBuf>>,
) {
    if !lab.enabled
        || std::env::var_os("MICROLOGY_NATIVE_CAPTURE").is_none()
        || !captures.is_empty()
    {
        return;
    }
    let Some(path) = &lab.last_dump else {
        return;
    };
    if last.as_ref() == Some(path) {
        return;
    }
    commands
        .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
        .observe(bevy::render::view::screenshot::save_to_disk(
            path.with_extension("png"),
        ));
    *last = Some(path.clone());
}
