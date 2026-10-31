use bevy::prelude::*;
use std::f64::consts::TAU;

use crate::simulation::clock::{SimulationClock, SimulationClockSet};

/// Орбитальное движение планеты.
#[derive(Component, Debug, Clone)]
pub struct Orbit {
    pub visual_radius: f32,
    pub period_seconds: f64,
    pub phase_at_epoch: f64,
}

impl Orbit {
    pub fn earth() -> Self {
        Self {
            visual_radius: 7.0,
            period_seconds: 365.256 * 86_400.0,
            phase_at_epoch: 0.0,
        }
    }
}

/// Наклон оси и вращение планеты вокруг собственной оси.
#[derive(Component, Debug, Clone)]
pub struct AxialRotation {
    /// Угол наклона относительно нормали орбитальной плоскости.
    pub obliquity_radians: f32,

    /// Продолжительность звёздных суток в модельных секундах.
    pub rotation_period_seconds: f64,
}

impl AxialRotation {
    pub fn earth() -> Self {
        Self {
            obliquity_radians: 23.439281_f32.to_radians(),
            rotation_period_seconds: 86_164.0905,
        }
    }
}

/// Видимая сфера Солнца.
#[derive(Component)]
pub struct Sun;

/// Направленное освещение от Солнца.
#[derive(Component)]
pub struct Sunlight;

pub struct AstronomyPlugin;

impl Plugin for AstronomyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (update_orbits, update_sunlight_direction)
                .chain()
                .after(SimulationClockSet::Advance),
        );
    }
}

/// Обновляет орбитальное положение, наклон оси и суточное вращение.
fn update_orbits(
    clock: Res<SimulationClock>,
    mut query: Query<(&Orbit, &AxialRotation, &mut Transform)>,
) {
    // Орбита и суточное вращение не меняются на паузе.
    if clock.paused {
        return;
    }

    for (orbit, axial, mut transform) in &mut query {
        let orbit_period = orbit.period_seconds.max(1.0);

        let orbit_angle = (orbit.phase_at_epoch - clock.elapsed_seconds / orbit_period * TAU)
            .rem_euclid(TAU) as f32;

        transform.translation = Vec3::new(
            orbit.visual_radius * orbit_angle.cos(),
            0.0,
            orbit.visual_radius * orbit_angle.sin(),
        );

        let rotation_period = axial.rotation_period_seconds.max(1.0);

        let spin_angle = (clock.elapsed_seconds / rotation_period * std::f64::consts::TAU)
            .rem_euclid(std::f64::consts::TAU) as f32;

        // Орбитальная плоскость XZ, её нормаль — ось Y.
        // Поворот вокруг Z создаёт наклон 23,44°.
        let axial_tilt = Quat::from_rotation_z(axial.obliquity_radians);

        // Суточное вращение вокруг собственной оси Y.
        let daily_spin = Quat::from_rotation_y(spin_angle);

        // Наклон остаётся постоянным в пространстве,
        // а планета вращается вокруг наклонённой оси.
        transform.rotation = axial_tilt * daily_spin;
    }
}

/// Направляет солнечный свет от Солнца к планете.
fn update_sunlight_direction(
    planet_query: Query<&Transform, (With<Orbit>, Without<Sunlight>, Changed<Transform>)>,
    mut sunlight_query: Query<&mut Transform, (With<Sunlight>, Without<Orbit>)>,
) {
    let Ok(planet_transform) = planet_query.single() else {
        return;
    };

    let Ok(mut light_transform) = sunlight_query.single_mut() else {
        return;
    };

    let radial_direction = planet_transform.translation.normalize_or_zero();

    if radial_direction.length_squared() < 1e-6 {
        return;
    }

    let virtual_light_position = planet_transform.translation - radial_direction * 100.0;

    *light_transform = Transform::from_translation(virtual_light_position)
        .looking_at(planet_transform.translation, Vec3::Y);
}
