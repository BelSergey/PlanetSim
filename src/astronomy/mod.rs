// src/astronomy/mod.rs

use crate::simulation::clock::{SimulationClock, SimulationClockSet};
use crate::world::WorldConfig;
use bevy::prelude::*;
use std::f64::consts::TAU;

/// Ресурс текущего астрономического сезона, вычисляемый из положения на орбите.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct OrbitalSeasonState {
    /// Угол на орбите в радианах [0..TAU)
    pub orbit_angle: f32,
    /// Индекс сезона для Северного полушария:
    /// +1.0 = Летнее солнцестояние, 0.0 = Равноденствие (Весна/Осень), -1.0 = Зимнее солнцестояние
    pub season_factor: f32,
}

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

#[derive(Component, Debug, Clone)]
pub struct AxialRotation {
    pub obliquity_radians: f32,
    pub rotation_period_seconds: f64,
    /// Ограничитель визуальной скорости суточного вращения (рад/сек реального времени),
    /// чтобы при быстром движении по орбите планета не крутилась слишком бешено.
    pub max_visual_spin_speed: f32,
}

impl AxialRotation {
    pub fn earth() -> Self {
        Self {
            obliquity_radians: 23.439281_f32.to_radians(),
            rotation_period_seconds: 86_164.0905,
            max_visual_spin_speed: 0.8,
        }
    }
}

#[derive(Component)]
pub struct Sun;

/// Направленное освещение от Солнца.
#[derive(Component)]
pub struct Sunlight;

pub struct AstronomyPlugin;

impl Plugin for AstronomyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrbitalSeasonState>().add_systems(
            Update,
            (update_orbits, update_sunlight_direction)
                .chain()
                .after(SimulationClockSet::Advance),
        );
    }
}

/// Обновляет орбитальное положение, наклон оси и суточное вращение.
fn update_orbits(
    time: Res<Time>,
    clock: Res<SimulationClock>,
    world_config: Res<WorldConfig>,
    mut season_state: ResMut<OrbitalSeasonState>,
    mut last_system_scale: Local<Option<f32>>,
    mut visual_spin_angle: Local<f32>,
    mut query: Query<(&Orbit, &AxialRotation, &mut Transform), Without<Sun>>,
    mut sun_query: Query<&mut Transform, (With<Sun>, Without<Orbit>)>,
) {
    let system_scale = world_config.system_scale();

    let scale_changed = match *last_system_scale {
        Some(last) => (last - system_scale).abs() > 1e-6,
        None => true,
    };

    if scale_changed {
        *last_system_scale = Some(system_scale);
        if let Ok(mut sun_transform) = sun_query.single_mut() {
            sun_transform.scale = Vec3::splat(system_scale);
        }
    }

    if clock.paused && !scale_changed {
        return;
    }

    for (orbit, axial, mut transform) in &mut query {
        let orbit_period = orbit.period_seconds.max(1.0);
        let orbit_angle = (orbit.phase_at_epoch - clock.elapsed_seconds / orbit_period * TAU)
            .rem_euclid(TAU) as f32;

        season_state.orbit_angle = orbit_angle;
        season_state.season_factor = orbit_angle.cos();

        let orbit_radius = orbit.visual_radius * system_scale;
        transform.translation = Vec3::new(
            orbit_radius * orbit_angle.cos(),
            0.0,
            orbit_radius * orbit_angle.sin(),
        );

        if !clock.paused {
            let physical_spin_rate =
                (clock.time_scale / axial.rotation_period_seconds.max(1.0) * TAU) as f32;
            let effective_spin_rate = physical_spin_rate.min(axial.max_visual_spin_speed);
            *visual_spin_angle = (*visual_spin_angle + effective_spin_rate * time.delta_secs())
                .rem_euclid(TAU as f32);
        }

        let axial_tilt = Quat::from_rotation_z(axial.obliquity_radians);
        let daily_spin = Quat::from_rotation_y(*visual_spin_angle);

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
