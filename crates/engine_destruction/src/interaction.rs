//! Optional, backend-neutral policies for manipulating volumetric fragments.
//! The host supplies world-space points and velocities. No cell becomes a body.

/// Clamp a finite vector by length, preserving direction.
pub fn bounded_vector(vector: [f64; 3], limit: f64) -> Option<[f64; 3]> {
    if !limit.is_finite() || limit < 0.0 || !vector.into_iter().all(f64::is_finite) {
        return None;
    }
    let length = vector[0].hypot(vector[1]).hypot(vector[2]);
    if !length.is_finite() {
        return None;
    }
    let scale = if length > limit { limit / length } else { 1.0 };
    Some(vector.map(|v| v * scale))
}

/// Damped, bounded point controller. The host applies this acceleration through
/// its actual mass/inertia at the held point, once per fixed step. Heavy bodies
/// receive less acceleration once the force ceiling is reached.
#[derive(Clone, Copy, Debug)]
pub struct GrabPolicy {
    pub stiffness: f64,
    pub damping: f64,
    pub max_acceleration: f64,
    pub max_force: f64,
    pub break_distance: f64,
}

impl Default for GrabPolicy {
    fn default() -> Self {
        Self {
            stiffness: 36.0,
            damping: 12.0,
            max_acceleration: 80.0,
            max_force: 12_000.0,
            break_distance: 24.0,
        }
    }
}

impl GrabPolicy {
    /// None releases an invalid or overextended hold; it never edits geometry.
    pub fn acceleration(
        self,
        error: [f64; 3],
        point_velocity: [f64; 3],
        mass: f64,
    ) -> Option<[f64; 3]> {
        if ![
            self.stiffness,
            self.damping,
            self.max_acceleration,
            self.max_force,
            self.break_distance,
            mass,
        ]
        .into_iter()
        .all(|v| v.is_finite() && v > 0.0)
            || !error.into_iter().chain(point_velocity).all(f64::is_finite)
            || error[0].hypot(error[1]).hypot(error[2]) > self.break_distance
        {
            return None;
        }
        let value =
            std::array::from_fn(|i| self.stiffness * error[i] - self.damping * point_velocity[i]);
        bounded_vector(value, self.max_acceleration.min(self.max_force / mass))
    }
}

/// Finite radial kick with linear distance attenuation and no singularity at
/// the origin. This is a gameplay velocity budget, not a fluid blast solver.
pub fn radial_velocity(offset: [f64; 3], radius: f64, speed: f64) -> Option<[f64; 3]> {
    if !radius.is_finite()
        || radius <= 0.0
        || !speed.is_finite()
        || speed < 0.0
        || !offset.into_iter().all(f64::is_finite)
    {
        return None;
    }
    let distance = offset[0].hypot(offset[1]).hypot(offset[2]);
    if !distance.is_finite() {
        return None;
    }
    if distance >= radius {
        return Some([0.0; 3]);
    }
    let direction = if distance < 1e-9 {
        [0.0, 1.0, 0.0]
    } else {
        offset.map(|v| v / distance)
    };
    Some(direction.map(|v| v * speed * (1.0 - distance / radius)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grab_is_damped_mass_limited_and_releases_instead_of_teleporting() {
        let p = GrabPolicy::default();
        assert_eq!(p.acceleration([0.; 3], [0.; 3], 1.), Some([0.; 3]));
        assert!(p.acceleration([1., 0., 0.], [10., 0., 0.], 1.).unwrap()[0] < 0.);
        assert_eq!(
            p.acceleration([10., 0., 0.], [0.; 3], 1000.),
            Some([12., 0., 0.])
        );
        assert!(p.acceleration([25., 0., 0.], [0.; 3], 1.).is_none());
        assert!(p.acceleration([f64::NAN, 0., 0.], [0.; 3], 1.).is_none());
        assert!(p.acceleration([0.; 3], [0.; 3], 0.).is_none());
    }
    #[test]
    fn radial_kick_has_finite_center_falloff_and_zero_outside() {
        assert_eq!(radial_velocity([0.; 3], 10., 20.), Some([0., 20., 0.]));
        assert_eq!(radial_velocity([5., 0., 0.], 10., 20.), Some([10., 0., 0.]));
        assert_eq!(radial_velocity([11., 0., 0.], 10., 20.), Some([0.; 3]));
        assert!(radial_velocity([f64::INFINITY, 0., 0.], 10., 20.).is_none());
        assert!(bounded_vector([f64::MAX; 3], 1.).is_none());
    }
}
