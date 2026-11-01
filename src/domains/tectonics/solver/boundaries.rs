// src/domains/tectonics/solver/boundaries.rs

use super::{
    init::{craton_continental_factor, find_closest_plates_and_boundary_dists},
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
            let warped = fast_domain_warp(dir, 1.6, 0.42);
            let (p1_idx, p2_idx, _, bd2, _) =
                find_closest_plates_and_boundary_dists(warped, plates);

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

    let ref_speed = (config.angular_vel_max * 1.35).max(0.5);
    let edge_thresh = config.edge_threshold.clamp(0.24, 0.48);

    let plate_ids = &mut geology.plate_ids;
    let thickness = &mut geology.crust_thickness;
    let density = &mut geology.crust_density;
    let age = &mut geology.crust_age;
    let sediment = &mut geology.sediment_thickness;
    let stress = &mut geology.tectonic_stress;
    let orogeny = &mut geology.orogeny_root;
    let subcell_shift = &mut geology.subcell_shift; 

    plate_ids
        .par_iter_mut()
        .zip(thickness.par_iter_mut())
        .zip(density.par_iter_mut())
        .zip(age.par_iter_mut())
        .zip(sediment.par_iter_mut())
        .zip(stress.par_iter_mut())
        .zip(orogeny.par_iter_mut())
        .zip(subcell_shift.par_iter_mut()) 
        .enumerate()
        .for_each(
            |(
                i,
                (
                    ((((((p_out, thick_out), dens_out), age_out), sed_out), stress_out),
                    orog_out), subcell_out,
                ),
            )| {
                let dir = topology.cells[i].center;
                let warped_dir =
                    fast_domain_warp(dir, config.warp_scale as f32, config.warp_strength);

                // 1. Истинные угловые расстояния до границ с плитами 2 и 3
                let (p1_idx, p2_idx, p3_idx, bd2, bd3) =
                    find_closest_plates_and_boundary_dists(warped_dir, plates);
                *p_out = p1_idx;

                let p1 = &plates[p1_idx];
                let p2 = &plates[p2_idx];
                let p3 = &plates[p3_idx];

                let eps = 0.0004_f32;
                let w2 = 1.0 / (bd2 * bd2 + eps);
                let w3 = 1.0 / (bd3 * bd3 + eps);
                let w_sum = w2 + w3;
                let weight2 = w2 / w_sum;
                let weight3 = w3 / w_sum;

                // 2. Континентальность клетки считается по её собственной плите p1
                let c1 = craton_continental_factor(warped_dir, p1);
                let c2 = craton_continental_factor(warped_dir, p2);
                let c3 = craton_continental_factor(warped_dir, p3);

                // 3. Относительные скорости схождения (> 0) и расхождения (< 0)
                let v1 = p1.omega.cross(dir);
                let normal2 = (p2.center_dir - p1.center_dir).normalize_or_zero();
                let v2 = p2.omega.cross(dir);
                let conv2 = ((v1 - v2).dot(normal2) / ref_speed).clamp(-1.2, 1.2);

                let normal3 = (p3.center_dir - p1.center_dir).normalize_or_zero();
                let v3 = p3.omega.cross(dir);
                let conv3 = ((v1 - v3).dot(normal3) / ref_speed).clamp(-1.2, 1.2);

                // 3D-шум вдоль разломов (создает отдельные горные пики, перевалы и острова в дугах)
                let mp = warped_dir * (config.boundary_noise_scale as f32 * 0.55);
                let n_a = (mp.x * 1.3 + mp.y * 1.8).sin() * (mp.z * 1.6 - mp.x * 1.2).cos();
                let n_b = (mp.y * 2.9 - mp.z * 2.4).sin() * (mp.x * 2.7 + mp.z * 2.1).cos();
                let ridge_mod = (0.52 + 0.42 * n_a + 0.28 * n_b).clamp(0.08, 1.25);

                // 4. Профили границ (строго синхронизированы с полем напряжений!)
                let f2 = (1.0 - bd2 / edge_thresh).clamp(0.0, 1.0);
                let f3 = (1.0 - bd3 / edge_thresh).clamp(0.0, 1.0);

                let override_2 = p1_idx < p2_idx;
                let override_3 = p1_idx < p3_idx;

                let (dh2, orog2, age_mod2) =
                    boundary_elevation_delta(c1, c2, f2, conv2, ridge_mod, override_2);
                let (dh3, orog3, age_mod3) =
                    boundary_elevation_delta(c1, c3, f3, conv3, ridge_mod, override_3);

                // В `subcell_shift[i][0]` сохраняем чистую тектоническую дельту высоты от границ,
                // чтобы в `isostasy.rs` не путать склон кратона с островной дугой!
                let boundary_elev_delta = dh2 * weight2 + dh3 * weight3;
                let target_orogeny = (orog2 * weight2 + orog3 * weight3).clamp(0.0, 1.0);

                let local_stress =
                    (conv2 * smoothstep(f2) * weight2 + conv3 * smoothstep(f3) * weight3)
                        .clamp(-1.0, 1.0);

                // 5. Возраст коры: 0 на срединно-океанических хребтах, растет к абиссали
                let spread_reset = (age_mod2 * weight2 + age_mod3 * weight3).clamp(0.0, 1.0);
                let dist_from_boundary = bd2 * weight2 + bd3 * weight3;
                let ocean_age = if spread_reset < 0.96 {
                    ((dist_from_boundary / 0.55) * config.oceanic_mature_age)
                        .clamp(0.02, config.oceanic_mature_age)
                } else {
                    (0.45 + dist_from_boundary * 1.4).clamp(0.35, config.oceanic_mature_age)
                };

                *age_out = if c1 > 0.45 {
                    0.8 + c1 * 2.2
                } else {
                    ocean_age
                };

                // 6. Плотность и толщина коры (строго монотонные от c1 + реальных гор)
                let cooling = (ocean_age / config.oceanic_mature_age).clamp(0.0, 1.0).sqrt();
                let ocean_dens = config.young_ocean_density
                    + cooling * (config.old_ocean_density - config.young_ocean_density);

                *dens_out = ocean_dens - c1 * (ocean_dens - config.cont_crust_density);

                let base_thick = config.ocean_crust_thickness_km
                    + c1 * (config.cont_crust_thickness_km - config.ocean_crust_thickness_km);

                // Орогенез синхронизирован с текущим напряжением границы (никаких призрачных стен!)
                *orog_out = target_orogeny;

                // Подъем коры над плюмами
                let mut plume_elev = 0.0_f32;
                for plume in plumes {
                    let d = dir.dot(plume.position).clamp(-1.0, 1.0).acos();
                    if d < plume.radius * 2.0 {
                        let pf = smoothstep(1.0 - d / (plume.radius * 2.0));
                        plume_elev += pf * plume.strength * 0.08;
                    }
                }

                *thick_out = (base_thick + boundary_elev_delta * 45.0)
                    .clamp(3.0, config.max_crust_thickness_km);
                *stress_out = local_stress;
                subcell_out[0] = boundary_elev_delta + plume_elev;
                subcell_out[1] = c1;

                if c1 > 0.15 && c1 < 0.72 {
                    *sed_out = (*sed_out + 0.12 * dt).clamp(0.35, 4.0);
                }
            },
        );
}

/// Вычисляет чистую тектоническую поправку высоты `dh` (в единицах высоты рельефа),
/// орогенез `[0.0 .. 1.0]` и сброс возраста коры.
/// Гарантирует непрерывность на самом шве плит (`f = 1.0`) и плавный переход по континентальности `c1, c2`.
#[inline]
fn boundary_elevation_delta(
    c1: f32,
    c2: f32,
    f: f32,
    convergence: f32,
    ridge_mod: f32,
    is_overriding_plate: bool,
) -> (f32, f32, f32) {
    if f <= 1e-4 || convergence.abs() < 0.03 {
        return (0.0, 0.0, 1.0);
    }

    // Мягкие веса континентальности (без резкого булевого скачка на c1 = 0.42!)
    let w_c1 = smoothstep((c1 - 0.25) / 0.45);
    let w_c2 = smoothstep((c2 - 0.25) / 0.45);

    if convergence > 0.0 {
        // =====================================================================
        // СХОЖДЕНИЕ ПЛИТ (КОЛЛИЗИЯ, АНДСКАЯ СУБДУКЦИЯ, ЖЕЛОБА И ОСТРОВНЫЕ ДУГИ)
        // =====================================================================
        let conv_mag = convergence.clamp(0.0, 1.2);

        // 1. Чистая коллизия двух континентов (Гималаи): единый пик на шве f = 1.0
        let cc_bell = smoothstep(f) * f.powf(0.8);
        let dh_cc = cc_bell * 0.44 * ridge_mod * conv_mag;
        let orog_cc = (cc_bell * 0.95 * conv_mag).clamp(0.0, 1.0);

        // 2. Субдукция Океан под Континент:
        // На стороне континента (w_c1 = 1, w_c2 = 0): береговой хребет Андского типа
        let andes_zone = ((f - 0.12) / 0.78).clamp(0.0, 1.0);
        let andes_bell = (andes_zone * PI).sin().powi(2);
        // На самом шве f = 1.0 край плавно опускается к желобу (-0.18), чтобы совпасть с океанической стороной
        let seam_trench = smoothstep((f - 0.78) / 0.22) * (-0.18) * conv_mag;
        let dh_co = andes_bell * 0.38 * ridge_mod * conv_mag + seam_trench;
        let orog_co = (andes_bell * 0.90 * conv_mag).clamp(0.0, 1.0);

        // На стороне океана (w_c1 = 0, w_c2 = 1): глубоководный жёлоб у подножия материка
        let trench_bell = smoothstep((f - 0.30) / 0.70);
        let dh_oc = -trench_bell * 0.28 * conv_mag;

        // 3. Столкновение Океан–Океан (w_c1 = 0, w_c2 = 0):
        // На самом шве f = 1.0 ОБЕ плиты уходят в глубокий Марианский жёлоб (-0.28 * conv_mag),
        // поэтому вдоль границы нет ступеньки в 1 гекс!
        let (dh_oo, orog_oo) = if is_overriding_plate {
            // Верхняя океаническая плита: от жёлоба (f = 1.0) поднимается крутая гряда вулканических островов (f = 0.42..0.78)!
            let arc_zone = ((f - 0.22) / 0.62).clamp(0.0, 1.0);
            let arc_bell = (arc_zone * PI).sin().powi(2);
            // Прорываются над водой (+0.05..+0.28) отдельные острова дуги там, где ridge_mod высок
            let island_peaks = ((ridge_mod - 0.32).max(0.0) * 1.45).powf(1.25);
            let arc_uplift = arc_bell * (0.26 + 0.34 * island_peaks) * conv_mag;
            let trench_edge = smoothstep((f - 0.78) / 0.22) * (-0.28) * conv_mag;
            (
                arc_uplift + trench_edge,
                (arc_bell * 0.85 * conv_mag).clamp(0.0, 1.0),
            )
        } else {
            // Нижняя (субдуцирующая) океаническая плита: плавный спуск в глубокий жёлоб к f = 1.0
            // + легкий внешний вал (outer trench swell) перед жёлобом на f ~ 0.35
            let outer_swell = ((f / 0.50).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.04 * conv_mag;
            let trench_drop = smoothstep((f - 0.35) / 0.65) * (-0.28) * conv_mag;
            (outer_swell + trench_drop, 0.0)
        };

        // Билинейно смешиваем 4 режима по мягким весам (w_c1, w_c2) — никаких обрывов на берегах!
        let dh = w_c1 * w_c2 * dh_cc
            + w_c1 * (1.0 - w_c2) * dh_co
            + (1.0 - w_c1) * w_c2 * dh_oc
            + (1.0 - w_c1) * (1.0 - w_c2) * dh_oo;

        let orog = w_c1 * w_c2 * orog_cc
            + w_c1 * (1.0 - w_c2) * orog_co
            + (1.0 - w_c1) * (1.0 - w_c2) * orog_oo;

        (dh, orog, 1.0)
    } else {
        // =====================================================================
        // РАСХОЖДЕНИЕ ПЛИТ (КОНТИНЕНТАЛЬНЫЕ РИФТЫ И СРЕДИННО-ОКЕАНИЧЕСКИЕ ХРЕБТЫ)
        // =====================================================================
        let div_mag = (-convergence).clamp(0.0, 1.2);

        // 1. Континентальный рифт (Восточная Африка / Байкал): глубокий грабен + плечи горстов
        let graben = -smoothstep((f - 0.45) / 0.55) * 0.22;
        let horsts = ((f / 0.75).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.14 * ridge_mod;
        let dh_cc = (graben + horsts) * div_mag;
        let orog_cc = (horsts * 2.5 * div_mag).clamp(0.0, 0.55);

        // 2. Срединно-океанический хребет (СОХ):
        // Подводный вал с двумя плечами (пик на f ≈ 0.72, подъем до +0.16 над дном, т.е. ОСТАЕТСЯ ПОД ВОДОЙ ~ -0.18!)
        // и четким осевым рифтовым жёлобом посередине (на f = 0.85..1.0 проваливается вниз)!
        let ridge_flanks = (f * PI * 0.72).sin().powi(2) * 0.16 * (0.65 + 0.35 * ridge_mod);
        let axial_rift_valley = -smoothstep((f - 0.72) / 0.28) * 0.18;
        let dh_oo = (ridge_flanks + axial_rift_valley) * div_mag;
        let orog_oo = (0.22 * f * div_mag).clamp(0.0, 0.40);

        // 3. Пассивный рифт океан-континент: плавное углубление шельфа
        let dh_mixed = -smoothstep(f) * 0.06 * div_mag;

        let both_cont = w_c1 * w_c2;
        let both_ocean = (1.0 - w_c1) * (1.0 - w_c2);
        let mixed = (1.0 - both_cont - both_ocean).max(0.0);

        let dh = both_cont * dh_cc + both_ocean * dh_oo + mixed * dh_mixed;
        let orog = both_cont * orog_cc + both_ocean * orog_oo;
        let age_mod = 1.0 - both_ocean * f;

        (dh, orog, age_mod)
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

            let jitter_tangent = Vec3::new(h1 - 0.5, h2 - 0.5, h3 - 0.5)
                .cross(plume.position)
                .normalize_or_zero();
            let jitter_angle = (h1 - 0.5) * config.hotspot_spacing_rad * 0.55;
            let volcano_pos = (plume.position + jitter_tangent * jitter_angle).normalize_or_zero();

            let height = (config.hotspot_height_min
                + h2.powf(1.15) * (config.hotspot_height_max - config.hotspot_height_min))
                * (0.70 + 0.45 * (plume.strength / config.hotspot_height_max.max(0.1)));
            let radius = (config.hotspot_radius_base * (0.95 + 0.75 * h3)
                + height * config.volcano_radius_per_height * 1.35)
                .max(config.volcano_radius_min * 1.5);

            geology.volcanoes.push(Volcano {
                center: volcano_pos,
                height,
                radius,
                plate_id,
                age: 0.0,
            });

            let is_dormant_gap = plume.eruption_counter % 3 == 0;
            let spacing_mult = if is_dormant_gap {
                2.10 + h1 * 2.20
            } else {
                0.45 + h2 * 0.55
            };
            plume.next_eruption_dist = (config.hotspot_spacing_rad * spacing_mult).max(0.022);
        }
    }
}