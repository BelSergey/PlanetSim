// src/domains/tectonics/solver/init.rs

use super::{smoothstep, TectonicsGeometryCache};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{GeologyLayer, MantlePlume, Plate, Volcano},
    },
    world::topology::WorldTopology,
};
use bevy::math::Vec3;
use noise::NoiseFn;
use rand::RngExt;
use rayon::prelude::*;
use std::f32::consts::PI;

/// Создает произвольный единичный вектор в касательной плоскости сферы в точке `normal`
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

/// Поворачивает вектор `dir` вдоль касательной `tangent` на угол `arc_rad` радиан
#[inline]
fn move_on_sphere(dir: Vec3, tangent: Vec3, arc_rad: f32) -> Vec3 {
    if tangent == Vec3::ZERO || arc_rad.abs() < 1e-6 {
        return dir;
    }
    (dir * arc_rad.cos() + tangent * arc_rad.sin()).normalize_or_zero()
}

/// Генерирует тектонические плиты: большинство смешанные (со смещенным континентальным кратоном),
/// а небольшая доля (`pure_oceanic_plate_fraction`, минимум 1) — чисто океанические
pub fn generate_plates(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<Plate> {
    let num_plates = config.num_plates.max(2);
    let mut plates = Vec::with_capacity(num_plates);

    // Средний угловой радиус одной плиты на сфере
    let avg_plate_radius = (4.0 * PI / num_plates as f32).sqrt() * 0.56;
    let num_pure_oceanic = ((num_plates as f32 * config.pure_oceanic_plate_fraction).round()
        as usize)
        .clamp(1, num_plates - 1);

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

        // Индивидуальная базовая высота плиты (случайная плавучесть)
        let var = config.plate_base_height_variance.max(0.0);
        let base_elevation_offset = if var > 0.0 {
            rng.random_range(-var..var)
        } else {
            0.0
        };

        // Первые `num_pure_oceanic` плит делаем чисто океаническими (как Тихоокеанская),
        // остальные — смешанными с эксцентрично смещенным континентальным кратоном
        let (craton_center, craton_radius) = if i < num_pure_oceanic {
            (best_dir, 0.0)
        } else {
            let shift_az: f32 = rng.random_range(0.0..(2.0 * PI));
            let shift_dist: f32 =
                rng.random_range(0.0..(avg_plate_radius * config.craton_eccentricity));
            let tangent = random_tangent(best_dir, shift_az);
            let shifted_center = move_on_sphere(best_dir, tangent, shift_dist);

            let cont_scale = (config.continental_ratio as f32).clamp(0.15, 0.85) * 1.65;
            let radius =
                avg_plate_radius * cont_scale * rng.random_range(0.75..1.30);
            (shifted_center, radius)
        };

        // Золотое сечение для равномерного распределения оттенков плит
        let color_hue = ((i as f32 * 137.50776) % 360.0).clamp(0.0, 360.0);

        plates.push(Plate {
            center_dir: best_dir,
            euler_pole,
            angular_vel,
            omega: euler_pole * angular_vel,
            base_elevation_offset,
            craton_center,
            craton_radius,
            color_hue,
        });
    }

    plates
}

/// Генерирует стационарные мантийные плюмы
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
        let radius = config.hotspot_radius_base * rng.random_range(0.85..1.25);
        let pulse_period = rng.random_range(0.12..0.28);
        let pulse_timer = rng.random_range(0.0..pulse_period);

        plumes.push(MantlePlume {
            position: pos,
            strength,
            radius,
            pulse_timer,
            pulse_period,
        });
    }
    plumes
}

/// Совместимая с `generator.rs` функция: создает стартовые точки вулканов над плюмами.
/// Сами вулканические цепи теперь рождаются честно в процессе движения плит над плюмами!
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

/// Инициализирует физические поля коры (толщину, плотность, возраст) перед запуском шагов движения
pub fn init_tectonics(
    topology: &WorldTopology,
    _cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    geology.resize(num_cells);
    geology.sim_step = 0;

    // Если плюмы еще не инициализированы, извлекаем их стационарные позиции из стартовых вулканов
    if geology.plumes.is_empty() {
        geology.plumes = geology
            .volcanoes
            .iter()
            .enumerate()
            .map(|(idx, v)| MantlePlume {
                position: v.center.normalize_or_zero(),
                strength: v.height.clamp(config.hotspot_height_min, config.hotspot_height_max),
                radius: v.radius.max(config.volcano_radius_min),
                pulse_period: 0.16 + (idx % 5) as f32 * 0.03,
                pulse_timer: (idx % 3) as f32 * 0.05,
            })
            .collect();
    }
    // Очищаем список вулканов: они честно нарисуются по мере прохода плит над плюмами
    geology.volcanoes.clear();

    let plates = &geology.plates;
    if plates.is_empty() {
        return;
    }

    let plate_ids = &mut geology.plate_ids;
    let thickness = &mut geology.crust_thickness;
    let density = &mut geology.crust_density;
    let age = &mut geology.crust_age;
    let sediment = &mut geology.sediment_thickness;
    let stress = &mut geology.tectonic_stress;
    let orogeny = &mut geology.orogeny_root;
    let shifts = &mut geology.subcell_shift;

    plate_ids
        .par_iter_mut()
        .zip(thickness.par_iter_mut())
        .zip(density.par_iter_mut())
        .zip(age.par_iter_mut())
        .zip(sediment.par_iter_mut())
        .zip(stress.par_iter_mut())
        .zip(orogeny.par_iter_mut())
        .zip(shifts.par_iter_mut())
        .enumerate()
        .for_each(
            |(
                cell_id,
                (
                    (
                        (((((p_out, thick_out), dens_out), age_out), sed_out), stress_out),
                        orog_out,
                    ),
                    shift_out,
                ),
            )| {
                let dir = topology.cells[cell_id].center;
                let p = dir.as_dvec3() * config.warp_scale;
                let offsets = &config.warp_offsets;

                let wx = noise.get([
                    p.x + offsets[0][0],
                    p.y + offsets[0][1],
                    p.z + offsets[0][2],
                ]) as f32;
                let wy = noise.get([
                    p.x + offsets[1][0],
                    p.y + offsets[1][1],
                    p.z + offsets[1][2],
                ]) as f32;
                let wz = noise.get([
                    p.x + offsets[2][0],
                    p.y + offsets[2][1],
                    p.z + offsets[2][2],
                ]) as f32;

                let warped_dir =
                    (dir + Vec3::new(wx, wy, wz) * config.warp_strength).normalize_or_zero();

                // Находим ближайшую плиту для ячейки
                let mut best_plate = 0;
                let mut best_dist = f32::MAX;
                for (i, plate) in plates.iter().enumerate() {
                    let d = warped_dir.dot(plate.center_dir).clamp(-1.0, 1.0).acos();
                    if d < best_dist {
                        best_dist = d;
                        best_plate = i;
                    }
                }

                *p_out = best_plate;
                let plate = &plates[best_plate];

                // Определяем, попадает ли клетка в континентальный кратон плиты
                let craton_noise =
                    noise.get([p.x * 1.35 - 19.2, p.y * 1.35 + 27.4, p.z * 1.35 - 8.1]) as f32;
                let d_craton = warped_dir.dot(plate.craton_center).clamp(-1.0, 1.0).acos();
                let effective_radius =
                    (plate.craton_radius * (1.0 + craton_noise * 0.38)).max(0.0);

                // Плавная переходная зона (континентальный шельф и склон) шириной 0.14 рад
                let shelf_width = 0.14;
                let cont_factor = if effective_radius <= 1e-4 {
                    0.0
                } else {
                    1.0 - smoothstep((d_craton - (effective_radius - shelf_width)) / shelf_width)
                };

                if cont_factor > 0.05 {
                    // Смешанная/континентальная кора (ядро кратона + пассивный шельф по краю)
                    let shield_var = craton_noise * 3.5;
                    *thick_out = config.ocean_crust_thickness_km
                        + cont_factor
                            * (config.cont_crust_thickness_km - config.ocean_crust_thickness_km)
                        + shield_var * cont_factor;
                    *dens_out = config.old_ocean_density
                        - cont_factor * (config.old_ocean_density - config.cont_crust_density);
                    *age_out = 0.4 + cont_factor * (2.5 + craton_noise.abs());
                    *sed_out = 0.25 + (1.0 - cont_factor) * cont_factor * 2.2;
                    // Древние остаточные складчатые пояса внутри кратона
                    *orog_out = (craton_noise - 0.35).max(0.0) * 0.35 * cont_factor;
                } else {
                    // Океаническая кора по краям смешанной плиты или на чисто океанической плите
                    let age_init = (d_craton * 0.65).clamp(0.08, config.oceanic_mature_age * 0.85);
                    let cooling = (age_init / config.oceanic_mature_age).clamp(0.0, 1.0).sqrt();
                    *thick_out = config.ocean_crust_thickness_km;
                    *dens_out = config.young_ocean_density
                        + cooling * (config.old_ocean_density - config.young_ocean_density);
                    *age_out = age_init;
                    *sed_out = 0.08 * cooling;
                    *orog_out = 0.0;
                }

                *stress_out = 0.0;
                *shift_out = [0.0; 3];
            },
        );
}