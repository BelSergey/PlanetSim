// src/domains/tectonics/solver/boundaries.rs

use super::{TectonicsGeometryCache, TectonicsScratch, smoothstep};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{GeologyLayer, Volcano},
    },
    world::topology::WorldTopology,
};
use bevy::math::Vec3;
use rayon::prelude::*;
use std::collections::VecDeque;
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

/// Данные узкополосного детектора границ для одной клетки
#[derive(Clone, Copy)]
struct BoundaryBandCell {
    p2_idx: usize,
    p3_idx: usize,
    bd2: f32,
    bd3: f32,
    seam_cell_2: usize,
    seam_cell_3: usize,
    normal2: Vec3,
    normal3: Vec3,
}

/// Узкополосный (Narrow-Band) поиск расстояний до клеточных границ плит, соседних плит и гладких нормалей.
/// Работает только в полосе `edge_thresh` (~6-9 гексов от разлома), не пересчитывая всю сферу.
fn compute_narrow_band_boundaries(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    plate_ids: &[usize],
    plates_len: usize,
    edge_thresh: f32,
) -> Vec<BoundaryBandCell> {
    let num_cells = topology.cells.len();
    let max_steps = ((edge_thresh / cache.avg_cell_rad.max(1e-4)).ceil() as u32).clamp(3, 14);

    let mut band = vec![
        BoundaryBandCell {
            p2_idx: usize::MAX,
            p3_idx: usize::MAX,
            bd2: edge_thresh + 0.25,
            bd3: edge_thresh + 0.25,
            seam_cell_2: usize::MAX,
            seam_cell_3: usize::MAX,
            normal2: Vec3::ZERO,
            normal3: Vec3::ZERO,
        };
        num_cells
    ];

    let mut steps_2 = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::with_capacity(num_cells / 4);

    // 1. Инициализируем клетки непосредственно на шве разлома (расстояние 0.5 * avg_cell_rad до линии шва)
    let half_step = cache.avg_cell_rad * 0.5;
    for i in 0..num_cells {
        let my_p = plate_ids[i];
        let start = cache.edge_offsets[i];
        let end = cache.edge_offsets[i + 1];

        let mut primary_other = usize::MAX;
        let mut secondary_other = usize::MAX;
        let mut primary_seam_nbr = usize::MAX;
        let mut secondary_seam_nbr = usize::MAX;
        let mut n2_acc = Vec3::ZERO;
        let mut n3_acc = Vec3::ZERO;

        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            let np = plate_ids[nbr];
            if np != my_p && np < plates_len {
                let tang = cache.edge_tangent_dirs[e];
                if primary_other == usize::MAX || primary_other == np {
                    primary_other = np;
                    primary_seam_nbr = nbr;
                    n2_acc += tang;
                } else if secondary_other == usize::MAX || secondary_other == np {
                    secondary_other = np;
                    secondary_seam_nbr = nbr;
                    n3_acc += tang;
                }
            }
        }

        if primary_other != usize::MAX {
            steps_2[i] = 0;
            band[i].p2_idx = primary_other;
            band[i].bd2 = half_step;
            band[i].seam_cell_2 = primary_seam_nbr;
            band[i].normal2 = n2_acc.normalize_or_zero();

            if secondary_other != usize::MAX {
                band[i].p3_idx = secondary_other;
                band[i].bd3 = half_step;
                band[i].seam_cell_3 = secondary_seam_nbr;
                band[i].normal3 = n3_acc.normalize_or_zero();
            }
            queue.push_back(i);
        }
    }

    // 2. Распространяем узкую полосу разлома вглубь каждой плиты до `max_steps`
    while let Some(u) = queue.pop_front() {
        let cur_step = steps_2[u];
        if cur_step >= max_steps {
            continue;
        }
        let my_p = plate_ids[u];
        let u_info = band[u];

        let start = cache.edge_offsets[u];
        let end = cache.edge_offsets[u + 1];
        for e in start..end {
            let v = cache.edge_neighbors[e];
            if plate_ids[v] != my_p {
                continue;
            }
            let next_dist = u_info.bd2 + cache.edge_dists_rad[e];
            if next_dist > edge_thresh {
                continue;
            }

            // Переносим нормаль в касательную плоскость клетки `v`
            let r_v = topology.cells[v].center;
            let transported_n2 =
                (u_info.normal2 - r_v * r_v.dot(u_info.normal2)).normalize_or_zero();

            if next_dist + 1e-5 < band[v].bd2 {
                // Если в `v` уже была записана другая соседняя плита, сдвигаем её во второй слот `p3` (тройной стык)
                if band[v].p2_idx != usize::MAX && band[v].p2_idx != u_info.p2_idx {
                    band[v].p3_idx = band[v].p2_idx;
                    band[v].bd3 = band[v].bd2;
                    band[v].seam_cell_3 = band[v].seam_cell_2;
                    band[v].normal3 = band[v].normal2;
                }
                band[v].p2_idx = u_info.p2_idx;
                band[v].bd2 = next_dist;
                band[v].seam_cell_2 = u_info.seam_cell_2;
                band[v].normal2 = transported_n2;

                if steps_2[v] > cur_step + 1 {
                    steps_2[v] = cur_step + 1;
                    queue.push_back(v);
                }
            } else if u_info.p2_idx != band[v].p2_idx && next_dist < band[v].bd3 {
                band[v].p3_idx = u_info.p2_idx;
                band[v].bd3 = next_dist;
                band[v].seam_cell_3 = u_info.seam_cell_2;
                band[v].normal3 = transported_n2;
            }
        }
    }

    // 3. Два прохода сглаживания касательных нормалей внутри полосы разлома,
    //    чтобы векторы `normal2` были гладкими без ступенек гексагональной сетки
    let prev_normals: Vec<Vec3> = band.iter().map(|b| b.normal2).collect();
    band.par_iter_mut().enumerate().for_each(|(i, b)| {
        if b.p2_idx == usize::MAX {
            return;
        }
        let my_p = plate_ids[i];
        let r_i = topology.cells[i].center;
        let mut sum_n = prev_normals[i] * 2.0;
        let start = cache.edge_offsets[i];
        let end = cache.edge_offsets[i + 1];
        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            if plate_ids[nbr] == my_p && prev_normals[nbr] != Vec3::ZERO {
                sum_n += prev_normals[nbr];
            }
        }
        b.normal2 = (sum_n - r_i * r_i.dot(sum_n)).normalize_or_zero();
    });

    band
}

pub fn project_plates_and_boundaries(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_cells = topology.cells.len();
    let num_plates = geology.plates.len();
    if num_plates < 3 || num_cells == 0 {
        return;
    }
    geology.ensure_aux_buffers(num_cells);

    let ref_speed = (config.angular_vel_max * 1.35).max(0.5);
    let edge_thresh = config.edge_threshold.clamp(0.18, 0.55);
    let warp_scale = config.warp_scale as f32;
    let warp_strength = config.warp_strength;

    // 1. Строим узкую полосу разломов по текущим клеточным `plate_ids`
    let band = compute_narrow_band_boundaries(
        topology,
        cache,
        &geology.plate_ids,
        num_plates,
        edge_thresh,
    );

    // 2. Проверяем, не врезался ли плавучий континент нижней плиты в жёлоб (инверсия полярности субдукции)
    if dt > 0.0 {
        let mut flip_votes = vec![0i32; num_plates * num_plates];
        for (i, b) in band.iter().enumerate() {
            if b.p2_idx >= num_plates || b.bd2 > cache.avg_cell_rad * 1.5 {
                continue;
            }
            let p1 = geology.plate_ids[i];
            let p2 = b.p2_idx;
            if geology.is_plate_subducting_under(p1, p2) == Some(true) {
                let c1 = geology.continental_factor[i];
                let c2 = geology
                    .continental_factor
                    .get(b.seam_cell_2)
                    .copied()
                    .unwrap_or(0.0);
                // Если на ныряющей плите к жёлобу подошёл материк, а на верхней — океан, полярность переворачивается
                if c1 > 0.58 && c2 < 0.28 {
                    flip_votes[p1 * num_plates + p2] += 1;
                }
            }
        }
        for p1 in 0..num_plates {
            for p2 in 0..num_plates {
                if flip_votes[p1 * num_plates + p2] >= 6 {
                    geology.set_subduction_polarity(p2, p1);
                }
            }
        }
    }

    let plume_bounds: Vec<(Vec3, f32, f32, f32)> = geology
        .plumes
        .iter()
        .map(|pl| {
            let max_r = (pl.radius * 2.2).clamp(0.01, 0.6);
            (pl.position, max_r, max_r.cos(), pl.strength)
        })
        .collect();

    let mut is_spreading_ridge = vec![false; num_cells];

    {
        let plates = &geology.plates;
        let plate_ids = &geology.plate_ids;
        let cont_factors = &geology.continental_factor;
        let sutures = &geology.suture_weakness;
        let subduction_matrix = &geology.subduction_polarity;

        let thickness = &mut geology.crust_thickness;
        let sediment = &mut geology.sediment_thickness;
        let sed_supply = &mut geology.sediment_supply;
        let stress = &mut geology.tectonic_stress;
        let orogeny = &mut geology.orogeny_root;
        let shifts = &mut geology.subcell_shift;

        let conv_scratch = &mut scratch.boundary_convergence;
        let shear_scratch = &mut scratch.boundary_shear;
        let nbr_p_scratch = &mut scratch.neighbor_plate;
        let norm_scratch = &mut scratch.boundary_normal;
        let nbr_cont_scratch = &mut scratch.neighbor_continental;
        let subduct_scratch = &mut scratch.is_subducting_slab;

        let orog_decay = (-config.orogen_memory_decay.max(0.02) * dt).exp();

        thickness
            .par_iter_mut()
            .zip(sediment.par_iter_mut())
            .zip(sed_supply.par_iter_mut())
            .zip(stress.par_iter_mut())
            .zip(orogeny.par_iter_mut())
            .zip(shifts.par_iter_mut())
            .zip(conv_scratch.par_iter_mut())
            .zip(shear_scratch.par_iter_mut())
            .zip(nbr_p_scratch.par_iter_mut())
            .zip(norm_scratch.par_iter_mut())
            .zip(nbr_cont_scratch.par_iter_mut())
            .zip(subduct_scratch.par_iter_mut())
            .zip(is_spreading_ridge.par_iter_mut())
            .enumerate()
            .for_each(
                |(
                    i,
                    (
                        (
                            (
                                (
                                    (
                                        (
                                            (
                                                (
                                                    (
                                                        (
                                                            ((thick_out, sed_out), supply_out),
                                                            stress_out,
                                                        ),
                                                        orog_out,
                                                    ),
                                                    shift_out,
                                                ),
                                                conv_s_out,
                                            ),
                                            shear_s_out,
                                        ),
                                        nbr_p_s_out,
                                    ),
                                    norm_s_out,
                                ),
                                nbr_cont_s_out,
                            ),
                            subduct_s_out,
                        ),
                        ridge_flag_out,
                    ),
                )| {
                    let dir = topology.cells[i].center;
                    let warped_dir = fast_domain_warp(dir, warp_scale, warp_strength);
                    let p1_idx = plate_ids[i].min(num_plates - 1);
                    let p1 = &plates[p1_idx];
                    let b = &band[i];

                    let c1_raw = cont_factors[i];
                    let suture_w = sutures[i];

                    // Долговолновой шум изгибает саму дистанцию до разлома `bd`, убирая эффект «линейки»
                    let dist_warp = ((warped_dir.x * 5.2 + warped_dir.y * 4.7).sin()
                        * (warped_dir.z * 5.1 - 1.8).cos())
                        * 0.022;

                    let has_p2 = b.p2_idx < num_plates && b.bd2 < edge_thresh;
                    let has_p3 = b.p3_idx < num_plates && b.bd3 < edge_thresh;

                    let bd2 = (b.bd2 + dist_warp).max(0.0);
                    let bd3 = (b.bd3 + dist_warp).max(0.0);

                    let f2 = if has_p2 {
                        (1.0 - bd2 / edge_thresh).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let f3 = if has_p3 {
                        (1.0 - bd3 / edge_thresh).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };

                    let c2_raw = if has_p2 && b.seam_cell_2 < num_cells {
                        cont_factors[b.seam_cell_2]
                    } else {
                        c1_raw
                    };
                    let c3_raw = if has_p3 && b.seam_cell_3 < num_cells {
                        cont_factors[b.seam_cell_3]
                    } else {
                        c1_raw
                    };

                    // Сшивание континентальных мостов и сдвиговых окраин через границу,
                    // чтобы на трансформных и коллизионных стыках не возникало резкой ступеньки!
                    let bridge_2 = if has_p2 {
                        if c1_raw > 0.25 && c2_raw > 0.25 {
                            (1.0 - smoothstep(bd2 / 0.18)) * (c1_raw * c2_raw).sqrt()
                        } else {
                            // На контакте континент-океан плавно скругляем край шельфа к границе
                            let seam_blend = smoothstep((f2 - 0.65) / 0.35) * 0.38;
                            c1_raw * (1.0 - seam_blend) + c2_raw * seam_blend
                        }
                    } else {
                        c1_raw
                    };

                    let c_eff = if has_p2 && c1_raw > 0.25 && c2_raw > 0.25 {
                        c1_raw.max(bridge_2).clamp(0.0, 1.0)
                    } else if has_p2 {
                        bridge_2.clamp(0.0, 1.0)
                    } else {
                        c1_raw
                    };

                    let v1 = p1.omega.cross(dir);

                    // Граница 2
                    let (raw_conv2, raw_shear2, conv2, shear2, override_2) = if has_p2 {
                        let p2 = &plates[b.p2_idx];
                        let v2 = p2.omega.cross(dir);
                        let rel_v2 = v1 - v2;
                        let rc = rel_v2.dot(b.normal2);
                        let rs = (rel_v2 - b.normal2 * rc).length();
                        let sub_pol = subduction_matrix[p1_idx * num_plates + b.p2_idx];
                        let is_over = if sub_pol != 0 {
                            sub_pol < 0
                        } else {
                            c1_raw >= c2_raw
                        };
                        (
                            rc,
                            rs,
                            (rc / ref_speed).clamp(-1.2, 1.2),
                            (rs / ref_speed).clamp(0.0, 1.2),
                            is_over,
                        )
                    } else {
                        (0.0, 0.0, 0.0, 0.0, true)
                    };

                    // Граница 3 (для тройных стыков)
                    let (conv3, shear3, override_3) = if has_p3 {
                        let p3 = &plates[b.p3_idx];
                        let v3 = p3.omega.cross(dir);
                        let rel_v3 = v1 - v3;
                        let rc = rel_v3.dot(b.normal3);
                        let rs = (rel_v3 - b.normal3 * rc).length();
                        let sub_pol = subduction_matrix[p1_idx * num_plates + b.p3_idx];
                        let is_over = if sub_pol != 0 {
                            sub_pol < 0
                        } else {
                            c1_raw >= c3_raw
                        };
                        (
                            (rc / ref_speed).clamp(-1.2, 1.2),
                            (rs / ref_speed).clamp(0.0, 1.2),
                            is_over,
                        )
                    } else {
                        (0.0, 0.0, true)
                    };

                    if f2 > 0.0 {
                        *conv_s_out = raw_conv2 * f2;
                        *shear_s_out = raw_shear2 * f2;
                        *nbr_p_s_out = b.p2_idx;
                        *norm_s_out = b.normal2;
                        *nbr_cont_s_out = c2_raw;
                        *subduct_s_out = !override_2;
                    } else {
                        *conv_s_out = 0.0;
                        *shear_s_out = 0.0;
                        *nbr_p_s_out = usize::MAX;
                        *norm_s_out = Vec3::ZERO;
                        *nbr_cont_s_out = 0.0;
                        *subduct_s_out = false;
                    }

                    // 3D-шум вдоль разломов + высокочастотная гармоника для трансформных сдвигов
                    let mp = warped_dir * (config.boundary_noise_scale as f32 * 0.55);
                    let n_a = (mp.x * 1.3 + mp.y * 1.8).sin() * (mp.z * 1.6 - mp.x * 1.2).cos();
                    let n_b = (mp.y * 2.9 - mp.z * 2.4).sin() * (mp.x * 2.7 + mp.z * 2.1).cos();
                    let transform_stripes =
                        (mp.x * 6.5 - mp.y * 5.8 + mp.z * 6.1).sin();
                    let noise_base = config.boundary_noise_base.clamp(0.20, 0.90);
                    let ridge_mod = (noise_base + (1.0 - noise_base) * (0.60 * n_a + 0.40 * n_b))
                        .clamp(0.08, 1.30);

                    let (dh2, orog2, is_ridge2) = boundary_elevation_delta(
                        c1_raw,
                        c2_raw,
                        f2,
                        conv2,
                        shear2,
                        ridge_mod,
                        transform_stripes,
                        override_2,
                        edge_thresh,
                        config,
                    );

                    let (boundary_elev_delta, target_orogeny, local_stress) = if has_p3 {
                        let (dh3, orog3, _) = boundary_elevation_delta(
                            c1_raw,
                            c3_raw,
                            f3,
                            conv3,
                            shear3,
                            ridge_mod,
                            transform_stripes,
                            override_3,
                            edge_thresh,
                            config,
                        );
                        let eps = 0.0004_f32;
                        let w2 = 1.0 / (bd2 * bd2 + eps);
                        let w3 = 1.0 / (bd3 * bd3 + eps);
                        let w_sum = w2 + w3;
                        let weight2 = w2 / w_sum;
                        let weight3 = w3 / w_sum;
                        (
                            dh2 * weight2 + dh3 * weight3,
                            (orog2 * weight2 + orog3 * weight3).clamp(0.0, 1.0),
                            (conv2 * smoothstep(f2) * weight2 + conv3 * smoothstep(f3) * weight3)
                                .clamp(-1.0, 1.0),
                        )
                    } else {
                        (
                            dh2,
                            orog2.clamp(0.0, 1.0),
                            (conv2 * smoothstep(f2)).clamp(-1.0, 1.0),
                        )
                    };

                    if c_eff < 0.32 && is_ridge2 && bd2 < 0.060 {
                        *ridge_flag_out = true;
                    }

                    // Долгая память орогенеза (`orogen_memory_decay`): древние горы затухают медленно,
                    // а реликтовые швы (`suture_w`) сохраняют пологий фундамент старых хребтов
                    let suture_relic_orog = suture_w * 0.24 * c_eff;
                    let aged_orog = (*orog_out * orog_decay)
                        .max(target_orogeny)
                        .max(suture_relic_orog);
                    *orog_out = aged_orog;

                    let mut plume_elev = 0.0_f32;
                    for &(p_pos, max_r, cos_max_r, p_str) in &plume_bounds {
                        let dot = dir.dot(p_pos).clamp(-1.0, 1.0);
                        if dot > cos_max_r {
                            let d = dot.acos();
                            let pf = smoothstep(1.0 - d / max_r);
                            plume_elev += pf * p_str * 0.085;
                        }
                    }

                    let base_thick = config.ocean_crust_thickness_km
                        + c_eff
                            * (config.cont_crust_thickness_km - config.ocean_crust_thickness_km);
                    let compressional_thickening = boundary_elev_delta.max(0.0)
                        * (config.convergence_thickening_rate * 1.45)
                        + aged_orog * (config.convergence_thickening_rate * 0.28);
                    let extensional_thinning = if local_stress < 0.0 {
                        boundary_elev_delta.min(0.0)
                            * (config.rift_thinning_rate * 1.85)
                            * (-local_stress).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    *thick_out = (base_thick + compressional_thickening + extensional_thinning)
                        .clamp(3.0, config.max_crust_thickness_km);
                    *stress_out = local_stress;
                    shift_out[0] = boundary_elev_delta + plume_elev;
                    shift_out[1] = c_eff;
                    shift_out[2] = c2_raw;

                    // Чистый источник обломочного материала (`sediment_supply`) от разрушения гор и активных окраин
                    let erosion_yield = (aged_orog * 0.65 + boundary_elev_delta.max(0.0) * 1.2)
                        * config.sediment_deposition_efficiency;
                    *supply_out = erosion_yield.clamp(0.0, 2.5);

                    // Предгорные и пассивные прогибы плавно накапливают сносимые осадки без жёсткой ступеньки
                    let foreland_factor = if c_eff > 0.15 && aged_orog < 0.38 {
                        (1.0 - aged_orog / 0.38) * c_eff
                    } else {
                        0.0
                    };
                    let sed_decay = (-0.05 * dt).exp();
                    *sed_out = (*sed_out * sed_decay
                        + (0.14 * foreland_factor + erosion_yield * 0.18) * dt)
                        .clamp(0.05, 4.2);
                },
            );
    }

    if dt == 0.0
        || geology.sim_step.is_multiple_of(4)
        || geology.sim_step as usize + 1 >= config.simulation_steps
    {
        update_crust_age_and_density_fast(topology, cache, geology, &is_spreading_ridge, config);
    } else {
        update_crust_age_fast(geology, &is_spreading_ridge, dt, config);
    }
}

pub fn update_crust_age_fast(
    geology: &mut GeologyLayer,
    is_spreading_ridge: &[bool],
    dt: f32,
    config: &TectonicsConfig,
) {
    let mature = config.oceanic_mature_age.max(1e-4);
    let young = config.young_ocean_density;
    let old = config.old_ocean_density;
    let cont = config.cont_crust_density;

    let age = &mut geology.crust_age;
    let density = &mut geology.crust_density;
    let shifts = &geology.subcell_shift;

    age.par_iter_mut()
        .zip(density.par_iter_mut())
        .zip(shifts.par_iter())
        .zip(is_spreading_ridge.par_iter())
        .for_each(|(((age_out, dens_out), shift), &is_ridge)| {
            let c_eff = shift[1];
            let cont_blend = smoothstep((c_eff - 0.32) / 0.24);
            let cont_age = mature * (1.10 + c_eff * 1.50);

            let ocean_age = if is_ridge {
                0.0
            } else {
                ((*age_out).min(mature) + dt * 0.4).min(mature)
            };

            *age_out = ocean_age * (1.0 - cont_blend) + cont_age * cont_blend;

            let age_norm = (ocean_age / mature).clamp(0.0, 1.0).sqrt();
            let ocean_dens = young + age_norm * (old - young);
            *dens_out = ocean_dens * (1.0 - cont_blend) + cont * cont_blend;
        });
}

pub fn update_crust_age_and_density_fast(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    is_spreading_ridge: &[bool],
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    if num_cells == 0 || is_spreading_ridge.len() != num_cells {
        return;
    }

    let mut dist = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::with_capacity(num_cells / 4);
    for (i, &is_ridge) in is_spreading_ridge.iter().enumerate().take(num_cells) {
        if is_ridge {
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(u) = queue.pop_front() {
        let next_d = dist[u].saturating_add(1);
        for &v in &topology.cells[u].neighbors {
            if dist[v] > next_d {
                dist[v] = next_d;
                queue.push_back(v);
            }
        }
    }

    let mature = config.oceanic_mature_age.max(1e-4);
    let age_per_rad = mature / 0.40_f32;
    let young = config.young_ocean_density;
    let old = config.old_ocean_density;
    let cont = config.cont_crust_density;

    geology
        .crust_age
        .par_iter_mut()
        .zip(geology.crust_density.par_iter_mut())
        .zip(geology.subcell_shift.par_iter())
        .enumerate()
        .for_each(|(i, ((age_out, dens_out), shift))| {
            let new_age = if dist[i] == u32::MAX {
                ((*age_out).min(mature) + 0.045).min(mature)
            } else {
                let dist_rad = dist[i] as f32 * cache.avg_cell_rad;
                (dist_rad * age_per_rad).min(mature)
            };

            let c_eff = shift[1];
            let cont_blend = smoothstep((c_eff - 0.32) / 0.24);
            let cont_age = mature * (1.10 + c_eff * 1.50);
            *age_out = new_age * (1.0 - cont_blend) + cont_age * cont_blend;

            let age_norm = (new_age / mature).clamp(0.0, 1.0).sqrt();
            let ocean_dens = young + age_norm * (old - young);
            *dens_out = ocean_dens * (1.0 - cont_blend) + cont * cont_blend;
        });
}

/// Вычисляет профиль высот и орогенеза для всех типов границ, включая:
/// - Конвергентные (Коллизия, Андская субдукция, Островная дуга + задуговой бассейн),
/// - Дивергентные (Континентальный рифт, СОХ с трансформными разломами, Пассивная окраина),
/// - Трансформные / сдвиговые границы (`shear`), устраняющие необработанные ступеньки на стыках плит.
#[allow(clippy::too_many_arguments)]
#[inline]
fn boundary_elevation_delta(
    c1: f32,
    c2: f32,
    f: f32,
    convergence: f32,
    shear: f32,
    ridge_mod: f32,
    transform_stripes: f32,
    is_overriding_plate: bool,
    edge_thresh: f32,
    config: &TectonicsConfig,
) -> (f32, f32, bool) {
    if f <= 1e-4 || (convergence.abs() < 0.015 && shear < 0.015) {
        return (0.0, 0.0, false);
    }

    let w_c1 = smoothstep((c1 - 0.38) / 0.22);
    let w_c2 = smoothstep((c2 - 0.38) / 0.22);

    let thick_scale = (config.convergence_thickening_rate / 26.0).clamp(0.25, 3.0);
    let rift_scale = (config.rift_thinning_rate / 19.0).clamp(0.25, 3.0);
    let trench_amp = config.trench_depth_amp.clamp(0.05, 0.60);
    let arc_offset_f = (config.arc_inland_offset_rad / edge_thresh.max(0.1)).clamp(0.05, 0.45);

    // --- 1. Поперечная сдвиговая компонента (Transform / Strike-Slip Faults) ---
    let shear_mag = shear.clamp(0.0, 1.2);
    let strike_slip_dominance =
        (shear_mag / (shear_mag + convergence.abs() * 1.5 + 1e-4)).clamp(0.0, 1.0);
    let mut dh_transform = 0.0_f32;
    let mut orog_transform = 0.0_f32;

    if shear_mag > 0.015 {
        let fault_groove =
            -smoothstep((f - 0.78) / 0.22) * 0.085 * (0.7 + 0.3 * transform_stripes);
        let transpression_shoulders =
            ((f / 0.82).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.11 * ridge_mod;
        let dh_tr_cont = (fault_groove + transpression_shoulders) * shear_mag;

        let fracture_trough = -smoothstep((f - 0.60) / 0.40)
            * config.transform_fault_amp
            * (1.1 + 0.6 * transform_stripes)
            * shear_mag;

        let margin_step_smooth = (c2 - c1) * 0.12 * smoothstep((f - 0.45) / 0.55);

        let both_cont = w_c1 * w_c2;
        let both_ocean = (1.0 - w_c1) * (1.0 - w_c2);
        let mixed = (1.0 - both_cont - both_ocean).max(0.0);

        dh_transform = both_cont * dh_tr_cont
            + both_ocean * fracture_trough
            + mixed * (margin_step_smooth + dh_tr_cont * 0.45);
        orog_transform = (both_cont + mixed * 0.5)
            * (transpression_shoulders * 1.8 * shear_mag).clamp(0.0, 0.42);
    }

    if convergence.abs() < 0.02 {
        return (dh_transform, orog_transform, false);
    }

    // --- 2. Конвергентная компонента (Сближение: Коллизия и Субдукция) ---
    let (dh_norm, orog_norm, is_ridge) = if convergence > 0.0 {
        let conv_mag = convergence.clamp(0.0, 1.2);
        // Корневая шкала (`sqrt`) гарантирует явный рельеф даже при умеренной скорости сближения плит
        let eff_conv = conv_mag.sqrt().clamp(0.35, 1.15);

        // 2.1. Коллизия двух континентов (Гималаи / Тибет)
        let cc_bell = smoothstep(f) * f.powf(0.75);
        let plateau_asym = if is_overriding_plate { 1.12 } else { 0.88 };
        let dh_cc = cc_bell * 0.44 * plateau_asym * thick_scale * ridge_mod * conv_mag;
        let orog_cc = (cc_bell * 0.95 * conv_mag).clamp(0.0, 1.0);

        // 2.2. Субдукция Океан под Континент (Анды + глубокий жёлоб)
        let andes_outer_f = (1.0 - arc_offset_f * 0.85).clamp(0.42, 0.94);
        let andes_inner_f = 0.12_f32;
        let andes_span = (andes_outer_f - andes_inner_f).max(0.25);
        let andes_zone = ((f - andes_inner_f) / andes_span).clamp(0.0, 1.0);
        let andes_bell = (andes_zone * PI).sin().powi(2);
        let seam_trench = smoothstep((f - 0.82) / 0.18) * (-trench_amp * 0.85) * eff_conv;
        let dh_co = andes_bell * 0.42 * thick_scale * ridge_mod * conv_mag + seam_trench;
        let orog_co = (andes_bell * 0.92 * conv_mag).clamp(0.0, 1.0);

        let outer_swell_oc = ((f / 0.48).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.045 * eff_conv;
        let trench_bell = smoothstep((f - 0.28) / 0.72);
        let dh_oc = outer_swell_oc - trench_bell * (trench_amp * 1.25) * eff_conv;

        // 2.3. Субдукция Океан–Океан: ОДИН высокий, явный хребет на верхней плите + глубокий жёлоб на нижней
        let (dh_oo, orog_oo) = if is_overriding_plate {
            let arc_outer_f = (1.0 - arc_offset_f * 0.65).clamp(0.55, 0.92);
            let arc_inner_f = 0.18_f32;
            let arc_span = (arc_outer_f - arc_inner_f).max(0.24);
            let arc_zone = ((f - arc_inner_f) / arc_span).clamp(0.0, 1.0);
            // Высокий сплошной подводно-островной хребет с острым гребнем
            let arc_bell = (arc_zone * PI).sin().powi(2);
            let island_peaks = ((ridge_mod - 0.48).max(0.0) * 1.75).powf(1.20);
            let main_ridge_uplift =
                arc_bell * (0.28 + 0.24 * ridge_mod + 0.22 * island_peaks) * thick_scale * eff_conv;
            let back_arc_trough =
                -((f / arc_inner_f).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.055 * eff_conv;
            let trench_edge = smoothstep((f - 0.84) / 0.16) * (-trench_amp * 1.15) * eff_conv;
            (
                main_ridge_uplift + back_arc_trough + trench_edge,
                (arc_bell * (0.65 + 0.35 * ridge_mod) * eff_conv).clamp(0.0, 1.0),
            )
        } else {
            let outer_swell = ((f / 0.50).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.05 * eff_conv;
            let trench_drop = smoothstep((f - 0.32) / 0.68) * (-trench_amp * 1.35) * eff_conv;
            (outer_swell + trench_drop, 0.0)
        };

        let dh = w_c1 * w_c2 * dh_cc
            + w_c1 * (1.0 - w_c2) * dh_co
            + (1.0 - w_c1) * w_c2 * dh_oc
            + (1.0 - w_c1) * (1.0 - w_c2) * dh_oo;

        let orog = w_c1 * w_c2 * orog_cc
            + w_c1 * (1.0 - w_c2) * orog_co
            + (1.0 - w_c1) * (1.0 - w_c2) * orog_oo;

        (dh, orog, false)
    } else {
        // --- 3. Дивергентная компонента (Расхождение: Рифт и СОХ — два хребта с желобом посередине) ---
        let div_mag = (-convergence).clamp(0.0, 1.2);
        let eff_div = div_mag.sqrt().clamp(0.40, 1.15);

        // 3.1. Континентальный рифт (два плеча горстов с глубоким грабеном посередине)
        let graben = -smoothstep((f - 0.55) / 0.45) * 0.24 * rift_scale;
        let horsts = ((f / 0.72).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.16 * ridge_mod;
        let dh_cc = (graben + horsts) * eff_div;
        let orog_cc = (horsts * 2.5 * eff_div).clamp(0.0, 0.55);

        // 3.2. Срединно-океанический хребет (СОХ):
        // На каждой из двух плит на `f ≈ 0.56` поднимается высокий хребет (`ridge_crest`),
        // а прямо на шве (`f -> 1.0`) проваливается осевой рифтовый жёлоб (`axial_rift_valley`),
        // что в сумме на двух плитах даёт ДВА параллельных хребта с глубоким жёлобом посередине!
        let transform_zig =
            transform_stripes * config.transform_fault_amp * (0.35 + 0.65 * shear_mag);
        let f_shifted = (f + transform_zig).clamp(0.0, 1.0);

        let flank_zone = ((f_shifted - 0.10) / 0.74).clamp(0.0, 1.0);
        let ridge_crest =
            (flank_zone * PI).sin().powi(2) * 0.27 * (0.72 + 0.28 * ridge_mod);
        let axial_rift_valley = -smoothstep((f_shifted - 0.68) / 0.32) * 0.32 * rift_scale;
        let dh_oo = (ridge_crest + axial_rift_valley) * eff_div;
        let orog_oo = (ridge_crest * 1.6 * eff_div).clamp(0.0, 0.55);

        // 3.3. Пассивная рифтовая окраина океан-континент
        let dh_mixed = -smoothstep(f) * 0.08 * rift_scale * eff_div;

        let both_cont = w_c1 * w_c2;
        let both_ocean = (1.0 - w_c1) * (1.0 - w_c2);
        let mixed = (1.0 - both_cont - both_ocean).max(0.0);

        let dh = both_cont * dh_cc + both_ocean * dh_oo + mixed * dh_mixed;
        let orog = both_cont * orog_cc + both_ocean * orog_oo;

        (dh, orog, both_ocean > 0.45)
    };

    let total_dh = dh_norm + dh_transform * (0.45 + 0.55 * strike_slip_dominance);
    let total_orog = orog_norm.max(orog_transform);

    (total_dh, total_orog, is_ridge)
}

pub fn apply_boundary_and_plume_processes(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    project_plates_and_boundaries(topology, cache, geology, scratch, config, dt);
    spawn_hotspot_volcanoes_on_plates(topology, geology, config, dt);
}

pub(crate) fn spawn_hotspot_volcanoes_on_plates(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_plates = geology.plates.len();
    let num_cells = topology.cells.len();
    if num_plates == 0 || num_cells == 0 {
        return;
    }

    for (p_idx, plume) in geology.plumes.iter_mut().enumerate() {
        let is_first_lip_burst = plume.eruption_counter == 0;
        let next_cnt = plume.eruption_counter.wrapping_add(1);
        let h1 = ((next_cnt as f32 * 12.9898 + p_idx as f32 * 78.233).sin() * 43758.547)
            .fract()
            .abs();
        let h2 = ((next_cnt as f32 * 93.989 + p_idx as f32 * 19.17).sin() * 28461.33)
            .fract()
            .abs();
        let h3 = ((next_cnt as f32 * 41.33 + p_idx as f32 * 53.71).cos() * 19371.7)
            .fract()
            .abs();

        let jitter_tangent = Vec3::new(h1 - 0.5, h2 - 0.5, h3 - 0.5)
            .cross(plume.position)
            .normalize_or_zero();
        let jitter_angle = (h1 - 0.5) * config.hotspot_spacing_rad * 0.42;
        let volcano_pos = (plume.position + jitter_tangent * jitter_angle).normalize_or_zero();

        // Определяем плиту над плюмом по ближайшей клетке сетки
        let mut best_cell = 0;
        let mut best_dot = -2.0_f32;
        for (cid, cell) in topology.cells.iter().enumerate() {
            let d = cell.center.dot(volcano_pos);
            if d > best_dot {
                best_dot = d;
                best_cell = cid;
            }
        }
        let volcano_plate_id = geology.plate_ids[best_cell].min(num_plates - 1);

        let plate_vel = geology.plates[volcano_plate_id]
            .omega
            .cross(volcano_pos)
            .length();
        plume.accum_drift_rad += plate_vel * dt;

        if is_first_lip_burst || plume.accum_drift_rad >= plume.next_eruption_dist {
            plume.accum_drift_rad = 0.0;
            plume.eruption_counter = next_cnt;

            let base_height = (config.hotspot_height_min
                + h2.powf(1.15) * (config.hotspot_height_max - config.hotspot_height_min))
                * (0.70 + 0.45 * (plume.strength / config.hotspot_height_max.max(0.1)));
            let base_radius = (config.hotspot_radius_base * (0.95 + 0.75 * h3)
                + base_height * config.volcano_radius_per_height * 1.35)
                .max(config.volcano_radius_min * 1.5);

            // При первом прорыве головы плюма (`is_first_lip_burst`) формируется широкая трапповая провинция (LIP)
            let (height, radius) = if is_first_lip_burst {
                (base_height * 0.82, (base_radius * 1.65).min(0.16))
            } else {
                (base_height, base_radius)
            };

            geology.volcanoes.push(Volcano {
                center: volcano_pos,
                height,
                radius,
                plate_id: volcano_plate_id,
                age: 0.0,
            });

            let is_dormant_gap = plume.eruption_counter.is_multiple_of(3);
            let spacing_mult = if is_dormant_gap {
                1.85 + h1 * 1.95
            } else {
                0.42 + h2 * 0.52
            };
            plume.next_eruption_dist = (config.hotspot_spacing_rad * spacing_mult).max(0.020);
        }
    }
}