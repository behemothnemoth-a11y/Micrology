//! DROP 0004.8 host policy: impact sampling and bounded secondary damage.

use crate::destruction::DestructionHost;
use crate::fragment_render::FragmentEntities;
use crate::fragment_streaming::{FragmentStreamRes, FragmentStreamTasks, finish_fragment_saves};
use crate::physics::{DynamicFragmentBody, DynamicFragments, FragmentBodies, FragmentBudgetRes};
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use avian3d::prelude::Collisions;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use engine_destruction::{
    DamageAmount, DamageEvaluationLimits, DamageSequence, DamageSource, DamageSpace,
    FragmentDamageResult, FragmentId, FragmentImpact, ProgressiveDamageLimits,
    ProgressiveDamageStore, RefractureRefusal, SecondaryDamage, SecondaryDamageLimits,
    SecondaryDamageQueue, UniformDamagePolicy, UniformFailurePolicy, UniformImpactDamagePolicy,
    damage_fragment_store_if, evaluate_damage_event, static_failure_batch,
};
use std::collections::{BTreeMap, BTreeSet};

const SECONDARY_LIMITS: SecondaryDamageLimits = SecondaryDamageLimits::new(32, 4, 3);
const DAMAGE_STATE_LIMIT: ProgressiveDamageLimits = ProgressiveDamageLimits::new(16_384);
const FAILURE_THRESHOLD: DamageAmount = DamageAmount(18);
fn impact_policy() -> UniformImpactDamagePolicy {
    let min_normal_impulse = std::env::var("MICROLOGY_IMPACT_MIN")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(20.0);
    let damage_per_impulse = std::env::var("MICROLOGY_IMPACT_DAMAGE_SCALE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(1);
    UniformImpactDamagePolicy {
        min_normal_impulse,
        radius: 1.75,
        damage_per_impulse,
        max_strength: 48,
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct ImpactStats {
    pub collisions_sampled: u64,
    pub secondary_enqueued: u64,
    pub secondary_processed: u64,
    pub static_failed_cells: u64,
    pub fragment_failed_cells: u64,
    pub fragments_updated: u64,
    pub fragments_destroyed: u64,
    pub refractures: u64,
    pub refracture_children: u64,
    pub truncated_events: u64,
    pub rejected_events: u64,
}
#[derive(Resource, Default)]
pub struct ImpactHost {
    damage_sequence: DamageSequence,
    progressive: ProgressiveDamageStore,
    queue: SecondaryDamageQueue,
    generations: BTreeMap<FragmentId, u8>,
    active_contacts: BTreeSet<(u64, u64)>,
    stats: ImpactStats,
}

impl ImpactHost {
    pub fn stats(&self) -> ImpactStats {
        self.stats
    }

    pub fn note_fragments(
        &mut self,
        fragments: impl IntoIterator<Item = FragmentId>,
        generation: u8,
    ) {
        for id in fragments {
            self.generations.insert(id, generation);
        }
    }

    pub fn generation(&self, id: FragmentId) -> u8 {
        self.generations.get(&id).copied().unwrap_or(0)
    }
    pub fn forget_fragment(&mut self, id: FragmentId) {
        self.generations.remove(&id);
    }

    pub fn enqueue(&mut self, damage: SecondaryDamage) {
        match self.queue.enqueue(damage, SECONDARY_LIMITS) {
            Ok(()) => self.stats.secondary_enqueued += 1,
            Err(_) => self.stats.rejected_events += 1,
        }
    }
}

/// Sample solved Avian contacts after the physics step.
///
/// Only fragment-vs-nonfragment contacts become static-world secondary damage.
/// Fragment-vs-fragment contact damage remains a future host policy choice.
pub fn collect_fragment_impacts(
    collisions: Collisions,
    fragment_bodies: Query<&DynamicFragmentBody>,
    mut host: ResMut<ImpactHost>,
    lab: Res<crate::sim_lab::SimulationLab>,
) {
    // The one-material lab uses persistent bond fracture for contacts; mixing
    // scalar failure into it would bypass both its model and atomic admission.
    if lab.enabled {
        return;
    }
    let policy = impact_policy();
    let previous = std::mem::take(&mut host.active_contacts);
    let mut current = BTreeSet::new();

    for pair in collisions.iter() {
        let first = pair
            .body1
            .and_then(|entity| fragment_bodies.get(entity).ok())
            .copied()
            .or_else(|| fragment_bodies.get(pair.collider1).ok().copied());
        let second = pair
            .body2
            .and_then(|entity| fragment_bodies.get(entity).ok())
            .copied()
            .or_else(|| fragment_bodies.get(pair.collider2).ok().copied());

        let (body, fragment_is_first) = match (first, second) {
            (Some(body), None) => (body, true),
            (None, Some(body)) => (body, false),
            _ => continue,
        };
        let a = pair.collider1.to_bits();
        let b = pair.collider2.to_bits();
        let key = if a <= b { (a, b) } else { (b, a) };
        current.insert(key);
        if previous.contains(&key) {
            continue;
        }

        let Some(contact) = pair.find_deepest_contact() else {
            continue;
        };
        let normal_impulse = pair.max_normal_impulse_magnitude();
        if !normal_impulse.is_finite() || normal_impulse < policy.min_normal_impulse {
            continue;
        }

        host.stats.collisions_sampled += 1;
        let mut vector = pair.max_normal_impulse();
        if !fragment_is_first {
            vector = -vector;
        }
        let impact = FragmentImpact {
            fragment: body.id(),
            point_world: engine_core::GlobalPos::new(
                contact.point.x,
                contact.point.y,
                contact.point.z,
            ),
            impulse_world: [vector.x, vector.y, vector.z],
            normal_impulse,
        };
        let id = host.damage_sequence.next_root();
        let Some(event) = policy.event_for(id, impact) else {
            continue;
        };
        let generation = host.generation(body.id()).saturating_add(1);
        host.enqueue(SecondaryDamage { event, generation });
    }
    host.active_contacts = current;
}

#[derive(SystemParam)]
pub struct ImpactProcessResources<'w> {
    world: ResMut<'w, WorldRes>,
    destruction: ResMut<'w, DestructionHost>,
    fragments: ResMut<'w, DynamicFragments>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
    budget: Res<'w, FragmentBudgetRes>,
    stream: ResMut<'w, StreamRes>,
    fragment_stream: ResMut<'w, FragmentStreamRes>,
    fragment_tasks: ResMut<'w, FragmentStreamTasks>,
    host: ResMut<'w, ImpactHost>,
    status: ResMut<'w, StatusLine>,
}
pub fn process_secondary_damage(resources: ImpactProcessResources) {
    let ImpactProcessResources {
        mut world,
        mut destruction,
        mut fragments,
        bodies,
        renders,
        budget,
        mut stream,
        mut fragment_stream,
        mut fragment_tasks,
        mut host,
        mut status,
    } = resources;

    let events = host.queue.drain_step(SECONDARY_LIMITS);
    for damage in events {
        host.stats.secondary_processed += 1;

        match damage.event.space {
            DamageSpace::StaticWorld => {
                let Ok(evaluation) = evaluate_damage_event(
                    &damage.event,
                    DamageSource::Static(&world.0),
                    &UniformDamagePolicy,
                    DamageEvaluationLimits::new(4096),
                ) else {
                    host.stats.rejected_events += 1;
                    continue;
                };
                if !evaluation.may_apply() {
                    host.stats.truncated_events += 1;
                    continue;
                }
                let Ok(outcome) = host.progressive.apply(
                    evaluation.work,
                    &UniformFailurePolicy::new(FAILURE_THRESHOLD),
                    DAMAGE_STATE_LIMIT,
                ) else {
                    host.stats.rejected_events += 1;
                    continue;
                };
                if outcome.failed.is_empty() {
                    continue;
                }
                let edit = world.0.apply(&static_failure_batch(&outcome.failed));
                if edit.removed_cells.is_empty() {
                    continue;
                }
                for region in &edit.dirtied_regions {
                    stream.streamer.note_region_edited(*region);
                }
                host.stats.static_failed_cells = host
                    .stats
                    .static_failed_cells
                    .saturating_add(edit.removed_cells.len() as u64);
                destruction.enqueue_damage_edit(&edit, damage);
                status.0 = format!(
                    "secondary impact gen {} failed {} static cell(s)",
                    damage.generation,
                    edit.removed_cells.len()
                );
            }
            DamageSpace::FragmentLocal(parent_id) => {
                let Some(parent) = fragments.get(parent_id).cloned() else {
                    host.forget_fragment(parent_id);
                    continue;
                };
                let Ok(evaluation) = evaluate_damage_event(
                    &damage.event,
                    DamageSource::Fragment {
                        fragment: parent_id,
                        cells: &parent,
                    },
                    &UniformDamagePolicy,
                    DamageEvaluationLimits::new(4096),
                ) else {
                    host.stats.rejected_events += 1;
                    continue;
                };
                if !evaluation.may_apply() {
                    host.stats.truncated_events += 1;
                    continue;
                }

                let mut staged_damage = host.progressive.clone();
                let Ok(outcome) = staged_damage.apply(
                    evaluation.work,
                    &UniformFailurePolicy::new(FAILURE_THRESHOLD),
                    DAMAGE_STATE_LIMIT,
                ) else {
                    host.stats.rejected_events += 1;
                    continue;
                };
                if outcome.failed.is_empty() {
                    host.progressive = staged_damage;
                    continue;
                }
                let current_bytes = budget
                    .account(&fragments, &bodies, renders.mesh_bytes())
                    .footprint
                    .tracked_bytes();
                let old_storage = parent.footprint_bytes();
                let hard_bytes = budget.0.hard_bytes;

                let mut staged_store = fragments.store_ref().clone();
                let mut staged_sequence = destruction.sequence;
                let result = damage_fragment_store_if(
                    &mut staged_store,
                    &mut staged_sequence,
                    parent_id,
                    &outcome.failed,
                    |children| {
                        let new_storage: u64 =
                            children.iter().map(|child| child.footprint_bytes()).sum();
                        current_bytes
                            .saturating_sub(old_storage)
                            .saturating_add(new_storage)
                            <= hard_bytes
                    },
                );

                match result {
                    Ok(FragmentDamageResult::Unchanged) => {
                        host.progressive = staged_damage;
                    }
                    Ok(FragmentDamageResult::Updated(_)) => {
                        fragments.replace_store(staged_store);
                        host.progressive = staged_damage;
                        host.stats.fragments_updated += 1;
                        host.stats.fragment_failed_cells += outcome.failed.len() as u64;
                    }
                    Ok(FragmentDamageResult::Destroyed { .. }) => {
                        finish_fragment_saves(
                            &stream,
                            &mut fragment_stream,
                            &mut fragment_tasks,
                            &fragments,
                            &mut status,
                        );
                        if let Err(error) =
                            fragment_stream.delete_authoritative(&stream.dir, parent_id)
                        {
                            host.stats.rejected_events += 1;
                            status.0 = format!("fragment destroy held: {error}");
                            continue;
                        }
                        fragments.replace_store(staged_store);
                        host.progressive = staged_damage;
                        host.forget_fragment(parent_id);
                        host.stats.fragments_destroyed += 1;
                        host.stats.fragment_failed_cells += outcome.failed.len() as u64;
                    }
                    Ok(FragmentDamageResult::Refractured(result)) => {
                        finish_fragment_saves(
                            &stream,
                            &mut fragment_stream,
                            &mut fragment_tasks,
                            &fragments,
                            &mut status,
                        );
                        if let Err(error) =
                            fragment_stream.delete_authoritative(&stream.dir, parent_id)
                        {
                            host.stats.rejected_events += 1;
                            status.0 = format!("fragment refracture held: {error}");
                            continue;
                        }

                        let generation = host.generation(parent_id).max(damage.generation);
                        staged_damage.remap_fragment(parent_id, &result.fragments);
                        fragments.replace_store(staged_store);
                        destruction.sequence = staged_sequence;
                        host.progressive = staged_damage;
                        host.forget_fragment(parent_id);
                        host.note_fragments(
                            result.fragments.iter().map(|fragment| fragment.id),
                            generation,
                        );
                        host.stats.refractures += 1;
                        host.stats.refracture_children = host
                            .stats
                            .refracture_children
                            .saturating_add(result.fragments.len() as u64);
                        host.stats.fragment_failed_cells += outcome.failed.len() as u64;
                        status.0 = format!(
                            "re-fractured {} into {} child fragments",
                            parent_id,
                            result.fragments.len()
                        );
                    }
                    Err(
                        RefractureRefusal::RejectedByPolicy
                        | RefractureRefusal::IdentityCollision
                        | RefractureRefusal::ParentMissing
                        | RefractureRefusal::StillConnected,
                    ) => {
                        host.stats.rejected_events += 1;
                    }
                }
            }
        }
    }
}
