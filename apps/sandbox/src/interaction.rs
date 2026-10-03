//! Optional lab tools. Damage stays in engine transactions; rotation, picking,
//! force application and input belong to the host. Queues never own geometry.
use crate::{
    StatusLine, WorldRes,
    destruction::DestructionHost,
    fragment_render::FragmentEntities,
    physics::{DynamicFragmentBody, DynamicFragments, FragmentBodies, FragmentBudgetRes},
    sim_lab::{SimulationLab, fits_fragment_storage},
};
use avian3d::{
    math::{Quaternion, Vector},
    prelude::{
        AngularVelocity, ComputedMass, Forces, LinearVelocity, ReadRigidBodyForces, Sleeping,
        WriteRigidBodyForces,
    },
};
use bevy::{ecs::system::SystemParam, prelude::*};
use engine_core::{GlobalPos, Revision};
use engine_destruction::{
    AllResident, DamageAmount, DamageSpace, FractureImpact, FractureTransactionLimits, Fragment,
    FragmentDamageResult, FragmentId,
    interaction::{GrabPolicy, bounded_vector, radial_velocity},
};
use std::collections::{BTreeSet, VecDeque};

const MAX_BLAST_TARGETS: usize = 32;
const MAX_QUERY_FRAGMENTS: usize = 4096;
const TARGETS_PER_FRAME: usize = 2;

/// Explicit lab authoring command. Ordinary anchored cells provide the ground;
/// the original reference fixture and its oracle checks stay unchanged.
pub(crate) fn add_floor(lab: &mut SimulationLab, world: &mut WorldRes) -> Result<String, String> {
    if lab.interaction.floor_added {
        return Ok("arena floor already added".into());
    }
    let cells: Vec<_> = (32..96)
        .flat_map(|z| (32..96).map(move |x| engine_core::CellPos::new(x, -1, z)))
        .collect();
    if cells.iter().any(|c| world.0.get(*c).is_some()) {
        return Err("arena floor overlaps existing cells".into());
    }
    for cell in cells {
        world
            .0
            .set(cell, Some(engine_destruction::REFERENCE_FRACTURE_MATERIAL));
        world.0.set_anchor(cell, true);
    }
    lab.interaction.floor_added = true;
    Ok("one-material arena floor added: 4096 anchored cells".into())
}

#[derive(Clone, Debug)]
struct BlastTarget {
    space: DamageSpace,
    revision: Option<Revision>,
    center: GlobalPos,
    radius: u32,
    energy: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct HeldFragment {
    id: FragmentId,
    local_point: [f64; 3],
    geometry: Revision,
    target: [f64; 3],
    distance: f64,
    follow_camera: bool,
}

#[derive(Default, Debug)]
pub(crate) struct InteractionState {
    floor_added: bool,
    pending: VecDeque<BlastTarget>,
    motion: BTreeSet<FragmentId>,
    held: Option<HeldFragment>,
    blasts: u64,
    applied: u64,
    refused: u64,
    stale: u64,
    capped: u64,
    broken: u64,
    failed: u64,
    created: u64,
    kicks: u64,
    holds: u64,
    throws: u64,
    hold_steps: u64,
    releases: u64,
    work_ms: VecDeque<f64>,
}
impl InteractionState {
    pub fn hud(&self) -> String {
        format!(
            "tools: {} blast cells failed | {} throws | holding {}",
            self.failed,
            self.throws,
            self.held
                .as_ref()
                .map_or_else(|| "none".into(), |h| h.id.to_string())
        )
    }
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"floor_added": self.floor_added, "blasts": self.blasts, "applied": self.applied,
            "refused": self.refused, "stale": self.stale, "capped": self.capped,
            "broken_bonds": self.broken, "failed_cells": self.failed, "fragments_created": self.created,
            "kicks": self.kicks, "holds": self.holds, "throws": self.throws,
            "hold_steps": self.hold_steps, "releases": self.releases,
            "pending": self.pending.len(), "held": self.held.as_ref().map(|h| h.id.to_string()),
            "held_target": self.held.as_ref().map(|h| h.target), "batch_ms": self.work_ms})
    }
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn holding(&self) -> bool {
        self.held.is_some()
    }
}

fn rotation(fragment: &Fragment) -> Option<Quaternion> {
    let q = Quaternion::from_array(fragment.pose.rotation.0);
    (q.is_finite() && q.length_squared() > 1e-12).then(|| q.normalize())
}
fn translation(fragment: &Fragment) -> Vector {
    let p = fragment.pose.translation;
    Vector::new(p.x, p.y, p.z)
}
pub(crate) fn world_center(fragment: &Fragment) -> Option<Vector> {
    Some(translation(fragment) + rotation(fragment)? * Vector::from_array(fragment.local_centre()))
}

/// Closest authoritative material, with static geometry occluding fragments.
pub(crate) fn pick(
    world: &WorldRes,
    fragments: &DynamicFragments,
    origin: GlobalPos,
    direction: [f32; 3],
) -> Option<(engine_world::RayHit, DamageSpace, [f64; 3])> {
    if fragments.len() > MAX_QUERY_FRAGMENTS {
        return None;
    }
    let mut selected = engine_world::raycast(&world.0, origin, direction, 96.)
        .map(|hit| (hit, DamageSpace::StaticWorld, direction.map(f64::from)));
    for (id, fragment) in fragments.iter() {
        let Some(q) = rotation(fragment) else {
            continue;
        };
        // Cheap conservative sphere rejection before voxel traversal.
        let Some(center) = world_center(fragment) else {
            continue;
        };
        let from_world = Vector::new(origin.x, origin.y, origin.z);
        let dir_world = Vector::from_array(direction.map(f64::from)).normalize_or_zero();
        let along = (center - from_world).dot(dir_world).clamp(0., 96.);
        if (center - from_world - dir_world * along).length_squared()
            > fragment.bounding_radius().powi(2)
        {
            continue;
        }
        let from = q.conjugate() * (from_world - translation(fragment));
        let dir = q.conjugate() * dir_world;
        if let Some(hit) = engine_world::raycast(
            fragment,
            GlobalPos::new(from.x, from.y, from.z),
            dir.to_array().map(|v| v as f32),
            96.,
        ) && selected
            .as_ref()
            .is_none_or(|(old, _, _)| hit.distance < old.distance)
        {
            selected = Some((hit, DamageSpace::FragmentLocal(id), dir.to_array()));
        }
    }
    selected
}

/// One event's momentum budget is shared over all surviving parts by mass.
/// Subdivision therefore cannot multiply the directional kick.
pub(crate) fn directional_kick(
    lab: &mut SimulationLab,
    fragments: &mut DynamicFragments,
    ids: &[FragmentId],
    direction: [f64; 3],
    impulse: f64,
) {
    let mass: u64 = ids
        .iter()
        .filter_map(|id| fragments.get(*id))
        .map(Fragment::cell_count)
        .sum();
    if mass == 0 {
        return;
    }
    let Some(delta) = bounded_vector(direction.map(|v| v * impulse / mass as f64), 24.) else {
        return;
    };
    for id in ids {
        kick(lab, fragments, *id, delta);
    }
}

fn kick(
    lab: &mut SimulationLab,
    fragments: &mut DynamicFragments,
    id: FragmentId,
    delta: [f64; 3],
) {
    let Some(f) = fragments.get_mut(id) else {
        return;
    };
    let Some(velocity) = bounded_vector(
        std::array::from_fn(|i| f64::from(f.linear_velocity[i]) + delta[i]),
        80.,
    ) else {
        return;
    };
    engine_destruction::FragmentPhysicsState {
        pose: f.pose,
        linear_velocity: velocity.map(|v| v as f32),
        angular_velocity: f.angular_velocity,
        sleeping: false,
    }
    .apply_to(f);
    lab.interaction.motion.insert(id);
    lab.interaction.kicks += 1;
}

pub(crate) fn survivors(result: &FragmentDamageResult, parent: FragmentId) -> Vec<FragmentId> {
    match result {
        FragmentDamageResult::Destroyed { .. } => vec![],
        FragmentDamageResult::Refractured(split) => split.fragments.iter().map(|f| f.id).collect(),
        _ => vec![parent],
    }
}

/// Capture the bounded target set at trigger time. Newly created debris is not
/// targeted again by the same blast. Deferred fragment work validates geometry.
pub(crate) fn queue_blast(
    lab: &mut SimulationLab,
    fragments: &DynamicFragments,
    center: [i32; 3],
    radius: u32,
    energy: u32,
) -> Result<String, String> {
    if fragments.len() > MAX_QUERY_FRAGMENTS {
        return Err("blast held by candidate-query budget".into());
    }
    if lab.interaction.busy() {
        return Err("previous blast still resolving".into());
    }
    if !(1..=8).contains(&radius) || !(1..=24000).contains(&energy) {
        return Err("invalid blast limits".into());
    }
    let c = center.map(|v| f64::from(v) / 1000.);
    let center = GlobalPos::new(c[0], c[1], c[2]);
    let mut targets = vec![BlastTarget {
        space: DamageSpace::StaticWorld,
        revision: None,
        center,
        radius,
        energy,
    }];
    let mut nearby: Vec<_> = fragments
        .iter()
        .filter_map(|(id, f)| {
            let distance = (world_center(f)? - Vector::from_array(c)).length();
            (distance <= f64::from(radius) + f.bounding_radius()).then_some((
                distance,
                id,
                f.geometry_revision(),
            ))
        })
        .collect();
    nearby.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    lab.interaction.capped += nearby.len().saturating_sub(MAX_BLAST_TARGETS - 1) as u64;
    for (_, id, revision) in nearby.into_iter().take(MAX_BLAST_TARGETS - 1) {
        targets.push(BlastTarget {
            space: DamageSpace::FragmentLocal(id),
            revision: Some(revision),
            center,
            radius,
            energy,
        });
    }
    let count = targets.len();
    lab.interaction.pending.extend(targets);
    lab.interaction.blasts += 1;
    Ok(format!("blast queued: {count} bounded targets"))
}

pub(crate) fn grab_largest(
    lab: &mut SimulationLab,
    fragments: &DynamicFragments,
    target: [i32; 3],
) -> Result<String, String> {
    let (id, f) = fragments
        .iter()
        .max_by(|(a, x), (b, y)| x.cell_count().cmp(&y.cell_count()).then(b.cmp(a)))
        .ok_or("no fragment to grab")?;
    lab.interaction.held = Some(HeldFragment {
        id,
        local_point: f.local_centre(),
        geometry: f.geometry_revision(),
        target: target.map(|v| f64::from(v) / 1000.),
        distance: 12.,
        follow_camera: false,
    });
    lab.interaction.holds += 1;
    Ok(format!("holding native fragment {id}"))
}
pub(crate) fn move_grab(lab: &mut SimulationLab, target: [i32; 3]) -> Result<String, String> {
    let held = lab.interaction.held.as_mut().ok_or("nothing held")?;
    held.target = target.map(|v| f64::from(v) / 1000.);
    Ok("hold target moved".into())
}
pub(crate) fn release(
    lab: &mut SimulationLab,
    fragments: &mut DynamicFragments,
    velocity: [i32; 3],
) -> Result<String, String> {
    let held = lab.interaction.held.take().ok_or("nothing held")?;
    if velocity != [0; 3] {
        let delta =
            bounded_vector(velocity.map(|v| f64::from(v) / 1000.), 24.).ok_or("invalid throw")?;
        kick(lab, fragments, held.id, delta);
        lab.interaction.throws += 1;
    }
    lab.interaction.releases += 1;
    Ok("fragment released".into())
}

#[derive(SystemParam)]
pub struct InteractionResources<'w> {
    lab: ResMut<'w, SimulationLab>,
    world: ResMut<'w, WorldRes>,
    fragments: ResMut<'w, DynamicFragments>,
    destruction: ResMut<'w, DestructionHost>,
    budget: Res<'w, FragmentBudgetRes>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    status: ResMut<'w, StatusLine>,
    scroll: Res<'w, bevy::input::mouse::AccumulatedMouseScroll>,
}

pub fn drive(
    resources: InteractionResources,
    cursor: Option<Single<&bevy::window::CursorOptions>>,
    camera: Option<Single<(&Transform, &crate::camera::FlyCamera)>>,
) {
    let InteractionResources {
        mut lab,
        mut world,
        mut fragments,
        mut destruction,
        budget,
        bodies,
        renders,
        mouse,
        keys,
        mut status,
        scroll,
    } = resources;
    if !lab.enabled {
        return;
    }
    if keys.just_pressed(KeyCode::KeyG) {
        status.0 = add_floor(&mut lab, &mut world).unwrap_or_else(|e| e);
    }
    let grabbed = cursor.is_some_and(|c| crate::camera::cursor_grabbed(&c));
    if let Some(camera) = camera {
        let (transform, fly) = camera.into_inner();
        let dir = transform.forward().to_array();
        if let Some(held) = lab.interaction.held.as_mut()
            && held.follow_camera
        {
            held.distance = (held.distance - f64::from(scroll.delta.y)).clamp(3., 24.);
            held.target = [fly.global.x, fly.global.y, fly.global.z];
            for (v, d) in held.target.iter_mut().zip(dir) {
                *v += f64::from(d) * held.distance;
            }
            if !grabbed || mouse.just_released(MouseButton::Right) {
                let _ = release(&mut lab, &mut fragments, [0; 3]);
            } else if mouse.just_pressed(MouseButton::Left) {
                status.0 = release(&mut lab, &mut fragments, dir.map(|v| (v * 24000.) as i32))
                    .unwrap_or_else(|e| e);
            }
        }
        if grabbed
            && mouse.just_pressed(MouseButton::Right)
            && let Some((hit, DamageSpace::FragmentLocal(id), _)) =
                pick(&world, &fragments, fly.global, dir)
        {
            let point = [
                f64::from(hit.cell.x) + 0.5,
                f64::from(hit.cell.y) + 0.5,
                f64::from(hit.cell.z) + 0.5,
            ];
            lab.interaction.held = Some(HeldFragment {
                id,
                local_point: point,
                geometry: fragments
                    .get(id)
                    .expect("picked native fragment")
                    .geometry_revision(),
                target: std::array::from_fn(|i| {
                    [fly.global.x, fly.global.y, fly.global.z][i]
                        + f64::from(dir[i]) * f64::from(hit.distance.clamp(3., 24.))
                }),
                distance: f64::from(hit.distance.clamp(3., 24.)),
                follow_camera: true,
            });
            lab.interaction.holds += 1;
            status.0 = format!("holding {id}: move to pull, LMB to throw");
        }
        if grabbed
            && keys.just_pressed(KeyCode::KeyB)
            && let Some((hit, _, _)) = pick(&world, &fragments, fly.global, dir)
        {
            let c = [fly.global.x, fly.global.y, fly.global.z];
            let point = std::array::from_fn(|i| {
                ((c[i] + f64::from(dir[i]) * f64::from(hit.distance)) * 1000.).round() as i32
            });
            status.0 = queue_blast(&mut lab, &fragments, point, 6, 10000).unwrap_or_else(|e| e);
        }
    }
    let start = std::time::Instant::now();
    let measured = lab.interaction.busy();
    for _ in 0..TARGETS_PER_FRAME {
        if lab.background.enabled && !lab.background.room() {
            break;
        }
        let Some(target) = lab.interaction.pending.pop_front() else {
            break;
        };
        let mut center = target.center;
        if let DamageSpace::FragmentLocal(id) = target.space {
            let Some(f) = fragments
                .get(id)
                .filter(|f| Some(f.geometry_revision()) == target.revision)
            else {
                lab.interaction.stale += 1;
                continue;
            };
            let Some(q) = rotation(f) else {
                lab.interaction.stale += 1;
                continue;
            };
            let p = q.conjugate() * (Vector::new(center.x, center.y, center.z) - translation(f));
            center = GlobalPos::new(p.x, p.y, p.z);
        }
        let Some(impact) = FractureImpact::radial_blast(
            target.space,
            center,
            target.radius,
            DamageAmount(target.energy),
        ) else {
            lab.interaction.refused += 1;
            continue;
        };
        if lab.background.enabled {
            lab.background.enqueue(crate::fracture_worker::Request {
                work: crate::fracture_worker::Work::Impact(impact),
                effect: crate::fracture_worker::Effect::Blast(target.center, target.radius),
                space: target.space,
                geometry: target.revision,
                static_volume: None,
            });
            continue;
        }
        let available = budget.0.hard_bytes.saturating_sub(
            budget
                .account(&fragments, &bodies, renders.mesh_bytes())
                .footprint
                .tracked_bytes(),
        );
        let policy = lab.fracture_policy;
        let result = match target.space {
            DamageSpace::StaticWorld => engine_destruction::fracture_static_if(
                &mut world.0,
                fragments.store_mut(),
                &mut lab.fracture_state,
                &mut destruction.sequence,
                &impact,
                &policy,
                &AllResident,
                FractureTransactionLimits::default(),
                |parts| fits_fragment_storage(parts, available),
            )
            .map(|c| {
                (
                    c.fragments,
                    c.fracture.broken.len(),
                    c.fracture.failed.len(),
                    true,
                )
            }),
            DamageSpace::FragmentLocal(id) => {
                let available = available
                    .saturating_add(fragments.get(id).map_or(0, Fragment::footprint_bytes));
                engine_destruction::fracture_fragment_if(
                    fragments.store_mut(),
                    &mut lab.fracture_state,
                    &mut destruction.sequence,
                    &impact,
                    &policy,
                    FractureTransactionLimits::default(),
                    |parts| fits_fragment_storage(parts, available),
                )
                .map(|c| {
                    let split = matches!(c.result, FragmentDamageResult::Refractured(_));
                    (
                        survivors(&c.result, id),
                        c.fracture.broken.len(),
                        c.fracture.failed.len(),
                        split,
                    )
                })
            }
        };
        match result {
            Ok((ids, broken, failed, created)) => {
                lab.interaction.applied += 1;
                lab.interaction.broken += broken as u64;
                lab.interaction.failed += failed as u64;
                if created {
                    lab.interaction.created += ids.len() as u64;
                }
                for id in ids {
                    let Some(c) = fragments.get(id).and_then(world_center) else {
                        continue;
                    };
                    let offset =
                        (c - Vector::new(target.center.x, target.center.y, target.center.z))
                            .to_array();
                    if let Some(delta) = radial_velocity(offset, f64::from(target.radius) * 2., 22.)
                    {
                        kick(&mut lab, &mut fragments, id, delta);
                    }
                }
                status.0 = format!("blast: {broken} broken bonds, {failed} failed cells");
            }
            Err(e) => {
                lab.interaction.refused += 1;
                status.0 = format!("blast held: {e:?}");
            }
        }
    }
    if measured {
        if lab.interaction.work_ms.len() == 1024 {
            lab.interaction.work_ms.pop_front();
        }
        lab.interaction
            .work_ms
            .push_back(start.elapsed().as_secs_f64() * 1000.);
    }
    if lab
        .interaction
        .held
        .as_ref()
        .is_some_and(|h| fragments.get(h.id).is_none())
    {
        lab.interaction.held = None;
        lab.interaction.releases += 1;
    }
}

/// Native velocity edits must reach an existing backend body before its next
/// step. New bodies read them on creation; stale colliders are never updated.
pub fn sync_motion(
    mut commands: Commands,
    mut lab: ResMut<SimulationLab>,
    fragments: Res<DynamicFragments>,
    bodies: Res<FragmentBodies>,
    mut query: Query<(
        Entity,
        &DynamicFragmentBody,
        &mut LinearVelocity,
        &mut AngularVelocity,
    )>,
) {
    let ids = std::mem::take(&mut lab.interaction.motion);
    if ids.is_empty() {
        return;
    }
    for (entity, body, mut linear, mut angular) in &mut query {
        if !ids.contains(&body.id()) {
            continue;
        }
        let Some(f) = fragments.get(body.id()).filter(|f| bodies.is_current(f)) else {
            continue;
        };
        linear.0 = Vector::from_array(f.linear_velocity.map(f64::from));
        angular.0 = Vector::from_array(f.angular_velocity.map(f64::from));
        commands.entity(entity).remove::<Sleeping>();
    }
}

pub fn draw(
    lab: Res<SimulationLab>,
    fragments: Res<DynamicFragments>,
    origin: Res<crate::render::RenderOriginRes>,
    mut gizmos: Gizmos,
) {
    let Some(held) = lab.interaction.held.as_ref() else {
        return;
    };
    let Some(f) = fragments.get(held.id) else {
        return;
    };
    let Some(q) = rotation(f) else {
        return;
    };
    let point = translation(f) + q * Vector::from_array(held.local_point);
    let a = Vec3::from_array(
        origin
            .0
            .to_render(GlobalPos::new(point.x, point.y, point.z)),
    );
    let b = Vec3::from_array(origin.0.to_render(GlobalPos::new(
        held.target[0],
        held.target[1],
        held.target[2],
    )));
    let color = Color::srgb(0.2, 0.95, 1.);
    gizmos.line(a, b, color);
    gizmos.sphere(Isometry3d::from_translation(b), 0.18, color);
}

/// Fixed-step point force, independent of render rate or mouse event frequency.
pub fn hold(
    mut lab: ResMut<SimulationLab>,
    fragments: Res<DynamicFragments>,
    bodies: Res<FragmentBodies>,
    mut query: Query<(&DynamicFragmentBody, &ComputedMass, Forces)>,
) {
    if !lab.enabled {
        return;
    }
    let Some(held) = lab.interaction.held.clone() else {
        return;
    };
    let Some(f) = fragments.get(held.id) else {
        lab.interaction.held = None;
        return;
    };
    if f.geometry_revision() != held.geometry {
        lab.interaction.held = None;
        lab.interaction.releases += 1;
        return;
    }
    if !bodies.is_current(f) {
        return;
    }
    for (body, mass, mut forces) in &mut query {
        if body.id() != held.id {
            continue;
        }
        let point =
            forces.position().0 + forces.rotation().0 * Vector::from_array(held.local_point);
        let error = Vector::from_array(held.target) - point;
        let Some(acceleration) = GrabPolicy::default().acceleration(
            error.to_array(),
            forces.velocity_at_point(point).to_array(),
            mass.value(),
        ) else {
            lab.interaction.held = None;
            lab.interaction.releases += 1;
            return;
        };
        forces.apply_force_at_point(Vector::from_array(acceleration) * mass.value(), point);
        lab.interaction.hold_steps += 1;
        return;
    }
}

pub(crate) fn refuse_blast(lab: &mut SimulationLab) {
    lab.interaction.refused += 1;
}
pub(crate) fn complete_blast(
    lab: &mut SimulationLab,
    fragments: &mut DynamicFragments,
    summary: &engine_destruction::fracture_jobs::JobSummary,
    center: GlobalPos,
    radius: u32,
) {
    lab.interaction.applied += 1;
    lab.interaction.failed += summary.failed;
    lab.interaction.broken += summary.broken;
    lab.interaction.created += summary.created;
    for &id in &summary.ids {
        if let Some(c) = fragments.get(id).and_then(world_center)
            && let Some(delta) = radial_velocity(
                (c - Vector::new(center.x, center.y, center.z)).to_array(),
                f64::from(radius) * 2.,
                22.,
            )
        {
            kick(lab, fragments, id, delta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_floor_is_ordinary_anchored_one_material_geometry() {
        let mut world = WorldRes(engine_world::World::default());
        let mut lab = SimulationLab::default();
        add_floor(&mut lab, &mut world).unwrap();
        let count = world.0.stats().occupied_cells;
        assert_eq!(count, 4096);
        add_floor(&mut lab, &mut world).unwrap();
        assert_eq!(world.0.stats().occupied_cells, count);
        for z in 32..96 {
            for x in 32..96 {
                let c = engine_core::CellPos::new(x, -1, z);
                assert_eq!(
                    world.0.get(c),
                    Some(engine_destruction::REFERENCE_FRACTURE_MATERIAL)
                );
                assert!(world.0.is_anchor(c));
            }
        }
        assert!(
            !lab.enabled,
            "authored cells are valid with simulation disabled"
        );
    }
    fn native() -> DynamicFragments {
        let (chunk, _, _) = engine_stress::destruction_runner::canonical_fracture_chunk().unwrap();
        let mut result = DynamicFragments::default();
        result.insert(chunk);
        result
    }
    #[test]
    fn manipulating_a_fragment_preserves_geometry_and_damage() {
        let mut fragments = native();
        let mut lab = SimulationLab::default();
        let before = fragments.iter().next().unwrap().1.clone();
        grab_largest(&mut lab, &fragments, [64500, 16000, 78000]).unwrap();
        move_grab(&mut lab, [68500, 14000, 75000]).unwrap();
        release(&mut lab, &mut fragments, [0, 0, -24000]).unwrap();
        let after = fragments.get(before.id).unwrap();
        assert_eq!(after.geometry_revision(), before.geometry_revision());
        assert_eq!(
            after.occupied_cells().collect::<Vec<_>>(),
            before.occupied_cells().collect::<Vec<_>>()
        );
        assert_eq!(after.pose, before.pose);
        assert_eq!(after.linear_velocity[2], before.linear_velocity[2] - 24.);
        assert_eq!(lab.interaction.motion.len(), 1);
        assert!(!lab.interaction.holding());
        assert!(release(&mut lab, &mut fragments, [0; 3]).is_err());
    }
    #[test]
    fn blast_queue_is_bounded_and_cannot_repeat_while_pending() {
        let base = native();
        let (_, chunk) = base.iter().next().unwrap();
        let mut fragments = DynamicFragments::default();
        for i in 0..40 {
            let cells = chunk.occupied_cells().collect();
            let mut f = chunk
                .subfragment_with_id(FragmentId::new(500, i), &cells)
                .unwrap();
            let c = f.local_centre();
            f.pose.translation = GlobalPos::new(-c[0], -c[1], -c[2]);
            fragments.insert(f);
        }
        let mut lab = SimulationLab::default();
        queue_blast(&mut lab, &fragments, [0; 3], 6, 10000).unwrap();
        assert_eq!(lab.interaction.pending.len(), 32);
        assert_eq!(lab.interaction.capped, 9);
        assert!(queue_blast(&mut lab, &fragments, [0; 3], 6, 10000).is_err());
        assert_eq!(lab.interaction.blasts, 1);
        assert_eq!(fragments.len(), 40);
    }
    #[test]
    fn directional_impulse_is_shared_across_survivors() {
        let mut fragments = native();
        let id = fragments.iter().next().unwrap().0;
        let mass = fragments.get(id).unwrap().cell_count();
        let mut lab = SimulationLab::default();
        directional_kick(&mut lab, &mut fragments, &[id], [1., 0., 0.], 120.);
        assert!(
            (f64::from(fragments.get(id).unwrap().linear_velocity[0]) * mass as f64 - 120.).abs()
                < 1e-5
        );
    }
    #[test]
    fn rotated_fragment_pick_is_occluded_by_static_material() {
        let mut fragments = native();
        let id = fragments.iter().next().unwrap().0;
        let f = fragments.get_mut(id).unwrap();
        f.pose.rotation.0 = Quaternion::from_rotation_y(std::f64::consts::FRAC_PI_2).to_array();
        let c = world_center(f).unwrap();
        let from = GlobalPos::new(c.x, c.y, c.z + 30.);
        let mut world = WorldRes(engine_world::World::default());
        let hit = pick(&world, &fragments, from, [0., 0., -1.]).unwrap();
        assert_eq!(hit.1, DamageSpace::FragmentLocal(id));
        world.0.set(
            engine_core::CellPos::new(
                c.x.floor() as i32,
                c.y.floor() as i32,
                (c.z + 20.).floor() as i32,
            ),
            Some(engine_destruction::REFERENCE_FRACTURE_MATERIAL),
        );
        assert_eq!(
            pick(&world, &fragments, from, [0., 0., -1.]).unwrap().1,
            DamageSpace::StaticWorld
        );
    }
}
