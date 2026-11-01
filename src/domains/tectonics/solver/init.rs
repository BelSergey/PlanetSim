// src/domains/tectonics/solver/init.rs

use super::{smoothstep, TectonicsGeometryCache};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{GeologyLayer, MantlePlume, Plate, Volcano},
    },
    world::topology::WorldTopology,
};
use bevy::math::{Quat, Vec3};
use noise::NoiseFn;
use rand::RngExt;
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

/// Вычисляет степень континентальности коры `[0.0 .. 1.0]` в данной точке для плиты `plate`.
/// Считается строго для собственной плиты (без перетекания через границы на чужие океанические плиты!).
#[inline]
pub(crate) fn craton_continental_factor(warped_dir: Vec3, plate: &Plate) -> f32 {
    if plate.craton_radius <= 1e-4 {
        return 0.0;
    }

    let d_craton = warped_dir.dot(plate.craton_center).clamp(-1.0, 1.0).acos();
    let local_p = (plate.accum_rot.inverse() * warped_dir).normalize_or_zero();
    let s = plate.seed_offset;

    // Крупные полуострова, заливы и неровности края кратона
    let lobe_1 = (local_p.x * 2.5 + s).sin() * (local_p.z * 2.5 - s * 0.7).cos();
    let lobe_2 = (local_p.y * 4.8 - s * 1.3).sin() * (local_p.x * 4.3 + local_p.z * 4.1).cos();
    let lobe_3 = (local_p.z * 8.2 + local_p.y * 7.4 + s * 2.1).sin();

    let shape_mod = 1.0 + lobe_1 * 0.36 + lobe_2 * 0.20 + lobe_3 * 0.10;
    let effective_radius = (plate.craton_radius * shape_mod).max(0.05);

    // Плавный спуск от материкового плато через континентальный шельф к подножию склона
    let slope_width = 0.18;
    let inner_edge = (effective_radius - slope_width * 0.50).max(0.01);
    1.0 - smoothstep((d_craton - inner_edge) / slope_width)
}

/// Находит 3 ближайшие плиты и возвращает истинные угловые расстояния до границ Вороного (в радианах)
#[inline]
pub(crate) fn find_closest_plates_and_boundary_dists(
    dir: Vec3,
    plates: &[Plate],
) -> (usize, usize, usize, f32, f32) {
    let mut dist_1 = f32::MAX;
    let mut dist_2 = f32::MAX;
    let mut dist_3 = f32::MAX;

    let mut p1_idx = 0;
    let mut p2_idx = 0;
    let mut p3_idx = 0;

    for (i, plate) in plates.iter().enumerate() {
        let d = dir.dot(plate.center_dir).clamp(-1.0, 1.0).acos();
        if d < dist_1 {
            dist_3 = dist_2;
            p3_idx = p2_idx;
            dist_2 = dist_1;
            p2_idx = p1_idx;
            dist_1 = d;
            p1_idx = i;
        } else if d < dist_2 {
            dist_3 = dist_2;
            p3_idx = p2_idx;
            dist_2 = d;
            p2_idx = i;
        } else if d < dist_3 {
            dist_3 = d;
            p3_idx = i;
        }
    }

    let c1 = plates[p1_idx].center_dir;
    let c2 = plates[p2_idx].center_dir;
    let c3 = plates[p3_idx].center_dir;

    let sep12 = c1.dot(c2).clamp(-1.0, 1.0).acos();
    let sep13 = c1.dot(c3).clamp(-1.0, 1.0).acos();

    let norm12 = (2.0 * (sep12 * 0.5).sin()).max(0.18);
    let norm13 = (2.0 * (sep13 * 0.5).sin()).max(0.18);

    let bd2 = ((dist_2 - dist_1) / norm12).max(0.0);
    let bd3 = ((dist_3 - dist_1) / norm13).max(0.0);

    (p1_idx, p2_idx, p3_idx, bd2, bd3)
}

pub fn generate_plates(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<Plate> {
    let num_plates = config.num_plates.max(3);
    let mut plates = Vec::with_capacity(num_plates);

    // Средний угловой радиус одной плиты на единичной сфере
    let avg_plate_radius = (4.0 * PI / num_plates as f32).sqrt() * 0.55;
    // Около 28-30% плит — чисто океанические (как Тихоокеанская, Наска, Кокос), остальные — смешанные
    let pure_frac = config.pure_oceanic_plate_fraction.clamp(0.22, 0.55);
    let num_pure_oceanic = ((num_plates as f32 * pure_frac).round() as usize)
        .clamp(2, num_plates.saturating_sub(2).max(1));

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

        let (craton_center, craton_radius) = if i < num_pure_oceanic {
            (best_dir, 0.0)
        } else {
            let shift_az: f32 = rng.random_range(0.0..(2.0 * PI));
            // Смещаем кратон в случайную сторону (иногда в центре, иногда у самого края плиты)
            let shift_dist: f32 = rng.random_range(
                (avg_plate_radius * 0.15)..(avg_plate_radius * config.craton_eccentricity.max(0.55)),
            );
            let tangent = random_tangent(best_dir, shift_az);
            let shifted_center = move_on_sphere(best_dir, tangent, shift_dist);

            // Калибруем радиус кратона так, чтобы суммарная доля суши составляла ~30% при continental_ratio = 0.42
            let cont_scale = (config.continental_ratio as f32).clamp(0.15, 0.85) * 1.18;
            let radius = avg_plate_radius * cont_scale * rng.random_range(0.72..1.18);
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
        let next_eruption_dist = config.hotspot_spacing_rad * rng.random_range(0.45..1.75);

        plumes.push(MantlePlume {
            position: pos,
            strength,
            radius,
            pulse_timer,
            pulse_period,
            next_eruption_dist,
            eruption_counter: rng.random_range(0..100),
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
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    geology.resize(num_cells);
    geology.sim_step = 0;

    if geology.plumes.is_empty() {
        geology.plumes = geology
            .volcanoes
            .iter()
            .enumerate()
            .map(|(idx, v)| MantlePlume {
                position: v.center.normalize_or_zero(),
                strength: v.height.clamp(config.hotspot_height_min, config.hotspot_height_max),
                radius: v.radius.max(config.volcano_radius_min),
                pulse_period: 0.16 + (idx % 5) as f32 * 0.04,
                pulse_timer: (idx % 3) as f32 * 0.05,
                next_eruption_dist: config.hotspot_spacing_rad
                    * (0.5 + ((idx * 37) % 10) as f32 * 0.14),
                eruption_counter: (idx as u32).wrapping_mul(17),
            })
            .collect();
    }
    geology.volcanoes.clear();

    super::boundaries::project_plates_and_boundaries(topology, cache, geology, config, 0.0);
    let _ = noise;
}