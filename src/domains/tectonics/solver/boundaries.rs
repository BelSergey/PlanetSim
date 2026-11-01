// src/domains/tectonics/solver/boundaries.rs:

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

/// Точная единичная нормаль к биссектрисе Вороного между плитами `c1` и `c2` в точке сферы `x` (от `c1` к `c2`)
#[inline]
fn spherical_voronoi_normal(x: Vec3, c1: Vec3, c2: Vec3) -> Vec3 {
    let t1 = (c1 - x * x.dot(c1)).normalize_or_zero();
    let t2 = (c2 - x * x.dot(c2)).normalize_or_zero();
    (t2 - t1).normalize_or_zero()
}

/// Смещает точку `dir` по геодезической вдоль касательного вектора `tangent` на угол `arc_rad`
#[inline]
fn geodesic_step(dir: Vec3, tangent: Vec3, arc_rad: f32) -> Vec3 {
    if tangent == Vec3::ZERO || arc_rad.abs() < 1e-6 {
        return dir;
    }
    (dir * arc_rad.cos() + tangent * arc_rad.sin()).normalize_or_zero()
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
    let plates = &geology.plates;
    let plumes = &geology.plumes;
    if plates.len() < 3 {
        return;
    }

    let ref_speed = (config.angular_vel_max * 1.35).max(0.5);
    let edge_thresh = config.edge_threshold.clamp(0.18, 0.55);
    let warp_scale = config.warp_scale as f32;
    let warp_strength = config.warp_strength;

    let plume_bounds: Vec<(Vec3, f32, f32, f32)> = plumes
        .iter()
        .map(|pl| {
            let max_r = (pl.radius * 2.0).clamp(0.01, 0.6);
            (pl.position, max_r, max_r.cos(), pl.strength)
        })
        .collect();

    let mut is_spreading_ridge = vec![false; num_cells];

    {
        let plate_ids = &mut geology.plate_ids;
        let thickness = &mut geology.crust_thickness;
        let sediment = &mut geology.sediment_thickness;
        let stress = &mut geology.tectonic_stress;
        let orogeny = &mut geology.orogeny_root;
        let shifts = &mut geology.subcell_shift;

        let conv_scratch = &mut scratch.boundary_convergence;
        let shear_scratch = &mut scratch.boundary_shear;
        let nbr_p_scratch = &mut scratch.neighbor_plate;
        let norm_scratch = &mut scratch.boundary_normal;
        let nbr_cont_scratch = &mut scratch.neighbor_continental;
        let subduct_scratch = &mut scratch.is_subducting_slab;

        plate_ids
            .par_iter_mut()
            .zip(thickness.par_iter_mut())
            .zip(sediment.par_iter_mut())
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
                                                        (((p_out, thick_out), sed_out), stress_out),
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

                    let (p1_idx, p2_idx, p3_idx, bd2, bd3) =
                        find_closest_plates_and_boundary_dists(warped_dir, plates);
                    *p_out = p1_idx;

                    let p1 = &plates[p1_idx];
                    let p2 = &plates[p2_idx];
                    let p3 = &plates[p3_idx];

                    let normal2 = spherical_voronoi_normal(warped_dir, p1.center_dir, p2.center_dir);
                    let normal3 = spherical_voronoi_normal(warped_dir, p1.center_dir, p3.center_dir);

                    let eps = 0.0004_f32;
                    let w2 = 1.0 / (bd2 * bd2 + eps);
                    let w3 = 1.0 / (bd3 * bd3 + eps);
                    let w_sum = w2 + w3;
                    let weight2 = w2 / w_sum;
                    let weight3 = w3 / w_sum;

                    // Континентальность своей плиты берём в неискажённом `dir` (инвариантно к вращению),
                    // а континентальность соседних плит — в зеркально отражённой за границу Вороного точке!
                    let c1_raw = craton_continental_factor(dir, p1);
                    let probe_p2 = geodesic_step(dir, normal2, bd2 + 0.04);
                    let probe_p3 = geodesic_step(dir, normal3, bd3 + 0.04);
                    let c2_raw = craton_continental_factor(probe_p2, p2);
                    let c3_raw = craton_continental_factor(probe_p3, p3);

                    let bridge_2 = if c1_raw > 0.25 && c2_raw > 0.25 {
                        (1.0 - smoothstep(bd2 / 0.18)) * (c1_raw * c2_raw).sqrt()
                    } else {
                        0.0
                    };
                    let bridge_3 = if c1_raw > 0.25 && c3_raw > 0.25 {
                        (1.0 - smoothstep(bd3 / 0.18)) * (c1_raw * c3_raw).sqrt()
                    } else {
                        0.0
                    };
                    let c_eff = c1_raw.max(bridge_2).max(bridge_3).clamp(0.0, 1.0);

                    let v1 = p1.omega.cross(dir);
                    let v2 = p2.omega.cross(dir);
                    let rel_v2 = v1 - v2;
                    let raw_conv2 = rel_v2.dot(normal2);
                    let conv2 = (raw_conv2 / ref_speed).clamp(-1.2, 1.2);

                    let v3 = p3.omega.cross(dir);
                    let rel_v3 = v1 - v3;
                    let raw_conv3 = rel_v3.dot(normal3);
                    let conv3 = (raw_conv3 / ref_speed).clamp(-1.2, 1.2);

                    let f2 = (1.0 - bd2 / edge_thresh).clamp(0.0, 1.0);
                    let f3 = (1.0 - bd3 / edge_thresh).clamp(0.0, 1.0);

                    // Плавучесть учитывает и континентальность, и базовый изостатический подпор плиты
                    let buoy1 = c1_raw + p1.base_elevation_offset * 8.0;
                    let buoy2 = c2_raw + p2.base_elevation_offset * 8.0;
                    let buoy3 = c3_raw + p3.base_elevation_offset * 8.0;
                    let override_2 = (buoy1 > buoy2)
                        || ((buoy1 - buoy2).abs() < 1e-5 && p1.seed_offset > p2.seed_offset);
                    let override_3 = (buoy1 > buoy3)
                        || ((buoy1 - buoy3).abs() < 1e-5 && p1.seed_offset > p3.seed_offset);

                    if f2 > 0.0 {
                        *conv_s_out = raw_conv2 * f2;
                        *shear_s_out = (rel_v2 - normal2 * raw_conv2).length() * f2;
                        *nbr_p_s_out = p2_idx;
                        *norm_s_out = normal2;
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

                    // 3D-шум вдоль разломов с использованием `config.boundary_noise_base`
                    let mp = warped_dir * (config.boundary_noise_scale as f32 * 0.55);
                    let n_a = (mp.x * 1.3 + mp.y * 1.8).sin() * (mp.z * 1.6 - mp.x * 1.2).cos();
                    let n_b = (mp.y * 2.9 - mp.z * 2.4).sin() * (mp.x * 2.7 + mp.z * 2.1).cos();
                    let noise_base = config.boundary_noise_base.clamp(0.20, 0.90);
                    let ridge_mod = (noise_base + (1.0 - noise_base) * (0.60 * n_a + 0.40 * n_b))
                        .clamp(0.08, 1.30);

                    let (dh2, orog2, is_ridge2) = boundary_elevation_delta(
                        c1_raw,
                        c2_raw,
                        f2,
                        conv2,
                        ridge_mod,
                        override_2,
                        edge_thresh,
                        config,
                    );
                    let (dh3, orog3, is_ridge3) = boundary_elevation_delta(
                        c1_raw,
                        c3_raw,
                        f3,
                        conv3,
                        ridge_mod,
                        override_3,
                        edge_thresh,
                        config,
                    );

                    let boundary_elev_delta = dh2 * weight2 + dh3 * weight3;
                    let target_orogeny = (orog2 * weight2 + orog3 * weight3).clamp(0.0, 1.0);
                    let local_stress =
                        (conv2 * smoothstep(f2) * weight2 + conv3 * smoothstep(f3) * weight3)
                            .clamp(-1.0, 1.0);

                    if c_eff < 0.32 && ((is_ridge2 && bd2 < 0.055) || (is_ridge3 && bd3 < 0.055)) {
                        *ridge_flag_out = true;
                    }

                    let aged_orog = (*orog_out * (-0.35 * dt).exp()).max(target_orogeny);
                    *orog_out = aged_orog;

                    let mut plume_elev = 0.0_f32;
                    for &(p_pos, max_r, cos_max_r, p_str) in &plume_bounds {
                        let dot = dir.dot(p_pos).clamp(-1.0, 1.0);
                        if dot > cos_max_r {
                            let d = dot.acos();
                            let pf = smoothstep(1.0 - d / max_r);
                            plume_elev += pf * p_str * 0.08;
                        }
                    }

                    // Утолщение коры происходит только от положительного орогенеза/сжатия,
                    // а утончение (`rift_thinning_rate`) — только в зонах растяжения (`local_stress < 0`), но НЕ в желобах субдукции!
                    let base_thick = config.ocean_crust_thickness_km
                        + c_eff * (config.cont_crust_thickness_km - config.ocean_crust_thickness_km);
                    let compressional_thickening = boundary_elev_delta.max(0.0)
                        * (config.convergence_thickening_rate * 1.45)
                        + aged_orog * (config.convergence_thickening_rate * 0.22);
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

                    // Осадконакопление на шельфе с учётом `sediment_deposition_efficiency`
                    let sed_decay = (-0.08 * dt).exp();
                    let dep_rate = 0.22 * config.sediment_deposition_efficiency;
                    if c_eff > 0.18 && c_eff < 0.68 {
                        *sed_out = (*sed_out * sed_decay + dep_rate * dt).clamp(0.15, 3.8);
                    } else {
                        *sed_out = (*sed_out * sed_decay).clamp(0.05, 3.8);
                    }
                },
            );
    }

    update_crust_age_and_density(topology, cache, geology, &is_spreading_ridge, config);
}

fn update_crust_age_and_density(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    is_spreading_ridge: &[bool],
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let mut dist_steps = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::new();

    for (i, &is_ridge) in is_spreading_ridge.iter().enumerate() {
        if is_ridge {
            dist_steps[i] = 0;
            queue.push_back(i);
        }
    }

    while let Some(curr) = queue.pop_front() {
        let d = dist_steps[curr];
        let p_curr = geology.plate_ids[curr];
        for &nbr in &topology.cells[curr].neighbors {
            let p_nbr = geology.plate_ids[nbr];
            // Запрещаем волне молодого возраста перешагивать через чужие границы плит (кроме самой оси СОХ) и зоны субдукции
            let same_or_ridge =
                (p_curr == p_nbr) || is_spreading_ridge[curr] || is_spreading_ridge[nbr];
            let not_subduction_trench = geology.tectonic_stress[nbr] <= 0.08;

            if same_or_ridge
                && not_subduction_trench
                && geology.subcell_shift[nbr][1] < 0.65
                && dist_steps[nbr] > d + 1
            {
                dist_steps[nbr] = d + 1;
                queue.push_back(nbr);
            }
        }
    }

    let step_rad = cache.avg_cell_rad;
    let mature_rad = 0.75_f32;

    let age = &mut geology.crust_age;
    let density = &mut geology.crust_density;
    let shifts = &geology.subcell_shift;

    age.par_iter_mut()
        .zip(density.par_iter_mut())
        .zip(shifts.par_iter())
        .enumerate()
        .for_each(|(i, ((age_out, dens_out), shift))| {
            let c_eff = shift[1];
            let ocean_age = if dist_steps[i] != u32::MAX {
                let rad_from_ridge = (dist_steps[i] as f32) * step_rad;
                ((rad_from_ridge / mature_rad) * config.oceanic_mature_age)
                    .clamp(0.0, config.oceanic_mature_age * 1.15)
            } else {
                config.oceanic_mature_age * 0.90
            };

            let cont_age = config.oceanic_mature_age * (1.10 + c_eff * 1.50);
            let cont_blend = smoothstep((c_eff - 0.32) / 0.24);
            *age_out = ocean_age * (1.0 - cont_blend) + cont_age * cont_blend;

            let cooling = (ocean_age / config.oceanic_mature_age.max(0.1))
                .clamp(0.0, 1.0)
                .sqrt();
            let ocean_dens = config.young_ocean_density
                + cooling * (config.old_ocean_density - config.young_ocean_density);
            *dens_out = ocean_dens * (1.0 - cont_blend) + config.cont_crust_density * cont_blend;
        });
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn boundary_elevation_delta(
    c1: f32,
    c2: f32,
    f: f32,
    convergence: f32,
    ridge_mod: f32,
    is_overriding_plate: bool,
    edge_thresh: f32,
    config: &TectonicsConfig,
) -> (f32, f32, bool) {
    if f <= 1e-4 || convergence.abs() < 0.03 {
        return (0.0, 0.0, false);
    }

    let w_c1 = smoothstep((c1 - 0.38) / 0.22);
    let w_c2 = smoothstep((c2 - 0.38) / 0.22);

    let thick_scale = (config.convergence_thickening_rate / 26.0).clamp(0.25, 3.0);
    let rift_scale = (config.rift_thinning_rate / 19.0).clamp(0.25, 3.0);
    let trench_amp = config.trench_depth_amp.clamp(0.05, 0.60);
    // В координатах `f` шов границы равен 1.0, а глубина плиты — 0.0.
    // Поэтому отступ `arc_offset_f` откладывается вниз от 1.0 (`arc_outer_f = 1.0 - arc_offset_f`).
    let arc_offset_f = (config.arc_inland_offset_rad / edge_thresh.max(0.1)).clamp(0.05, 0.45);

    if convergence > 0.0 {
        let conv_mag = convergence.clamp(0.0, 1.2);

        // 1. Коллизия двух континентов (Гималаи)
        let cc_bell = smoothstep(f) * f.powf(0.8);
        let dh_cc = cc_bell * 0.44 * thick_scale * ridge_mod * conv_mag;
        let orog_cc = (cc_bell * 0.95 * conv_mag).clamp(0.0, 1.0);

        // 2. Субдукция Океан под Континент (Анды + жёлоб)
        let andes_outer_f = (1.0 - arc_offset_f * 0.85).clamp(0.42, 0.94);
        let andes_inner_f = 0.08_f32;
        let andes_span = (andes_outer_f - andes_inner_f).max(0.25);
        let andes_zone = ((f - andes_inner_f) / andes_span).clamp(0.0, 1.0);
        let andes_bell = (andes_zone * PI).sin().powi(2);
        let seam_trench = smoothstep((f - 0.82) / 0.18) * (-trench_amp * 0.72) * conv_mag;
        let dh_co = andes_bell * 0.40 * thick_scale * ridge_mod * conv_mag + seam_trench;
        let orog_co = (andes_bell * 0.90 * conv_mag).clamp(0.0, 1.0);

        let trench_bell = smoothstep((f - 0.30) / 0.70);
        let dh_oc = -trench_bell * trench_amp * conv_mag;

        // 3. Субдукция Океан–Океан (Островная дуга + глубокий жёлоб)
        let (dh_oo, orog_oo) = if is_overriding_plate {
            let arc_outer_f = (1.0 - arc_offset_f).clamp(0.38, 0.92);
            let arc_inner_f = 0.10_f32;
            let arc_span = (arc_outer_f - arc_inner_f).max(0.25);
            let arc_zone = ((f - arc_inner_f) / arc_span).clamp(0.0, 1.0);
            let arc_bell = (arc_zone * PI).sin().powi(2);
            let island_peaks = ((ridge_mod - 0.58).max(0.0) * 1.85).powf(1.35);
            let arc_uplift = arc_bell * (0.18 + 0.38 * island_peaks) * thick_scale * conv_mag;
            let trench_edge = smoothstep((f - 0.80) / 0.20) * (-trench_amp) * conv_mag;
            (
                arc_uplift + trench_edge,
                (arc_bell * 0.75 * island_peaks * conv_mag).clamp(0.0, 1.0),
            )
        } else {
            let outer_swell = ((f / 0.50).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.04 * conv_mag;
            let trench_drop = smoothstep((f - 0.35) / 0.65) * (-trench_amp) * conv_mag;
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
        let div_mag = (-convergence).clamp(0.0, 1.2);

        // 1. Континентальный рифт
        let graben = -smoothstep((f - 0.45) / 0.55) * 0.22 * rift_scale;
        let horsts = ((f / 0.75).clamp(0.0, 1.0) * PI).sin().powi(2) * 0.14 * ridge_mod;
        let dh_cc = (graben + horsts) * div_mag;
        let orog_cc = (horsts * 2.5 * div_mag).clamp(0.0, 0.55);

        // 2. Срединно-океанический хребет (СОХ)
        let ridge_flanks = (f * PI * 0.72).sin().powi(2) * 0.16 * (0.65 + 0.35 * ridge_mod);
        let axial_rift_valley = -smoothstep((f - 0.72) / 0.28) * 0.18 * rift_scale;
        let dh_oo = (ridge_flanks + axial_rift_valley) * div_mag;
        let orog_oo = (0.20 * f * div_mag).clamp(0.0, 0.35);

        // 3. Пассивный рифт океан-континент
        let dh_mixed = -smoothstep(f) * 0.06 * rift_scale * div_mag;

        let both_cont = w_c1 * w_c2;
        let both_ocean = (1.0 - w_c1) * (1.0 - w_c2);
        let mixed = (1.0 - both_cont - both_ocean).max(0.0);

        let dh = both_cont * dh_cc + both_ocean * dh_oo + mixed * dh_mixed;
        let orog = both_cont * orog_cc + both_ocean * orog_oo;

        (dh, orog, both_ocean > 0.45)
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
    if num_plates == 0 {
        return;
    }

    let warp_scale = config.warp_scale as f32;
    let warp_strength = config.warp_strength;

    for (p_idx, plume) in geology.plumes.iter_mut().enumerate() {
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

        let warped_volc = fast_domain_warp(volcano_pos, warp_scale, warp_strength);
        let (volcano_plate_id, _, _, _, _) =
            find_closest_plates_and_boundary_dists(warped_volc, &geology.plates);

        let plate_vel = geology.plates[volcano_plate_id]
            .omega
            .cross(volcano_pos)
            .length();
        plume.accum_drift_rad += plate_vel * dt;

        if plume.accum_drift_rad >= plume.next_eruption_dist {
            plume.accum_drift_rad = 0.0;
            plume.eruption_counter = next_cnt;

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

    let _ = topology;
}