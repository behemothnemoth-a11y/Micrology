//! Quantisation boundary for solved contacts. A host supplies contact positions
//! and directions in each participant's own frame; core code never interprets
//! the backend's quaternion convention.
use crate::{
    DamageAmount, DamageEvent, DamageEventId, DamageFalloff, DamageSpace, DamageVolume,
    FractureImpact,
};
use engine_core::GlobalPos;

#[derive(Clone, Copy, Debug)]
pub struct ContactFracturePolicy {
    /// Backend impulse units (normally kg*m/s), not force per rendered frame.
    pub minimum_impulse: f64,
    /// Pre-solve closing speed. Resting support must not inject damage each tick.
    pub minimum_closing_speed: f64,
    /// Abstract fracture units per impulse unit; not a claim of joules.
    pub energy_per_impulse: u32,
    pub maximum_energy: u32,
    pub reach_cells: u32,
}
impl Default for ContactFracturePolicy {
    fn default() -> Self {
        Self {
            minimum_impulse: 20.,
            minimum_closing_speed: 1.,
            energy_per_impulse: 100,
            maximum_energy: 6000,
            reach_cells: 6,
        }
    }
}
impl ContactFracturePolicy {
    /// One event budget shared between both participants. Changing the number
    /// of contact points never multiplies energy; the host picks one sample.
    pub fn energies(self, normal_impulse: f64, closing_speed: f64) -> Option<[DamageAmount; 2]> {
        if !normal_impulse.is_finite()
            || !closing_speed.is_finite()
            || !self.minimum_impulse.is_finite()
            || self.minimum_impulse < 0.
            || !self.minimum_closing_speed.is_finite()
            || self.minimum_closing_speed < 0.
            || normal_impulse <= 0.
            || normal_impulse < self.minimum_impulse
            || closing_speed <= 0.
            || closing_speed < self.minimum_closing_speed
            || self.energy_per_impulse == 0
            || self.maximum_energy < 2
        {
            return None;
        }
        let total = (normal_impulse * f64::from(self.energy_per_impulse))
            .floor()
            .min(f64::from(self.maximum_energy)) as u32;
        (total >= 2).then_some([DamageAmount(total / 2), DamageAmount(total - total / 2)])
    }

    pub fn impact(
        self,
        space: DamageSpace,
        point: GlobalPos,
        direction: [f64; 3],
        energy: DamageAmount,
    ) -> Option<FractureImpact> {
        if self.reach_cells == 0
            || self.reach_cells > 64
            || energy.0 == 0
            || energy.0 > self.maximum_energy
            || direction.iter().any(|v| !v.is_finite())
            || direction.iter().all(|v| *v == 0.)
        {
            return None;
        }
        let event = DamageEvent::new(
            DamageEventId::new(0, 0),
            space,
            None,
            DamageVolume::Sphere {
                center: point,
                radius: f64::from(self.reach_cells),
                falloff: DamageFalloff::Linear,
            },
            energy,
            None,
        )
        .ok()?;
        FractureImpact::from_event(&event, Some(direction)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contact_energy_is_finite_capped_and_shared_not_doubled() {
        let policy = ContactFracturePolicy::default();
        assert_eq!(policy.energies(60., 12.), Some([DamageAmount(3000); 2]));
        assert_eq!(
            policy.energies(f64::MAX, 12.),
            Some([DamageAmount(3000); 2])
        );
        for (impulse, speed) in [
            (f64::NAN, 2.),
            (40., f64::INFINITY),
            (40., 0.),
            (-40., 2.),
            (2., 12.),
            (40., -2.),
        ] {
            assert!(policy.energies(impulse, speed).is_none());
        }
    }
    #[test]
    fn target_direction_and_space_are_quantized_once() {
        let p = ContactFracturePolicy::default();
        let space = DamageSpace::FragmentLocal(crate::FragmentId::new(8, 2));
        let hit = p
            .impact(
                space,
                GlobalPos::new(1., 2., 3.),
                [0., 1., 0.],
                DamageAmount(1000),
            )
            .unwrap();
        assert_eq!(hit.space(), space);
        assert_eq!(hit.origin_milli(), [1000, 2000, 3000]);
        assert!(
            p.impact(
                space,
                GlobalPos::new(f64::NAN, 0., 0.),
                [0., 1., 0.],
                DamageAmount(1000)
            )
            .is_none()
        );
    }
}
