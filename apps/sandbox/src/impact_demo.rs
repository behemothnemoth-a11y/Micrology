//! Automated visual acceptance fixture for DROP 0004.8.
//!
//! Enabled only with MICROLOGY_IMPACT_DEMO=1.

use crate::destruction::DestructionHost;
use crate::fragment_streaming::{FragmentStreamRes, FragmentStreamTasks};
use crate::impact::ImpactHost;
use crate::physics::DynamicFragments;
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::app::AppExit;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use engine_core::{CellBounds, CellPos, GlobalPos};
use engine_destruction::{
    DamageAmount, DamageEvent, DamageEventId, DamageImpulse, DamageSpace, DamageVolume,
    SecondaryDamage,
};
use std::path::PathBuf;

#[derive(Resource, Default)]
pub struct ImpactDemo {
    enabled: bool,
    elapsed: f32,
    fragment_seen_at: Option<f32>,
    local_cut_sent: bool,
    baseline_static_failed: u64,
    success_at: Option<f32>,
}

#[derive(SystemParam)]
pub struct ImpactDemoSeedResources<'w> {
    world: ResMut<'w, WorldRes>,
    stream: ResMut<'w, StreamRes>,
    destruction: ResMut<'w, DestructionHost>,
    impact: ResMut<'w, ImpactHost>,
    fragments: ResMut<'w, DynamicFragments>,
    fragment_stream: ResMut<'w, FragmentStreamRes>,
    fragment_tasks: ResMut<'w, FragmentStreamTasks>,
    demo: ResMut<'w, ImpactDemo>,
    status: ResMut<'w, StatusLine>,
}

pub fn seed(resources: ImpactDemoSeedResources) {
    if std::env::var_os("MICROLOGY_IMPACT_DEMO").is_none() {
        return;
    }
    let ImpactDemoSeedResources {
        mut world,
        mut stream,
        mut destruction,
        mut impact,
        mut fragments,
        mut fragment_stream,
        mut fragment_tasks,
        mut demo,
        mut status,
    } = resources;

    let materials = world.0.materials().clone();
    world.0 = engine_world::World::with_materials(materials);
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();
    stream.dir = PathBuf::from("target/drop0004_8_demo_world");
    fragments.replace_store(Default::default());
    fragment_stream.reset(Default::default());
    fragment_tasks.clear();
    *destruction = DestructionHost::default();
    *impact = ImpactHost::default();

    // Anchored floor.
    world.0.fill_box(
        CellPos::new(-24, 4, -9),
        CellPos::new(28, 4, 9),
        Some(crate::scene::STONE),
    );
    world
        .0
        .set_anchor_box(CellPos::new(-24, 4, -9), CellPos::new(28, 4, 9), true);
    // Narrow support plus a long top bar. Cutting the support detaches the bar.
    world.0.fill_box(
        CellPos::new(-4, 5, -1),
        CellPos::new(-2, 12, 1),
        Some(crate::scene::STONE),
    );
    world.0.fill_box(
        CellPos::new(-11, 13, -2),
        CellPos::new(8, 15, 2),
        Some(crate::scene::BRICK),
    );

    // Breakable wall in the launch direction.
    world.0.fill_box(
        CellPos::new(18, 5, -7),
        CellPos::new(19, 20, 7),
        Some(crate::scene::BRICK),
    );
    world.0.mark_all_dirty();

    stream.meta.spawn = [1.0, 15.0, 42.0];
    stream.meta.look_at = Some([5.0, 13.0, 0.0]);

    let cut = DamageEvent::new(
        DamageEventId::new(8000, 0),
        DamageSpace::StaticWorld,
        Some(GlobalPos::new(-8.0, 11.0, 0.0)),
        DamageVolume::Box(CellBounds::new(
            CellPos::new(-4, 9, -1),
            CellPos::new(-2, 10, 1),
        )),
        DamageAmount(18),
        Some(DamageImpulse::new([2600.0, 4500.0, 0.0]).unwrap()),
    )
    .unwrap();
    impact.enqueue(SecondaryDamage::root(cut));
    demo.enabled = true;
    status.0 = "0004.8 demo: damage cut queued with launch impulse".into();
}

pub fn drive(
    time: Res<Time>,
    fragments: Res<DynamicFragments>,
    destruction: Res<DestructionHost>,
    mut impact: ResMut<ImpactHost>,
    mut demo: ResMut<ImpactDemo>,
    mut status: ResMut<StatusLine>,
    mut exits: MessageWriter<AppExit>,
) {
    if !demo.enabled {
        return;
    }
    demo.elapsed += time.delta_secs();

    if !fragments.store_ref().is_empty() && demo.fragment_seen_at.is_none() {
        demo.fragment_seen_at = Some(demo.elapsed);
        demo.baseline_static_failed = impact.stats().static_failed_cells;
    }

    if !demo.local_cut_sent
        && demo
            .fragment_seen_at
            .is_some_and(|seen| demo.elapsed - seen >= 0.8)
        && let Some((id, fragment)) = fragments.iter().next()
    {
        let mid_x = (fragment.bounds.min.x + fragment.bounds.max.x) / 2;
        let local_cut = DamageEvent::new(
            DamageEventId::new(8001, 0),
            DamageSpace::FragmentLocal(id),
            None,
            DamageVolume::Box(CellBounds::new(
                CellPos::new(mid_x, fragment.bounds.min.y, fragment.bounds.min.z),
                CellPos::new(mid_x, fragment.bounds.max.y, fragment.bounds.max.z),
            )),
            DamageAmount(18),
            None,
        )
        .unwrap();
        let generation = impact.generation(id);
        impact.enqueue(SecondaryDamage {
            event: local_cut,
            generation,
        });
        demo.local_cut_sent = true;
        status.0 = format!("0004.8 demo: fragment {id} local cut queued");
    }

    let stats = impact.stats();
    let secondary_wall_damage =
        stats.static_failed_cells > demo.baseline_static_failed && stats.collisions_sampled > 0;
    if demo.local_cut_sent && stats.refractures > 0 && secondary_wall_damage {
        let now = demo.elapsed;
        let success_at = *demo.success_at.get_or_insert(now);
        status.0 = format!(
            "0004.8 complete: refractures {} -> {} children | impacts {} | secondary failed cells {}",
            stats.refractures,
            stats.refracture_children,
            stats.collisions_sampled,
            stats.static_failed_cells - demo.baseline_static_failed
        );
        if demo.elapsed - success_at >= 3.0 {
            info!(
                "0004.8 demo PASSED: refractures={} children={} impacts={} secondary_failed={} total_static_failed={} fragment_failed={}",
                stats.refractures,
                stats.refracture_children,
                stats.collisions_sampled,
                stats.static_failed_cells - demo.baseline_static_failed,
                stats.static_failed_cells,
                stats.fragment_failed_cells,
            );
            exits.write(AppExit::Success);
        }
        return;
    }

    if demo.elapsed > 14.0 {
        let structural = destruction.stats();
        panic!(
            "0004.8 demo timed out: fragments={} refractures={} impacts={} static_failed={} baseline={} structural requested={} pending={} active={} stale={} inconclusive={} rejected={} detached_tx={} detached_cells={}",
            fragments.len(),
            stats.refractures,
            stats.collisions_sampled,
            stats.static_failed_cells,
            demo.baseline_static_failed,
            structural.requested,
            structural.pending_requests,
            structural.active_jobs,
            structural.stale,
            structural.inconclusive,
            structural.rejected_by_policy,
            structural.detached_transactions,
            structural.cells_detached,
        );
    }
}
