// src/domains/tectonics/solver/boundaries.rs

use super::{
    init::{craton_continental_factor, find_closest_plates},
    smoothstep, TectonicsGeometryCache, TectonicsScratch,
};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{GeologyLayer, Volcano},
    },
    world::topology::WorldTopology,
};
use bevy::math::Vec3;
use rayon::prelude::*;
use std::f32::consts::PI;

/// Быстрый детерминированный 3D-шум (без внешних аллокаций, готов для WGSL шейдера)
#[inline]
pub(crate) fn fast_domain_warp(dir: Vec3, scale: f32, strength: f32) -> Vec3 {
    let p = dir * scale;
    let wx = (p.x * 1.7 + p.y * 2.3 + 12.3).sin() * (p.z * 1.9 - 4.5).cos()
        + 0.5 * (p.y * 3.7 - p.z * 3.1 + 7.1).sin();
    let wy = (p.y * 1.8 + p.z * 2.1 + 45.6).sin() * (p.x * 1.6 + 8.2).cos()
        + 0.5 * (p.z * 3.5 - p.x * 2.9 - 3.4).sin();
    let wz = (p.z * 1.5 + p.x * 2.4 + 78.9).sin() * (p.y * 1.8 - 6.7).cos()
        + 0.5 * (p.x * 3.3 + p.y * 3.6 + 5.9).sin();

    (dir + Vec3::new(wx, wy, wz) * (strength * 0.45)).normalize_or_zero()
}

/// Детектирует границы плит для расчёта геодинамических сил в `forces.rs`
pub fn detect_plate_boundaries(
    topology: &WorldTopology,
    _cache: &TectonicsGeometryCache,
    geology: &GeologyLayer,
    scratch: &mut TectonicsScratch,
) {
    let plates = &geology.plates;
    if plates.len() < 2 {
        return;
    }

    let conv_out = &mut scratch.boundary_convergence;
    let shear_out = &mut scratch.boundary_shear;
    let nbr_p_out = &mut scratch.neighbor_plate;
    let norm_out = &mut scratch.boundary_normal;

    conv_out
        .par_iter_mut()
        .zip(shear_out.par_iter_mut())
        .zip(nbr_p_out.par_iter_mut())
        .zip(norm_out.par_iter_mut())
        .enumerate()
        .for_each(|(i, (((conv, shear), other_p), normal))| {
            let dir = topology.cells[i].center;
            let warped = fast_domain_warp(dir, config_warp_scale(), 0.42);
            let (p1_idx, p2_idx, _, d1, d2, _) = find_closest_plates(warped, plates);

            let bd2 = d2 - d1;
            if bd2 < 0.22 {
                let p1 = &plates[p1_idx];
                let p2 = &plates[p2_idx];
                let n12 = (p2.center_dir - p1.center_dir).normalize_or_zero();
                let v1 = p1.omega.cross(dir);
                let v2 = p2.omega.cross(dir);
                let rel_v = v1 - v2;

                let decay = 1.0 - (bd2 / 0.22);
                let c = rel_v.dot(n12);
                *conv = c * decay;
                *shear = (rel_v - n12 * c).length() * decay;
                *other_p = p2_idx;
                *normal = n12;
            } else {
                *conv = 0.0;
                *shear = 0.0;
                *other_p = usize::MAX;
                *normal = Vec3::ZERO;
            }
        });
}

#[inline]
fn config_warp_scale() -> f32 {
    1.6
}

/// Проецирует гладкие границы движущихся плит, кратонов и зон субдукции/коллизии/спрединга на сетку
pub fn project_plates_and_boundaries(
    topology: &WorldTopology,
    _cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
    dt: f32,
) {
    let plates = &geology.plates;
    let plumes = &geology.plumes;
    if plates.len() < 3 {
        return;
    }

    let ref_speed = (config.angular_vel_max * 1.5).max(0.5);
    let edge_thresh = config.edge_threshold.max(0.28);

    let plate_ids = &mut geology.plate_ids;
    let thickness = &mut geology.crust_thickness;
    let density = &mut geology.crust_density;
    let age = &mut geology.crust_age;
    let sediment = &mut geology.sediment_thickness;
    let stress = &mut geology.tectonic_stress;
    let orogeny = &mut geology.orogeny_root;

    plate_ids
        .par_iter_mut()
        .zip(thickness.par_iter_mut())
        .zip(density.par_iter_mut())
        .zip(age.par_iter_mut())
        .zip(sediment.par_iter_mut())
        .zip(stress.par_iter_mut())
        .zip(orogeny.par_iter_mut())
        .enumerate()
        .for_each(
            |(
                i,
                (
                    (((((p_out, thick_out), dens_out), age_out), sed_out), stress_out),
                    orog_out,
                ),
            )| {
                let dir = topology.cells[i].center;
                let warped_dir =
                    fast_domain_warp(dir, config.warp_scale as f32, config.warp_strength);

                // 1. Находим 3 ближайшие плиты в искаженных координатах (без ступенек и гребёнки!)
                let (p1_idx, p2_idx, p3_idx, dist_1, dist_2, dist_3) =
                    find_closest_plates(warped_dir, plates);
                *p_out = p1_idx;

                let p1 = &plates[p1_idx];
                let p2 = &plates[p2_idx];
                let p3 = &plates[p3_idx];

                let bd2 = (dist_2 - dist_1).max(0.0);
                let bd3 = (dist_3 - dist_1).max(0.0);

                // Веса IDW для гладкого смешивания в тройных стыках
                let eps = 0.0002_f32;
                let w2 = 1.0 / (bd2 * bd2 + eps);
                let w3 = 1.0 / (bd3 * bd3 + eps);
                let w_sum = w2 + w3;
                let weight2 = w2 / w_sum;
                let weight3 = w3 / w_sum;

                // 2. Определяем степень континентальности текущей клетки и соседних плит (p2, p3)
                let c1 = craton_continental_factor(warped_dir, p1);
                let c2 = craton_continental_factor(warped_dir, p2);
                let c3 = craton_continental_factor(warped_dir, p3);

                // Плавное сглаживание базовой континентальности через границу (создает пологие шельфы)
                let shelf_blend_2 = smoothstep(bd2 / 0.20);
                let shelf_blend_3 = smoothstep(bd3 / 0.20);
                let blended_c2 = c1 * shelf_blend_2 + (c1 + c2) * 0.5 * (1.0 - shelf_blend_2);
                let blended_c3 = c1 * shelf_blend_3 + (c1 + c3) * 0.5 * (1.0 - shelf_blend_3);
                let cont_factor = (blended_c2 * weight2 + blended_c3 * weight3).clamp(0.0, 1.0);

                // 3. Относительные скорости на границах 1-2 и 1-3
                let v1 = p1.omega.cross(dir);
                let normal2 = (p2.center_dir - p1.center_dir).normalize_or_zero();
                let v2 = p2.omega.cross(dir);
                let conv2 = ((v1 - v2).dot(normal2) / ref_speed).clamp(-1.2, 1.2);

                let normal3 = (p3.center_dir - p1.center_dir).normalize_or_zero();
                let v3 = p3.omega.cross(dir);
                let conv3 = ((v1 - v3).dot(normal3) / ref_speed).clamp(-1.2, 1.2);

                // Структурный 3D-шум для разбиения линейных хребтов на отдельные горные массивы и перевалы
                let mp = warped_dir * (config.boundary_noise_scale as f32 * 0.65);
                let ridge_mod = 0.65
                    + 0.35
                        * ((mp.x * 1.4 + mp.y * 1.9).sin() * (mp.z * 1.7 - mp.x * 1.1).cos())
                            .abs();

                // 4. Расчёт вклада от границы 2 и границы 3
                let f2 = (1.0 - bd2 / edge_thresh).clamp(0.0, 1.0);
                let f3 = (1.0 - bd3 / edge_thresh).clamp(0.0, 1.0);

                let (dh2, orog2, age_mod2) =
                    boundary_crust_response(c1, c2, f2, conv2, ridge_mod, config);
                let (dh3, orog3, age_mod3) =
                    boundary_crust_response(c1, c3, f3, conv3, ridge_mod, config);

                let boundary_thick_delta = dh2 * weight2 + dh3 * weight3;
                let target_orogeny = (orog2 * weight2 + orog3 * weight3).clamp(0.0, 1.0);
                let local_stress = (conv2 * f2 * weight2 + conv3 * f3 * weight3).clamp(-1.2, 1.2);

                // 5. Возраст коры: в зонах океанического спрединга (age_mod -> 0) кора молодая и легкая,
                // вдали от хребтов — старая, холодная и плотная (закон Парсонса — Склейтера)
                let spread_reset = (age_mod2 * weight2 + age_mod3 * weight3).clamp(0.0, 1.0);
                let ocean_age = ((bd2 * weight2 + bd3 * weight3) * 2.2 * spread_reset)
                    .clamp(0.02, config.oceanic_mature_age);

                *age_out = if cont_factor > 0.35 {
                    0.8 + cont_factor * 2.2
                } else {
                    ocean_age
                };

                // 6. Плотность и толщина коры (плавный переход континент <-> океан + деформация границ)
                let cooling = (ocean_age / config.oceanic_mature_age).clamp(0.0, 1.0).sqrt();
                let ocean_dens = config.young_ocean_density
                    + cooling * (config.old_ocean_density - config.young_ocean_density);

                let target_dens =
                    ocean_dens - cont_factor * (ocean_dens - config.cont_crust_density);
                let base_thick = config.ocean_crust_thickness_km
                    + cont_factor
                        * (config.cont_crust_thickness_km - config.ocean_crust_thickness_km);

                // Накапливаем орогенез во времени (по мере движения плит старые горные пояса плавно сглаживаются, а новые растут)
                if dt > 0.0 {
                    let keep = (1.0 - 0.14 * dt).clamp(0.85, 0.99);
                    *orog_out = (*orog_out * keep).max(target_orogeny);
                } else {
                    *orog_out = target_orogeny;
                }

                // Подъем коры над мантийными плюмами
                let mut plume_thick = 0.0_f32;
                for plume in plumes {
                    let d = dir.dot(plume.position).clamp(-1.0, 1.0).acos();
                    if d < plume.radius * 1.8 {
                        let pf = smoothstep(1.0 - d / (plume.radius * 1.8));
                        plume_thick += pf * plume.strength * 6.5;
                    }
                }

                let historical_mtn_thick = *orog_out * 18.0;
                *thick_out = (base_thick
                    + boundary_thick_delta.max(historical_mtn_thick * 0.65)
                    + plume_thick)
                    .clamp(3.8, config.max_crust_thickness_km);
                *dens_out = target_dens;
                *stress_out = local_stress;

                if cont_factor > 0.15 && cont_factor < 0.75 {
                    // Накопление осадков на континентальных шельфах и в предгорных прогибах
                    *sed_out = (*sed_out + 0.15 * dt).clamp(0.3, 4.5);
                }
            },
        );
}

/// Вычисляет гладкий профиль изменения толщины коры `dh_km`, складчатости `orogeny` и множителя возраста
/// для пары клеток с континентальностью `c1` (наша плита) и `c2` (соседняя плита)
#[inline]
fn boundary_crust_response(
    c1: f32,
    c2: f32,
    f: f32,
    convergence: f32,
    ridge_mod: f32,
    _config: &TectonicsConfig,
) -> (f32, f32, f32) {
    if f <= 1e-4 {
        return (0.0, 0.0, 1.0);
    }

    let is_c1 = c1 > 0.40;
    let is_c2 = c2 > 0.40;

    if convergence >= 0.0 {
        // === СХОЖДЕНИЕ ПЛИТ (КОЛЛИЗИЯ И СУБДУКЦИЯ) ===
        let conv_mag = convergence.clamp(0.0, 1.2);

        match (is_c1, is_c2) {
            (true, true) => {
                // 1. Коллизия двух континентов (Гималаи / Тибет): широкий складчатый пояс
                let bell = (f * PI * 0.85).sin().powi(2);
                let dh = bell * 28.0 * ridge_mod * conv_mag;
                let orog = (bell * 0.95 * conv_mag).clamp(0.0, 1.0);
                (dh, orog, 1.0)
            }
            (true, false) => {
                // 2. Субдукция, сторона континента (Анды): хребет плавно отступает от берега вглубь суши
                let arc_zone = ((f - 0.16) * 1.75).clamp(0.0, 1.0);
                let arc_bell = (arc_zone * PI).sin().powi(2);
                let dh = arc_bell * 24.0 * ridge_mod * conv_mag;
                let orog = (arc_bell * 0.88 * conv_mag).clamp(0.0, 1.0);
                (dh, orog, 1.0)
            }
            (false, true) => {
                // 3. Субдукция, сторона океана: плавный глубоководный желоб перед континентом
                let trench_zone = ((f - 0.18) * 1.8).clamp(0.0, 1.0);
                let trench_bell = (trench_zone * PI).sin().powi(2);
                let dh = -trench_bell * 5.5 * conv_mag;
                (dh, 0.0, 1.0)
            }
            (false, false) => {
                // 4. Океан-океан (Островная дуга): желоб у самой границы + гряда островов чуть позади
                let arc_zone = ((f - 0.22) * 2.1).clamp(0.0, 1.0);
                let arc_bell = (arc_zone * PI).sin().powi(2);
                let trench_bell = ((f - 0.82).max(0.0) / 0.18).powi(2);
                let dh = (arc_bell * 16.5 * ridge_mod - trench_bell * 4.5) * conv_mag;
                let orog = (arc_bell * 0.75 * conv_mag).clamp(0.0, 1.0);
                (dh, orog, 1.0)
            }
        }
    } else {
        // === РАСХОЖДЕНИЕ ПЛИТ (РИФТЫ И СПРЕДИНГ СОХ) ===
        let div_mag = (-convergence).clamp(0.0, 1.2);

        match (is_c1, is_c2) {
            (true, true) => {
                // 5. Континентальный рифт (Восточная Африка / Байкал): грабен в центре + плечи горстов
                let graben = -f.powi(4) * 14.0;
                let horsts = (f * PI).sin().powi(2) * 8.5 * ridge_mod;
                let dh = (graben + horsts) * div_mag;
                let orog = ((f * PI).sin().powi(2) * 0.45 * div_mag).clamp(0.0, 1.0);
                (dh, orog, 1.0)
            }
            (false, false) => {
                // 6. Срединно-океанический хребет (СОХ): плавный вал с узким осевым рифтом и молодой корой
                let ridge_swell = (f * PI * 0.65).sin().powi(2) * 4.2 * (0.7 + 0.3 * ridge_mod);
                let axial_cleft = -f.powi(6) * 2.2;
                let dh = (ridge_swell + axial_cleft) * div_mag;
                let age_mod = (1.0 - f).clamp(0.0, 1.0);
                (dh, 0.15 * f * div_mag, age_mod)
            }
            _ => {
                // 7. Пассивная окраина (расхождение океан-континент): мягкое опускание шельфа
                let shelf_dip = -(f * PI).sin().powi(2) * 4.0 * div_mag;
                (shelf_dip, 0.0, 1.0 - 0.5 * f)
            }
        }
    }
}

pub fn apply_boundary_and_plume_processes(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let _ = scratch;
    project_plates_and_boundaries(topology, cache, geology, config, dt);
    spawn_hotspot_volcanoes_on_plates(topology, geology, config);
}

/// Стохастическое рождение вулканов над плюмами (кластеры и паузы вместо равномерного частокола!)
fn spawn_hotspot_volcanoes_on_plates(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_plates = geology.plates.len();
    if num_plates == 0 {
        return;
    }

    for (p_idx, plume) in geology.plumes.iter_mut().enumerate() {
        let min_dist_to_existing = geology
            .volcanoes
            .iter()
            .map(|v| v.center.dot(plume.position).clamp(-1.0, 1.0).acos())
            .fold(f32::MAX, f32::min);

        if min_dist_to_existing >= plume.next_eruption_dist {
            // Детерминированный псевдослучайный хеш для каждого извержения
            plume.eruption_counter = plume.eruption_counter.wrapping_add(1);
            let h1 = ((plume.eruption_counter as f32 * 12.9898 + p_idx as f32 * 78.233).sin()
                * 43758.547)
                .fract()
                .abs();
            let h2 = ((plume.eruption_counter as f32 * 93.989 + p_idx as f32 * 19.17).sin()
                * 28461.33)
                .fract()
                .abs();
            let h3 = ((plume.eruption_counter as f32 * 41.33 + p_idx as f32 * 53.71).cos()
                * 19371.7)
                .fract()
                .abs();

            // Находим плиту над плюмом
            let mut best_cell = 0;
            let mut best_dot = -1.0_f32;
            for (cid, cell) in topology.cells.iter().enumerate() {
                let d = cell.center.dot(plume.position);
                if d > best_dot {
                    best_dot = d;
                    best_cell = cid;
                }
            }
            let plate_id = geology.plate_ids[best_cell];

            // Легкий поперечный разброс позиции жерла относительно центра плюма, чтобы цепь не была по линейке
            let jitter_tangent = Vec3::new(h1 - 0.5, h2 - 0.5, h3 - 0.5)
                .cross(plume.position)
                .normalize_or_zero();
            let jitter_angle = (h1 - 0.5) * config.hotspot_spacing_rad * 0.42;
            let volcano_pos = (plume.position + jitter_tangent * jitter_angle).normalize_or_zero();

            // Случайная высота и радиус конкретного вулкана в гряде
            let height = (config.hotspot_height_min
                + h2.powf(1.25) * (config.hotspot_height_max - config.hotspot_height_min))
                * (0.65 + 0.50 * (plume.strength / config.hotspot_height_max.max(0.1)));
            let radius = (config.hotspot_radius_base * (0.70 + 0.65 * h3)
                + height * config.volcano_radius_per_height)
                .max(config.volcano_radius_min);

            geology.volcanoes.push(Volcano {
                center: volcano_pos,
                height,
                radius,
                plate_id,
                age: 0.0,
            });

            // Следующая дистанция: чередуем сближенные группы из 2–3 вулканов и длинные паузы затишья!
            let is_dormant_gap = plume.eruption_counter % 3 == 0;
            let spacing_mult = if is_dormant_gap {
                1.85 + h1 * 1.90 // Длинный пролив / затишье плюма
            } else {
                0.42 + h2 * 0.55 // Сближенный кластер вулканов (перекрывающиеся массивы)
            };
            plume.next_eruption_dist =
                (config.hotspot_spacing_rad * spacing_mult).max(0.018);
        }
    }
}