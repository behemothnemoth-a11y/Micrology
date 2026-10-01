//! Host wiring: turning the engine's streaming decisions into real work.
//!
//! The engine decides *what* should happen — which regions to load, which to
//! save, which sections to compile — and knows nothing about threads or Bevy.
//! This module carries those decisions out on Bevy's async compute pool and
//! reports the results back.
//!
//! The important property is that **no ordinary path compiles a mesh on the
//! main schedule**. A dirty section is queued, dispatched to a worker, and
//! applied when it returns — and only if the world has not moved underneath it.
//!
//! Three task kinds are kept separate, because they have different staleness
//! rules and different failure modes: region loads, region saves, mesh compiles.

use crate::camera::FlyCamera;
use crate::render::SectionEntities;
use crate::{GeometryRes, StatusLine, WorldRes};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use engine_core::{CellPos, RegionPos, RenderSectionId, SectionGrid};
use engine_geometry::CachedSection;
use engine_io::{WorldMeta, v2};
use engine_stream::{
    LoadTicket, MeshScheduler, MeshUrgency, RegionLoad, RegionStreamer, SaveTicket,
    SchedulerLimits, SectionPriority, StreamAction, StreamingConfig,
};
use std::path::PathBuf;

/// Regions within this Chebyshev distance count as "required", so their
/// geometry outranks speculative work.
const REQUIRED_REGION_RADIUS: i32 = 1;

/// Streaming state for the running world.
#[derive(Resource)]
pub struct StreamRes {
    pub streamer: RegionStreamer,
    pub scheduler: MeshScheduler,
    pub dir: PathBuf,
    pub meta: WorldMeta,
    /// Whether streaming is driving residency. Off when the sandbox was given a
    /// single world file to look at rather than a directory to stream.
    pub enabled: bool,
}

impl StreamRes {
    pub fn new(dir: PathBuf, meta: WorldMeta) -> Self {
        let mut streamer = RegionStreamer::new(StreamingConfig::default());
        // Deliberately modest: a budget nobody ever reaches is a budget that
        // was never tested.
        streamer.set_budget(engine_stream::MemoryBudget::with_ceiling(96 * 1024 * 1024));
        Self {
            streamer,
            scheduler: MeshScheduler::new(SchedulerLimits {
                max_active_mesh_jobs: 6,
                max_results_applied_per_tick: 4,
                ..SchedulerLimits::default()
            }),
            dir,
            meta,
            enabled: true,
        }
    }

    /// A non-streaming session: everything is already resident.
    pub fn resident_only(dir: PathBuf, meta: WorldMeta) -> Self {
        Self {
            enabled: false,
            ..Self::new(dir, meta)
        }
    }
}

/// Work handed to the async compute pool.
///
/// Each task carries the bytes it owns, because in-flight work counts against
/// the memory budget just as resident data does — a budget that ignores the
/// queue is one that scheduling alone can blow.
#[derive(Resource, Default)]
pub struct StreamTasks {
    loads: Vec<(LoadTicket, Task<RegionLoad>)>,
    saves: Vec<(SaveTicket, u64, Task<bool>)>,
    meshes: Vec<(u64, Task<engine_stream::MeshJobResult>)>,
}

impl StreamTasks {
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.loads.len(), self.saves.len(), self.meshes.len())
    }

    /// Bytes held by mesh job snapshots currently compiling.
    pub fn mesh_snapshot_bytes(&self) -> u64 {
        self.meshes.iter().map(|(bytes, _)| bytes).sum()
    }

    /// Bytes held by region snapshots currently being written.
    pub fn io_snapshot_bytes(&self) -> u64 {
        self.saves.iter().map(|(_, bytes, _)| bytes).sum()
    }
}

/// Measure where memory is going, so the budget has something real to act on.
///
/// The pieces live in different places — the world owns cells, the mesh cache
/// owns compiled geometry, the scheduler and the task list own work in flight —
/// so only the host can add them up.
pub fn measure_memory(
    world: &WorldRes,
    geometry: &GeometryRes,
    stream: &StreamRes,
    tasks: &StreamTasks,
) -> engine_stream::MemoryAccount {
    let footprint = world.0.footprint();
    engine_stream::MemoryAccount {
        volume_bytes: footprint.cell_bytes,
        palette_bytes: footprint.palette_bytes,
        cpu_mesh_bytes: geometry.cache.stats().mesh_bytes,
        mesh_snapshot_bytes: tasks.mesh_snapshot_bytes(),
        awaiting_apply_bytes: stream.scheduler.awaiting_apply_bytes(),
        io_snapshot_bytes: tasks.io_snapshot_bytes(),
    }
}

/// Decide residency, and start the region I/O it needs.
pub fn drive_streaming(
    mut commands: Commands,
    mut stream: ResMut<StreamRes>,
    mut world: ResMut<WorldRes>,
    mut tasks: ResMut<StreamTasks>,
    mut geometry: ResMut<GeometryRes>,
    mut sections: ResMut<SectionEntities>,
    camera: Option<Single<&FlyCamera>>,
) {
    if !stream.enabled {
        return;
    }
    let Some(camera) = camera else {
        return;
    };
    let camera_region = camera.global.cell().region();

    // Tell the engine what memory is actually being used before it decides
    // what else to fetch.
    let account = measure_memory(&world, &geometry, &stream, &tasks);
    stream.streamer.note_memory(account);

    let pool = AsyncComputeTaskPool::get();
    let actions = stream.streamer.update(&mut world.0, camera_region);

    for action in actions {
        match action {
            StreamAction::LoadRegion(ticket) => {
                let dir = stream.dir.clone();
                let region = ticket.region;
                tasks
                    .loads
                    .push((ticket, pool.spawn(async move { read_region(&dir, region) })));
            }
            StreamAction::SaveRegion(ticket) => {
                // The region is cloned so the worker owns its data and the main
                // thread can keep editing the live one.
                let Some(snapshot) = world.0.region(ticket.region).cloned() else {
                    continue;
                };
                let dir = stream.dir.clone();
                let region = ticket.region;
                let bytes = snapshot.footprint().total();
                tasks.saves.push((
                    ticket,
                    bytes,
                    pool.spawn(async move { v2::save_region(&dir, region, &snapshot).is_ok() }),
                ));
            }
            StreamAction::DropRegion(region) => {
                tear_down_region(
                    &mut commands,
                    &mut stream,
                    &mut world,
                    &mut geometry,
                    &mut sections,
                    region,
                );
            }
        }
    }
}

fn read_region(dir: &std::path::Path, region: RegionPos) -> RegionLoad {
    match v2::load_region(dir, region) {
        Ok(region) => RegionLoad::Loaded(region),
        // No shard is normal for terrain nobody has built in; a shard that
        // exists but cannot be read is not, and must never be quietly treated
        // as empty.
        Err(engine_io::IoError::MissingRegion { .. }) => RegionLoad::Absent,
        Err(_) => RegionLoad::Failed,
    }
}

/// Drop a region and everything the renderer was holding for it.
///
/// Evicting world data is not enough: the mesh cache entry, the Bevy mesh asset
/// and the entity all have to go, or travelling leaves a trail of dead meshes.
fn tear_down_region(
    commands: &mut Commands,
    stream: &mut StreamRes,
    world: &mut WorldRes,
    geometry: &mut GeometryRes,
    sections: &mut SectionEntities,
    region: RegionPos,
) {
    let grid = geometry.cache.grid();
    // Collect the sections before the region goes away.
    let doomed: Vec<RenderSectionId> = world
        .0
        .region(region)
        .map(|r| r.volume_positions().map(|v| grid.section_for(v)).collect())
        .unwrap_or_default();

    stream.streamer.drop_region(&mut world.0, region);

    for section in doomed {
        // A section may be shared with a neighbouring region when the grid
        // groups volumes; only tear it down if nothing resident still needs it.
        if grid
            .volumes_in(section)
            .any(|volume| world.0.has_volume(volume))
        {
            continue;
        }
        stream.scheduler.cancel(section);
        geometry.cache.remove(section);
        sections.despawn(commands, section);
    }
}

/// Collect finished region I/O and tell the engine about it.
pub fn poll_region_tasks(
    mut stream: ResMut<StreamRes>,
    mut world: ResMut<WorldRes>,
    mut tasks: ResMut<StreamTasks>,
    mut status: ResMut<StatusLine>,
) {
    let mut finished_loads = Vec::new();
    tasks
        .loads
        .retain_mut(|(ticket, task)| match check_ready(task) {
            Some(result) => {
                finished_loads.push((*ticket, result));
                false
            }
            None => true,
        });
    for (ticket, result) in finished_loads {
        if result == RegionLoad::Failed {
            status.0 = format!("region {:?} failed to load", ticket.region);
        }
        stream
            .streamer
            .on_load_finished(&mut world.0, ticket, result);
    }

    let mut finished_saves = Vec::new();
    tasks
        .saves
        .retain_mut(|(ticket, _, task)| match check_ready(task) {
            Some(ok) => {
                finished_saves.push((*ticket, ok));
                false
            }
            None => true,
        });
    for (ticket, ok) in finished_saves {
        if !ok {
            status.0 = format!("region {:?} failed to save", ticket.region);
        }
        stream.streamer.on_save_finished(&mut world.0, ticket, ok);
    }
}

/// Turn dirty volumes into prioritised mesh requests.
pub fn queue_dirty_sections(
    mut stream: ResMut<StreamRes>,
    mut world: ResMut<WorldRes>,
    geometry: Res<GeometryRes>,
    camera: Option<Single<&FlyCamera>>,
) {
    let dirty = world.take_dirty();
    if dirty.is_empty() {
        return;
    }
    let grid = geometry.cache.grid();
    let camera_cell = camera.map(|c| c.global.cell()).unwrap_or(CellPos::ZERO);
    let camera_region = camera_cell.region();

    for volume in dirty {
        let section = grid.section_for(volume);
        let priority = priority_for(grid, section, camera_cell, camera_region);
        stream.scheduler.request(section, priority);
    }
}

/// Classify a section by where it is relative to the camera.
///
/// The host decides urgency because only it knows what is near; the scheduler
/// orders within that.
fn priority_for(
    grid: SectionGrid,
    section: RenderSectionId,
    camera_cell: CellPos,
    camera_region: RegionPos,
) -> SectionPriority {
    let centre = grid.cell_bounds(section).center_f32();
    let dx = centre[0] - camera_cell.x as f32;
    let dy = centre[1] - camera_cell.y as f32;
    let dz = centre[2] - camera_cell.z as f32;
    let distance_squared = (dx * dx + dy * dy + dz * dz).max(0.0) as u64;

    let region = grid.origin_volume(section).region();
    let urgency = if camera_region.chebyshev_distance(region) <= REQUIRED_REGION_RADIUS {
        MeshUrgency::RequiredDirty
    } else {
        MeshUrgency::DirtyOutOfView
    };
    SectionPriority::new(urgency, distance_squared)
}

/// Send the most important queued work to the compute pool.
pub fn dispatch_mesh_jobs(
    mut stream: ResMut<StreamRes>,
    world: Res<WorldRes>,
    geometry: Res<GeometryRes>,
    mut tasks: ResMut<StreamTasks>,
) {
    let grid = geometry.cache.grid();
    let compiler = geometry.compiler;
    let pool = AsyncComputeTaskPool::get();
    stream.scheduler.begin_tick();

    for job in stream.scheduler.take_jobs(&world.0, grid) {
        // The choice travels with the job because the job is compiled on a
        // worker; a trait object would need to be Send + Sync and shared.
        let bytes = job.snapshot_bytes();
        tasks
            .meshes
            .push((bytes, pool.spawn(async move { compiler.compile(&job) })));
    }
}

/// Collect finished geometry and apply what is still current.
/// The renderer-side handles applying a result needs, grouped so the system
/// stays within a readable argument count.
#[derive(bevy::ecs::system::SystemParam)]
pub struct UploadTargets<'w> {
    pub sections: ResMut<'w, SectionEntities>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub materials: ResMut<'w, Assets<StandardMaterial>>,
    pub origin: Res<'w, crate::render::RenderOriginRes>,
}

pub fn apply_mesh_results(
    mut commands: Commands,
    mut stream: ResMut<StreamRes>,
    world: Res<WorldRes>,
    mut geometry: ResMut<GeometryRes>,
    mut tasks: ResMut<StreamTasks>,
    mut targets: UploadTargets,
) {
    let mut finished = Vec::new();
    tasks
        .meshes
        .retain_mut(|(_, task)| match check_ready(task) {
            Some(result) => {
                finished.push(result);
                false
            }
            None => true,
        });
    for result in finished {
        stream.scheduler.deliver(result);
    }

    let grid = geometry.cache.grid();
    let ready = stream.scheduler.drain_ready(&world.0, grid);
    if ready.is_empty() {
        return;
    }

    for result in ready {
        let section = result.section;
        geometry.cache.insert(
            section,
            CachedSection {
                quads: result.quads,
                mesh: result.mesh,
            },
        );
        let cached = geometry.cache.get(section).expect("just inserted");
        targets.sections.upload(
            &mut commands,
            &mut targets.meshes,
            &mut targets.materials,
            &targets.origin.0,
            section,
            cached,
        );
    }
}
