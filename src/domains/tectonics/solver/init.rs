// src/domains/tectonics/solver/init.rs

use super::{TectonicsGeometryCache, TectonicsScratch, smoothstep};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{GeologyLayer, MantlePlume, Plate, Volcano},
    },
    world::topology::WorldTopology,
};
use bevy::math::{Quat, Vec3};
use noise::NoiseFn;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::f32::consts::PI;

fn random_tangent(normal: Vec3, angle: f32) -> Vec3 {
    let helper = if normal.y.abs() < 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t1 = normal.cross(helper).normalize_or_zero();
    let t2 = normal.cross(t1).normalize_or_zero();
    (t1 * angle.cos() + t2 * angle.sin()).normalize_or_zero()
}

#[inline]
fn move_on_sphere(dir: Vec3, tangent: Vec3, arc_rad: f32) -> Vec3 {
    if tangent == Vec3::ZERO || arc_rad.abs() < 1e-6 {
        return dir;
    }
    (dir * arc_rad.cos() + tangent * arc_rad.sin()).normalize_or_zero()
}

/// Вычисляет степень континентальности коры `[0.0 .. 1.0]` в точке `dir` для плиты `plate`.
/// Все искажения формы вычисляются в локальной системе отсчёта плиты (`accum_rot`),
/// поэтому форма и площадь континента строго инвариантны при вращении плиты на сфере.
#[inline]
pub(crate) fn craton_continental_factor(dir: Vec3, plate: &Plate) -> f32 {
    if plate.craton_radius <= 1e-4 {
        return 0.0;
    }

    let d_craton = dir.dot(plate.craton_center).clamp(-1.0, 1.0).acos();
    let local_p = (plate.accum_rot.inverse() * dir).normalize_or_zero();
    let s = plate.seed_offset;

    let lobe_1 = (local_p.x * 2.4 + s).sin() * (local_p.z * 2.4 - s * 0.7).cos();
    let lobe_2 = (local_p.y * 4.5 - s * 1.3).sin() * (local_p.x * 4.1 + local_p.z * 3.9).cos();
    let lobe_3 = (local_p.z * 7.8 + local_p.y * 7.1 + s * 2.1).sin();

    let shape_mod = 1.0 + lobe_1 * 0.30 + lobe_2 * 0.16 + lobe_3 * 0.08;
    let land_radius = (plate.craton_radius * shape_mod).max(0.08);

    let shelf_and_slope_width = 0.15;
    let inner_plateau = (land_radius * 0.40).max(0.02);
    let outer_foot = land_radius + shelf_and_slope_width;

    if d_craton <= inner_plateau {
        1.0
    } else if d_craton <= land_radius {
        let t = smoothstep((d_craton - inner_plateau) / (land_radius - inner_plateau).max(1e-4));
        1.0 - t * 0.43
    } else if d_craton < outer_foot {
        let t = smoothstep((d_craton - land_radius) / shelf_and_slope_width);
        0.57 * (1.0 - t)
    } else {
        0.0
    }
}

/// Находит 3 ближайшие плиты и возвращает истинные геодезические расстояния до границ Вороного (в радианах)
#[inline]
pub(crate) fn find_closest_plates_and_boundary_dists(
    dir: Vec3,
    plates: &[Plate],
) -> (usize, usize, usize, f32, f32) {
    let mut dot_1 = -2.0_f32;
    let mut dot_2 = -2.0_f32;
    let mut dot_3 = -2.0_f32;

    let mut p1_idx = 0;
    let mut p2_idx = 0;
    let mut p3_idx = 0;

    for (i, plate) in plates.iter().enumerate() {
        let dot = dir.dot(plate.center_dir).clamp(-1.0, 1.0);
        if dot > dot_1 {
            dot_3 = dot_2;
            p3_idx = p2_idx;
            dot_2 = dot_1;
            p2_idx = p1_idx;
            dot_1 = dot;
            p1_idx = i;
        } else if dot > dot_2 {
            dot_3 = dot_2;
            p3_idx = p2_idx;
            dot_2 = dot;
            p2_idx = i;
        } else if dot > dot_3 {
            dot_3 = dot;
            p3_idx = i;
        }
    }

    let c1 = plates[p1_idx].center_dir;
    let c2 = plates[p2_idx].center_dir;
    let c3 = plates[p3_idx].center_dir;

    // Длина хорды ||c1 - c2|| и ||c1 - c3||
    let chord12 = (c1 - c2).length().max(0.18);
    let chord13 = (c1 - c3).length().max(0.18);

    // Точное угловое расстояние от точки сферы до плоскости биссектрисы Вороного: sin(bd) = (dot_1 - dot_k) / ||c1 - ck||
    let bd2 = ((dot_1 - dot_2) / chord12).clamp(0.0, 1.0).asin();
    let bd3 = ((dot_1 - dot_3) / chord13).clamp(0.0, 1.0).asin();

    (p1_idx, p2_idx, p3_idx, bd2, bd3)
}

pub fn generate_plates(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<Plate> {
    let num_plates = config.num_plates.max(3);
    let mut plates = Vec::with_capacity(num_plates);

    let plate_angular_radius = (4.0 / num_plates as f32).sqrt();
    let pure_frac = config.pure_oceanic_plate_fraction.clamp(0.12, 0.45);
    let num_pure_oceanic = ((num_plates as f32 * pure_frac).round() as usize)
        .clamp(1, num_plates.saturating_sub(2).max(1));
    let oceanic_stride = (num_plates / num_pure_oceanic.max(1)).max(2);

    let mut pure_assigned = 0usize;

    for i in 0..num_plates {
        let mut best_dir = Vec3::Y;
        let mut best_dist = -1.0;

        for _ in 0..config.plate_dir_iterations {
            let z: f32 = rng.random_range(-1.0..1.0);
            let lon: f32 = rng.random_range(-PI..PI);
            let r = (1.0 - z * z).sqrt();
            let candidate = Vec3::new(r * lon.cos(), z, r * lon.sin());

            let min_dist = plates
                .iter()
                .map(|p: &Plate| candidate.dot(p.center_dir).clamp(-1.0, 1.0).acos())
                .fold(f32::MAX, f32::min);

            if min_dist > best_dist {
                best_dist = min_dist;
                best_dir = candidate;
            }
        }

        let rand_vec = Vec3::new(
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
        )
        .normalize_or_zero();
        let euler_pole = if rand_vec == Vec3::ZERO {
            Vec3::Y
        } else {
            rand_vec
        };
        let angular_vel = rng.random_range(config.angular_vel_min..config.angular_vel_max);

        let var = config.plate_base_height_variance.max(0.0);
        let base_elevation_offset = if var > 0.0 {
            rng.random_range(-var..var)
        } else {
            0.0
        };

        let is_pure_ocean = (i % oceanic_stride == 1) && (pure_assigned < num_pure_oceanic);

        let (craton_center, craton_radius) = if is_pure_ocean {
            pure_assigned += 1;
            (best_dir, 0.0)
        } else {
            let shift_az: f32 = rng.random_range(0.0..(2.0 * PI));
            let max_shift =
                plate_angular_radius * config.craton_eccentricity.clamp(0.1, 1.0) * 0.35;
            let shift_dist: f32 = rng.random_range((max_shift * 0.15)..max_shift.max(0.03));
            let tangent = random_tangent(best_dir, shift_az);
            let shifted_center = move_on_sphere(best_dir, tangent, shift_dist);

            let cont_ratio = (config.continental_ratio as f32).clamp(0.15, 0.85);
            let base_rad = plate_angular_radius * (0.48 + 0.85 * cont_ratio);
            let radius = base_rad * rng.random_range(0.84..1.18);
            (shifted_center, radius)
        };

        let seed_offset = rng.random_range(0.0..100.0);
        let color_hue = ((i as f32 * 137.50776) % 360.0).clamp(0.0, 360.0);

        plates.push(Plate {
            center_dir: best_dir,
            euler_pole,
            angular_vel,
            omega: euler_pole * angular_vel,
            accum_rot: Quat::IDENTITY,
            base_elevation_offset,
            craton_center,
            craton_radius,
            seed_offset,
            color_hue,
        });
    }

    plates
}

pub fn generate_plumes(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<MantlePlume> {
    let mut plumes = Vec::with_capacity(config.hotspot_count);
    for _ in 0..config.hotspot_count {
        let pos = Vec3::new(
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
        )
        .normalize_or_zero();

        if pos == Vec3::ZERO {
            continue;
        }

        let strength = rng.random_range(config.hotspot_height_min..config.hotspot_height_max);
        let radius = config.hotspot_radius_base * rng.random_range(0.85..1.45);
        let pulse_period = rng.random_range(0.12..0.32);
        let pulse_timer = rng.random_range(0.0..pulse_period);
        let next_eruption_dist = config.hotspot_spacing_rad * rng.random_range(0.45..1.45);
        let accum_drift_rad = next_eruption_dist * rng.random_range(0.55..0.98);

        plumes.push(MantlePlume {
            position: pos,
            strength,
            radius,
            pulse_timer,
            pulse_period,
            accum_drift_rad,
            next_eruption_dist,
            eruption_counter: rng.random_range(1..100),
        });
    }
    plumes
}

pub fn generate_hotspots(
    rng: &mut impl RngExt,
    plates: &[Plate],
    config: &TectonicsConfig,
) -> Vec<Volcano> {
    let plumes = generate_plumes(rng, config);
    plumes
        .into_iter()
        .map(|plume| {
            let mut best_dist = f32::MAX;
            let mut plate_id = 0;
            for (i, plate) in plates.iter().enumerate() {
                let d = plume.position.dot(plate.center_dir).clamp(-1.0, 1.0).acos();
                if d < best_dist {
                    best_dist = d;
                    plate_id = i;
                }
            }
            Volcano {
                center: plume.position,
                height: plume.strength,
                radius: plume.radius,
                plate_id,
                age: 0.0,
            }
        })
        .collect()
}

pub fn init_tectonics(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    geology.resize(num_cells);
    geology.sim_step = 0;

    // Защита от вызова на пустом GeologyLayer: генерируем плиты и плюмы автоматически
    if geology.plates.len() < 3 {
        let mut fallback_rng = StdRng::seed_from_u64(0x9E37_79B9_7F4A_7C15);
        geology.plates = generate_plates(&mut fallback_rng, config);
        if geology.plumes.is_empty() && geology.volcanoes.is_empty() {
            geology.plumes = generate_plumes(&mut fallback_rng, config);
        }
    }

    if geology.plumes.is_empty() {
        if !geology.volcanoes.is_empty() {
            geology.plumes = geology
                .volcanoes
                .iter()
                .enumerate()
                .map(|(idx, v)| {
                    let next_dist =
                        config.hotspot_spacing_rad * (0.45 + ((idx * 37) % 10) as f32 * 0.10);
                    MantlePlume {
                        position: v.center.normalize_or_zero(),
                        strength: v
                            .height
                            .clamp(config.hotspot_height_min, config.hotspot_height_max),
                        radius: v.radius.max(config.volcano_radius_min),
                        pulse_period: 0.16 + (idx % 5) as f32 * 0.04,
                        pulse_timer: (idx % 3) as f32 * 0.05,
                        accum_drift_rad: next_dist * 0.85,
                        next_eruption_dist: next_dist,
                        eruption_counter: (idx as u32).wrapping_mul(17).wrapping_add(1),
                    }
                })
                .collect();
        } else if config.hotspot_count > 0 {
            let mut fallback_rng = StdRng::seed_from_u64(0x85EB_CA6B_C2B2_AE35);
            geology.plumes = generate_plumes(&mut fallback_rng, config);
        }
    }
    geology.volcanoes.clear();

    super::boundaries::project_plates_and_boundaries(
        topology, cache, geology, scratch, config, 0.0,
    );
    // Гарантируем рождение стартовых вулканов, даже если simulation_steps == 0
    super::boundaries::spawn_hotspot_volcanoes_on_plates(
        topology,
        geology,
        config,
        config.time_step.max(0.015),
    );
    let _ = noise;
}
