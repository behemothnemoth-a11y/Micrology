//! A free-fly camera and cursor grabbing.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};
use std::f32::consts::FRAC_PI_2;

/// Marks the camera this module drives, and holds its tuning.
#[derive(Component)]
pub struct FlyCamera {
    /// Radians of rotation per pixel of mouse movement.
    pub sensitivity: f32,
    /// Cells per second at a walk.
    pub speed: f32,
    /// Multiplier while shift is held.
    pub boost: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            sensitivity: 0.0022,
            speed: 14.0,
            boost: 4.0,
        }
    }
}

/// Whether the cursor is currently captured for mouse-look.
pub fn cursor_grabbed(cursor: &CursorOptions) -> bool {
    cursor.grab_mode != CursorGrabMode::None
}

/// Click to capture the mouse, `Escape` to release it.
pub fn grab_cursor(
    cursor: Option<Single<&mut CursorOptions>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(cursor) = cursor else {
        return;
    };
    let mut cursor = cursor.into_inner();

    if keys.just_pressed(KeyCode::Escape) {
        cursor.visible = true;
        cursor.grab_mode = CursorGrabMode::None;
        return;
    }
    // Only the first click grabs; once grabbed, clicks are edits.
    if !cursor_grabbed(&cursor) && mouse.just_pressed(MouseButton::Left) {
        cursor.visible = false;
        cursor.grab_mode = CursorGrabMode::Locked;
    }
}

/// WASD to move, `Space`/`Control` for up and down, `Shift` to go faster,
/// mouse to look while the cursor is grabbed.
pub fn fly(
    cursor: Option<Single<&CursorOptions>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    camera: Option<Single<(&mut Transform, &FlyCamera)>>,
) {
    let Some(camera) = camera else {
        return;
    };
    let (mut transform, settings) = camera.into_inner();
    let grabbed = cursor.map(|c| cursor_grabbed(&c)).unwrap_or(false);

    if grabbed {
        let delta = mouse_motion.delta;
        if delta != Vec2::ZERO {
            let (yaw, pitch, roll) = transform.rotation.to_euler(EulerRot::YXZ);
            let yaw = yaw - delta.x * settings.sensitivity;
            // Clamping short of straight up avoids the gimbal flip that makes a
            // free camera feel broken at the extremes.
            const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
            let pitch = (pitch - delta.y * settings.sensitivity).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
        }
    }

    let mut direction = Vec3::ZERO;
    let forward = *transform.forward();
    let right = *transform.right();

    for (key, delta) in [
        (KeyCode::KeyW, forward),
        (KeyCode::KeyS, -forward),
        (KeyCode::KeyD, right),
        (KeyCode::KeyA, -right),
        (KeyCode::Space, Vec3::Y),
        (KeyCode::ControlLeft, Vec3::NEG_Y),
    ] {
        if keys.pressed(key) {
            direction += delta;
        }
    }

    if direction != Vec3::ZERO {
        let boost = if keys.pressed(KeyCode::ShiftLeft) {
            settings.boost
        } else {
            1.0
        };
        transform.translation += direction.normalize() * settings.speed * boost * time.delta_secs();
    }
}
