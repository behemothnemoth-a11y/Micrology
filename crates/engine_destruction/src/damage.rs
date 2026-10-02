//! Engine-neutral applied damage contract: DROP 0004.6.
//!
//! Damage is an input to games built on Micrology, not a weapon system and not a
//! mandatory world rule. A game/host chooses whether to create or apply these
//! events at all.
//!
//! Static cells are addressed in world space. Fragment cells are addressed in
//! fragment-local space because the engine deliberately stores fragment rotation
//! opaquely; a physics/game host that knows quaternion conventions transforms a
//! world-space impact into fragment-local coordinates before creating that event.
//! The original world-space source and impulse may still be carried for later
//! force-to-fragment coupling.

use crate::FragmentId;
use engine_core::{CellBounds, CellPos, CellSource, GlobalPos, MaterialId};
use std::fmt;

/// Stable deterministic identity for one damage event.
///
/// sequence orders root events; index leaves deterministic room for child or
/// secondary events later without making thread-completion order observable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DamageEventId {
    pub sequence: u64,
    pub index: u32,
}

impl DamageEventId {
    pub const fn new(sequence: u64, index: u32) -> Self {
        Self { sequence, index }
    }
}

impl fmt::Display for DamageEventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "d{}.{}", self.sequence, self.index)
    }
}

/// Monotonic root-event sequence owned by a game/host.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct DamageSequence(u64);

impl DamageSequence {
    pub const fn new(next: u64) -> Self {
        Self(next)
    }

    pub const fn peek(self) -> u64 {
        self.0
    }

    pub fn next_root(&mut self) -> DamageEventId {
        let id = DamageEventId::new(self.0, 0);
        self.0 = self.0.wrapping_add(1);
        id
    }
}

/// Integer damage currency.
///
/// The unit is deliberately abstract in DROP 0004. A later material policy can
/// decide what a given amount means for wood, steel, glass or fantasy matter.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct DamageAmount(pub u32);

impl DamageAmount {
    pub const ZERO: Self = Self(0);

    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    fn scaled(self, factor: f64) -> Self {
        if self.0 == 0 || factor <= 0.0 {
            return Self::ZERO;
        }
        if factor >= 1.0 {
            return self;
        }
        Self((f64::from(self.0) * factor).floor() as u32)
    }
}

/// Coordinate domain in which an event's affected volume is expressed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DamageSpace {
    StaticWorld,
    FragmentLocal(FragmentId),
}

/// How strength changes across a radial event.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DamageFalloff {
    /// Every cell inside the sphere receives full base strength.
    Uniform,
    /// Strength falls linearly from full at the center to zero at the radius.
    Linear,
}

/// A target-space damage volume.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DamageVolume {
    /// Exactly one target-space cell.
    Cell(CellPos),
    /// Every cell whose center lies inside the sphere.
    Sphere {
        center: GlobalPos,
        radius: f64,
        falloff: DamageFalloff,
    },
    /// Inclusive cell-aligned box.
    Box(CellBounds),
}

/// A world-space impulse vector carried once by the event.
///
/// It is not repeated per damaged cell. DROP 0004.8 may transfer some or all of
/// this vector to fragments created by a detachment.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DamageImpulse(pub [f64; 3]);

impl DamageImpulse {
    pub fn new(vector: [f64; 3]) -> Result<Self, DamageEventError> {
        if vector.into_iter().all(f64::is_finite) {
            Ok(Self(vector))
        } else {
            Err(DamageEventError::NonFiniteImpulse)
        }
    }
}

/// Validation errors for externally-created damage events.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DamageEventError {
    NonFiniteSource,
    NonFiniteCenter,
    NonFiniteImpulse,
    InvalidRadius,
}

impl fmt::Display for DamageEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteSource => write!(f, "damage source contains a non-finite coordinate"),
            Self::NonFiniteCenter => write!(f, "damage center contains a non-finite coordinate"),
            Self::NonFiniteImpulse => write!(f, "damage impulse contains a non-finite component"),
            Self::InvalidRadius => write!(f, "damage sphere radius must be finite and non-negative"),
        }
    }
}

impl std::error::Error for DamageEventError {}

/// One deterministic, gameplay-independent damage input.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DamageEvent {
    pub id: DamageEventId,
    pub space: DamageSpace,
    pub source_world: Option<GlobalPos>,
    pub volume: DamageVolume,
    pub strength: DamageAmount,
    pub impulse_world: Option<DamageImpulse>,
}

impl DamageEvent {
    pub fn new(
        id: DamageEventId,
        space: DamageSpace,
        source_world: Option<GlobalPos>,
        volume: DamageVolume,
        strength: DamageAmount,
        impulse_world: Option<DamageImpulse>,
    ) -> Result<Self, DamageEventError> {
        if source_world.is_some_and(|p| !finite_point(p)) {
            return Err(DamageEventError::NonFiniteSource);
        }
        if let DamageVolume::Sphere { center, radius, .. } = volume {
            if !finite_point(center) {
                return Err(DamageEventError::NonFiniteCenter);
            }
            if !radius.is_finite() || radius < 0.0 {
                return Err(DamageEventError::InvalidRadius);
            }
        }
        Ok(Self {
            id,
            space,
            source_world,
            volume,
            strength,
            impulse_world,
        })
    }

    /// Conservative inclusive cell bounds a host can iterate before sampling.
    ///
    /// Returns None only when a sphere lies wholly outside the current i32 cell
    /// address space. Cell and box events already carry valid CellPos bounds.
    pub fn affected_cell_bounds(self) -> Option<CellBounds> {
        match self.volume {
            DamageVolume::Cell(cell) => Some(CellBounds::new(cell, cell)),
            DamageVolume::Box(bounds) => Some(bounds),
            DamageVolume::Sphere { center, radius, .. } => sphere_cell_bounds(center, radius),
        }
    }

    /// Base damage delivered to this target-space cell, before material policy.
    pub fn amount_at_cell(self, cell: CellPos) -> Option<DamageAmount> {
        let amount = match self.volume {
            DamageVolume::Cell(target) => (target == cell).then_some(self.strength)?,
            DamageVolume::Box(bounds) => bounds_contains(bounds, cell).then_some(self.strength)?,
            DamageVolume::Sphere {
                center,
                radius,
                falloff,
            } => {
                let dx = (f64::from(cell.x) + 0.5) - center.x;
                let dy = (f64::from(cell.y) + 0.5) - center.y;
                let dz = (f64::from(cell.z) + 0.5) - center.z;
                let distance_sq = dx * dx + dy * dy + dz * dz;
                let radius_sq = radius * radius;
                if distance_sq > radius_sq {
                    return None;
                }
                match falloff {
                    DamageFalloff::Uniform => self.strength,
                    DamageFalloff::Linear if radius == 0.0 => self.strength,
                    DamageFalloff::Linear => {
                        let distance = distance_sq.sqrt();
                        self.strength.scaled(1.0 - distance / radius)
                    }
                }
            }
        };
        (amount != DamageAmount::ZERO).then_some(amount)
    }
}

fn finite_point(point: GlobalPos) -> bool {
    point.x.is_finite() && point.y.is_finite() && point.z.is_finite()
}

fn bounds_contains(bounds: CellBounds, cell: CellPos) -> bool {
    cell.x >= bounds.min.x
        && cell.x <= bounds.max.x
        && cell.y >= bounds.min.y
        && cell.y <= bounds.max.y
        && cell.z >= bounds.min.z
        && cell.z <= bounds.max.z
}

fn sphere_cell_bounds(center: GlobalPos, radius: f64) -> Option<CellBounds> {
    fn axis(center: f64, radius: f64) -> Option<(i32, i32)> {
        // Cell x occupies center x + 0.5. The lower expression is intentionally
        // conservative by up to one cell; amount_at_cell performs the exact test.
        let low = center - radius - 0.5;
        let high = center + radius - 0.5;
        if high < f64::from(i32::MIN) || low > f64::from(i32::MAX) {
            return None;
        }
        let min = low.floor().max(f64::from(i32::MIN)) as i32;
        let max = high.floor().min(f64::from(i32::MAX)) as i32;
        Some((min, max))
    }

    let (min_x, max_x) = axis(center.x, radius)?;
    let (min_y, max_y) = axis(center.y, radius)?;
    let (min_z, max_z) = axis(center.z, radius)?;
    Some(CellBounds::new(
        CellPos::new(min_x, min_y, min_z),
        CellPos::new(max_x, max_y, max_z),
    ))
}

/// One occupied cell presented to a game/engine damage policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DamageTarget {
    StaticCell {
        cell: CellPos,
        material: MaterialId,
    },
    FragmentCell {
        fragment: FragmentId,
        cell: CellPos,
        material: MaterialId,
    },
}

impl DamageTarget {
    pub const fn cell(self) -> CellPos {
        match self {
            Self::StaticCell { cell, .. } | Self::FragmentCell { cell, .. } => cell,
        }
    }

    pub const fn material(self) -> MaterialId {
        match self {
            Self::StaticCell { material, .. } | Self::FragmentCell { material, .. } => material,
        }
    }

    pub const fn space(self) -> DamageSpace {
        match self {
            Self::StaticCell { .. } => DamageSpace::StaticWorld,
            Self::FragmentCell { fragment, .. } => DamageSpace::FragmentLocal(fragment),
        }
    }
}

/// Unit of damage work emitted by a policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DamageWork {
    pub event: DamageEventId,
    pub target: DamageTarget,
    pub amount: DamageAmount,
}

/// Game/engine policy seam between an event and a particular occupied cell.
///
/// DROP 0004 ships UniformDamagePolicy as a reference response. DROP 0005 can
/// supply material-aware policies without changing event geometry or the sparse
/// accumulation layer built on DamageWork.
pub trait DamagePolicy: Send + Sync {
    fn evaluate(&self, event: &DamageEvent, target: DamageTarget) -> Option<DamageWork>;
}

/// Explicit work ceiling for sampling one event over a CellSource.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DamageEvaluationLimits {
    pub max_cells_considered: usize,
}

impl DamageEvaluationLimits {
    pub const UNLIMITED: Self = Self {
        max_cells_considered: usize::MAX,
    };

    pub const fn new(max_cells_considered: usize) -> Self {
        Self {
            max_cells_considered,
        }
    }
}

/// Deterministic result of sampling an event against occupied cells.
///
/// Work from a truncated result must not be partially applied. The caller may
/// retry with a larger budget, split the gameplay event, or discard it.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct DamageEvaluation {
    pub work: Vec<DamageWork>,
    pub cells_considered: u64,
    pub truncated: bool,
}

impl DamageEvaluation {
    pub fn may_apply(&self) -> bool {
        !self.truncated
    }
}

/// Evaluate one event against any CellSource in the event's own coordinate space.
///
/// A World supplies static world cells. A Fragment supplies fragment-local
/// cells. The event's DamageSpace decides which DamageTarget variant is emitted.
pub fn evaluate_damage_event(
    event: &DamageEvent,
    source: &dyn CellSource,
    policy: &dyn DamagePolicy,
    limits: DamageEvaluationLimits,
) -> DamageEvaluation {
    let Some(bounds) = event.affected_cell_bounds() else {
        return DamageEvaluation::default();
    };

    let mut evaluation = DamageEvaluation::default();

    'cells: for y in bounds.min.y..=bounds.max.y {
        for z in bounds.min.z..=bounds.max.z {
            for x in bounds.min.x..=bounds.max.x {
                if evaluation.cells_considered as usize >= limits.max_cells_considered {
                    evaluation.truncated = true;
                    break 'cells;
                }
                evaluation.cells_considered = evaluation.cells_considered.saturating_add(1);

                let cell = CellPos::new(x, y, z);
                let Some(material) = source.material_at(cell) else {
                    continue;
                };
                let target = match event.space {
                    DamageSpace::StaticWorld => DamageTarget::StaticCell { cell, material },
                    DamageSpace::FragmentLocal(fragment) => DamageTarget::FragmentCell {
                        fragment,
                        cell,
                        material,
                    },
                };
                if let Some(work) = policy.evaluate(event, target) {
                    evaluation.work.push(work);
                }
            }
        }
    }

    evaluation
}

/// Reference policy: material identity does not change resistance yet.
#[derive(Clone, Copy, Default, Debug)]
pub struct UniformDamagePolicy;

impl DamagePolicy for UniformDamagePolicy {
    fn evaluate(&self, event: &DamageEvent, target: DamageTarget) -> Option<DamageWork> {
        if event.space != target.space() {
            return None;
        }
        Some(DamageWork {
            event: event.id,
            target,
            amount: event.amount_at_cell(target.cell())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_event_ids_are_deterministic_and_ordered() {
        let mut sequence = DamageSequence::new(9);
        assert_eq!(sequence.next_root(), DamageEventId::new(9, 0));
        assert_eq!(sequence.next_root(), DamageEventId::new(10, 0));
        assert_eq!(sequence.peek(), 11);
    }

    #[test]
    fn linear_sphere_is_strongest_at_the_center_and_zero_at_the_edge() {
        let event = DamageEvent::new(
            DamageEventId::new(1, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Sphere {
                center: GlobalPos::new(0.5, 0.5, 0.5),
                radius: 4.0,
                falloff: DamageFalloff::Linear,
            },
            DamageAmount(100),
            None,
        )
        .unwrap();

        assert_eq!(event.amount_at_cell(CellPos::ZERO), Some(DamageAmount(100)));
        let near = event.amount_at_cell(CellPos::new(2, 0, 0)).unwrap();
        assert!(near < DamageAmount(100));
        assert_eq!(event.amount_at_cell(CellPos::new(4, 0, 0)), None);
    }

    #[test]
    fn uniform_sphere_keeps_full_strength_inside_radius() {
        let event = DamageEvent::new(
            DamageEventId::new(2, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Sphere {
                center: GlobalPos::new(0.5, 0.5, 0.5),
                radius: 4.0,
                falloff: DamageFalloff::Uniform,
            },
            DamageAmount(77),
            None,
        )
        .unwrap();

        assert_eq!(
            event.amount_at_cell(CellPos::new(3, 0, 0)),
            Some(DamageAmount(77))
        );
    }

    #[test]
    fn target_space_prevents_world_event_from_accidentally_damaging_fragment_local_cells() {
        let event = DamageEvent::new(
            DamageEventId::new(3, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Cell(CellPos::new(1, 2, 3)),
            DamageAmount(10),
            None,
        )
        .unwrap();
        let policy = UniformDamagePolicy;
        let fragment = FragmentId::new(4, 0);

        assert!(policy
            .evaluate(
                &event,
                DamageTarget::StaticCell {
                    cell: CellPos::new(1, 2, 3),
                    material: MaterialId(1),
                },
            )
            .is_some());
        assert!(policy
            .evaluate(
                &event,
                DamageTarget::FragmentCell {
                    fragment,
                    cell: CellPos::new(1, 2, 3),
                    material: MaterialId(1),
                },
            )
            .is_none());
    }

    #[test]
    fn fragment_local_events_are_material_agnostic_in_drop_0004() {
        let id = FragmentId::new(5, 0);
        let event = DamageEvent::new(
            DamageEventId::new(4, 0),
            DamageSpace::FragmentLocal(id),
            Some(GlobalPos::new(100.0, 50.0, -20.0)),
            DamageVolume::Cell(CellPos::ZERO),
            DamageAmount(25),
            DamageImpulse::new([3.0, 0.0, -1.0]).ok(),
        )
        .unwrap();
        let policy = UniformDamagePolicy;

        let a = policy
            .evaluate(
                &event,
                DamageTarget::FragmentCell {
                    fragment: id,
                    cell: CellPos::ZERO,
                    material: MaterialId(1),
                },
            )
            .unwrap();
        let b = policy
            .evaluate(
                &event,
                DamageTarget::FragmentCell {
                    fragment: id,
                    cell: CellPos::ZERO,
                    material: MaterialId(999),
                },
            )
            .unwrap();
        assert_eq!(a.amount, b.amount);
    }


    #[test]
    fn same_event_evaluator_works_for_world_and_fragment_sources() {
        use crate::Fragment;
        use engine_world::World;
        use std::collections::BTreeSet;

        let mut world = World::new();
        world.fill_box(
            CellPos::ZERO,
            CellPos::new(1, 0, 0),
            Some(MaterialId(1)),
        );
        world.take_dirty();

        let static_event = DamageEvent::new(
            DamageEventId::new(5, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Box(CellBounds::new(CellPos::ZERO, CellPos::new(1, 0, 0))),
            DamageAmount(9),
            None,
        )
        .unwrap();
        let static_eval = evaluate_damage_event(
            &static_event,
            &world,
            &UniformDamagePolicy,
            DamageEvaluationLimits::UNLIMITED,
        );
        assert!(static_eval.may_apply());
        assert_eq!(static_eval.work.len(), 2);

        let members = BTreeSet::from([CellPos::ZERO, CellPos::new(1, 0, 0)]);
        let fragment_id = FragmentId::new(50, 0);
        let fragment = Fragment::from_cells(fragment_id, &world, &members).unwrap();
        let local_event = DamageEvent::new(
            DamageEventId::new(5, 1),
            DamageSpace::FragmentLocal(fragment_id),
            None,
            DamageVolume::Box(CellBounds::new(CellPos::ZERO, CellPos::new(1, 0, 0))),
            DamageAmount(9),
            None,
        )
        .unwrap();
        let fragment_eval = evaluate_damage_event(
            &local_event,
            &fragment,
            &UniformDamagePolicy,
            DamageEvaluationLimits::UNLIMITED,
        );
        assert!(fragment_eval.may_apply());
        assert_eq!(fragment_eval.work.len(), 2);
        assert!(fragment_eval
            .work
            .iter()
            .all(|work| matches!(work.target, DamageTarget::FragmentCell { fragment, .. } if fragment == fragment_id)));
    }

    #[test]
    fn truncated_event_evaluation_is_never_partially_actionable() {
        use engine_world::World;

        let mut world = World::new();
        world.fill_box(
            CellPos::ZERO,
            CellPos::new(15, 0, 0),
            Some(MaterialId(1)),
        );
        let event = DamageEvent::new(
            DamageEventId::new(5, 2),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Box(CellBounds::new(CellPos::ZERO, CellPos::new(15, 0, 0))),
            DamageAmount(1),
            None,
        )
        .unwrap();

        let evaluation = evaluate_damage_event(
            &event,
            &world,
            &UniformDamagePolicy,
            DamageEvaluationLimits::new(4),
        );
        assert!(evaluation.truncated);
        assert!(!evaluation.may_apply());
        assert_eq!(evaluation.cells_considered, 4);
        assert_eq!(evaluation.work.len(), 4);
    }

    #[test]
    fn invalid_external_floats_are_rejected() {
        assert_eq!(
            DamageImpulse::new([f64::NAN, 0.0, 0.0]),
            Err(DamageEventError::NonFiniteImpulse)
        );
        assert_eq!(
            DamageEvent::new(
                DamageEventId::new(6, 0),
                DamageSpace::StaticWorld,
                None,
                DamageVolume::Sphere {
                    center: GlobalPos::new(0.0, 0.0, 0.0),
                    radius: f64::INFINITY,
                    falloff: DamageFalloff::Uniform,
                },
                DamageAmount(1),
                None,
            ),
            Err(DamageEventError::InvalidRadius)
        );
    }

    #[test]
    fn sphere_bounds_clip_safely_at_cell_address_edges() {
        let event = DamageEvent::new(
            DamageEventId::new(7, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Sphere {
                center: GlobalPos::new(f64::from(i32::MAX), 0.5, 0.5),
                radius: 32.0,
                falloff: DamageFalloff::Uniform,
            },
            DamageAmount(1),
            None,
        )
        .unwrap();

        let bounds = event.affected_cell_bounds().unwrap();
        assert_eq!(bounds.max.x, i32::MAX);
        assert!(bounds.min.x < i32::MAX);
    }
}
