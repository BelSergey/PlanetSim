// src/domains/tectonics/solver/lithology.rs

use super::{
    TectonicsGeometryCache, boundaries::fast_domain_warp, isostasy::step_elevation_diffusion,
    ocean::compute_oceans_and_continentality, smoothstep,
};
use crate::{
    domains::{
        generator::GenerationLogger,
        tectonics::{
            tectonics_config::TectonicsConfig,
            tectonics_data::{GeologyLayer, RockType},
        },
    },
    world::topology::WorldTopology,
};
use bevy::math::Vec3;
use noise::NoiseFn;
use rayon::prelude::*;

pub fn finalize_tectonics(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
    logger: Option<&GenerationLogger>,
) {
    if let Some(log) = logger {
        log.start_step("Тектоника: орогенез, поиск океанов и континентальность...");
    }

    {
        let volcanoes = &geology.volcanoes;
        let orogeny = &geology.orogeny_root;
        let stress = &geology.tectonic_stress;
        let elevations = &mut geology.elevations;
        let volcano_mask = &mut geology.volcano_mask;
        let profile_pow = config.volcano_profile_pow.max(1);

        elevations
            .par_iter_mut()
            .zip(volcano_mask.par_iter_mut())
            .enumerate()
            .for_each(|(i, (elev_out, mask_out))| {
                let dir = topology.cells[i].center;
                // Искажённое направление границ плит (совпадает с boundaries.rs и classify_physical_lithology)
                let boundary_warped =
                    fast_domain_warp(dir, config.warp_scale as f32, config.warp_strength);
                let bw_dvec = boundary_warped.as_dvec3();

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
                let w_dvec = warped_dir.as_dvec3();

                // Мультифрактальный горный шум вычисляется в пространстве `bw_dvec`, согласованном с `orogeny[i]`
                let m_scale = config.boundary_noise_scale;
                let raw_m1 = noise.get([
                    bw_dvec.x * m_scale,
                    bw_dvec.y * m_scale,
                    bw_dvec.z * m_scale,
                ]) as f32;
                let raw_m2 = noise.get([
                    bw_dvec.x * (m_scale * 2.1) + 19.3,
                    bw_dvec.y * (m_scale * 2.1) - 27.8,
                    bw_dvec.z * (m_scale * 2.1) + 11.4,
                ]) as f32;
                let ridge_crest = (1.0 - raw_m1.abs()) * (0.65 + 0.35 * (1.0 - raw_m2.abs()));
                let mtn_mod = (ridge_crest.powf(1.25) - 0.25) * config.boundary_noise_amp;
                let orog_addition = orogeny[i] * mtn_mod;

                // На спрединге в океане добавляем только мелкую шероховатость, строго не поднимая хребет выше -0.06
                let rift_addition = if stress[i] < -0.05 && *elev_out < -0.10 {
                    (-stress[i]).clamp(0.0, 1.0) * (raw_m2 * 0.035)
                } else {
                    0.0
                };

                // Щитовые вулканические острова и стратовулканы с учётом `volcano_profile_pow` и `volcano_noise_base`
                let v_scale = config.volcano_noise_scale;
                let raw_v1 =
                    noise.get([w_dvec.x * v_scale, w_dvec.y * v_scale, w_dvec.z * v_scale]) as f32;
                let raw_v2 = noise.get([
                    w_dvec.x * (v_scale * 1.9) + 17.3,
                    w_dvec.y * (v_scale * 1.9) - 31.1,
                    w_dvec.z * (v_scale * 1.9) + 9.4,
                ]) as f32;

                let v_warp = (dir
                    + Vec3::new(wx, wy, wz) * 0.028
                    + Vec3::new(raw_v1, raw_v2, -raw_v1) * 0.014)
                    .normalize_or_zero();

                let base_h = *elev_out;
                let mut surface_h = base_h;
                let mut is_volc = false;

                for v in volcanoes {
                    let dist = v_warp.dot(v.center).clamp(-1.0, 1.0).acos();
                    let eff_radius = (v.radius * (0.80 + 0.40 * raw_v1 + 0.22 * raw_v2)).max(0.018);

                    if dist < eff_radius {
                        let r_norm = (dist / eff_radius).clamp(0.0, 1.0);
                        let age_flatten = (v.age * 1.8).clamp(0.0, 0.65);
                        let dome = (1.0 - r_norm * r_norm).max(0.0);
                        let shoulder = smoothstep(1.0 - r_norm);
                        let cone_shape = dome * (0.45 + 0.55 * age_flatten)
                            + shoulder.powi(profile_pow) * (0.55 - 0.55 * age_flatten);

                        let ridges = config.volcano_noise_base
                            + (1.0 - config.volcano_noise_base) * (1.0 - raw_v2.abs())
                            + config.volcano_noise_mult * raw_v1;

                        let volcano_target_h = if base_h < 0.0 {
                            let island_pedestal = base_h * (1.0 - shoulder * 0.88);
                            island_pedestal + v.height * 0.52 * cone_shape * ridges
                        } else {
                            base_h + v.height * 0.65 * cone_shape * ridges
                        };

                        if volcano_target_h > surface_h {
                            surface_h = volcano_target_h;
                        }
                        if r_norm < 0.48 && volcano_target_h > 0.02 {
                            is_volc = true;
                        }
                    }
                }
                *mask_out = is_volc;

                // Оба масштаба детального шума берутся в согласованном искаженном пространстве `w_dvec`!
                let d_scale = config.detail_noise_scale;
                let detail_macro =
                    noise.get([w_dvec.x * d_scale, w_dvec.y * d_scale, w_dvec.z * d_scale]) as f32;
                let detail_micro = noise.get([
                    w_dvec.x * (d_scale * 2.7) + 41.2,
                    w_dvec.y * (d_scale * 2.7) - 19.8,
                    w_dvec.z * (d_scale * 2.7) + 63.5,
                ]) as f32;
                let raw_detail = detail_macro * 0.70 + detail_micro * 0.30;

                // На спокойных безгорных равнинах суши смещаем шум слегка вверх (меньше бессточных ям)
                let is_flat_lowland = base_h > 0.0 && !is_volc && orogeny[i] < 0.25;
                let detail_combined = if is_flat_lowland {
                    (raw_detail.max(-0.25) + 0.05) * config.detail_noise_amp * 0.28
                } else {
                    raw_detail * config.detail_noise_amp * 0.42
                };

                let h = surface_h + orog_addition + rift_addition + detail_combined;
                *elev_out = h.max(config.elevation_min_clamp);
            });
    }

    step_elevation_diffusion(
        topology,
        &mut geology.elevations,
        config.diffusion_strength * 0.40,
        config.slope_erosion_factor,
        config.diffusion_max_strength,
    );

    compute_oceans_and_continentality(topology, cache, geology, config);

    if let Some(log) = logger {
        log.start_step("Тектоника: классификация пород и расчёт рельефности...");
    }

    classify_physical_lithology(topology, cache, geology, noise, config);
    compute_terrain_ruggedness(topology, cache, geology, noise);
}

fn classify_physical_lithology(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let elevations = &geology.elevations;
    let thickness = &geology.crust_thickness;
    let density = &geology.crust_density;
    let age = &geology.crust_age;
    let sediment = &geology.sediment_thickness;
    let stress = &geology.tectonic_stress;
    let orogeny = &geology.orogeny_root;
    let is_ocean = &geology.is_ocean;
    let volcano_mask = &geology.volcano_mask;
    let lithology = &mut geology.lithology;

    let ref_rad = cache.avg_cell_rad;
    let warp_scale = config.warp_scale as f32;
    let warp_strength = config.warp_strength;

    lithology
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, lith_out)| {
            let elev = elevations[i];
            let thick = thickness[i];
            let dens = density[i];
            let c_age = age[i];
            let sed = sediment[i];
            let str_val = stress[i];
            let orog = orogeny[i];
            let ocean_cell = is_ocean[i];
            let lat_abs = topology.cells[i].lat_lon.x.abs();

            // Используем те же искаженные координаты `warped_dir`, что и для границ/орогенеза
            let warped_dir = fast_domain_warp(topology.cells[i].center, warp_scale, warp_strength);
            let d = warped_dir.as_dvec3();
            let l_noise = noise.get([d.x * 6.5 + 31.7, d.y * 6.5 - 19.3, d.z * 6.5 + 47.1]) as f32;

            if volcano_mask[i] {
                *lith_out = if dens >= 2830.0 {
                    RockType::FloodBasalt
                } else if elev > config.cont_base_height + 0.14 {
                    RockType::Andesite
                } else {
                    RockType::FloodBasalt
                };
                return;
            }

            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            let mut max_slope = 0.0_f32;
            for e in start..end {
                let nbr = cache.edge_neighbors[e];
                let grad = ((elev - elevations[nbr]).abs() / cache.edge_dists_rad[e]) * ref_rad;
                if grad > max_slope {
                    max_slope = grad;
                }
            }

            if ocean_cell {
                if elev > -0.095 + l_noise * 0.015 && lat_abs < 0.62 + l_noise * 0.05 {
                    *lith_out = RockType::Limestone;
                } else if orog > 0.35 && str_val > 0.05 {
                    *lith_out = RockType::Andesite;
                } else if dens < 2810.0 && sed > 0.6 {
                    *lith_out = if sed > 1.4 {
                        RockType::ShaleClay
                    } else {
                        RockType::Sandstone
                    };
                } else if c_age < 0.28 || str_val < -0.08 {
                    *lith_out = RockType::OceanicBasalt;
                } else if elev < config.ocean_base_height - 0.02 || sed > 0.22 {
                    *lith_out = RockType::PelagicSediment;
                } else {
                    *lith_out = RockType::OceanicBasalt;
                }
                return;
            }

            if str_val < -0.08 && thick < 26.0 {
                *lith_out = if l_noise > -0.20 {
                    RockType::FloodBasalt
                } else {
                    RockType::Sandstone
                };
                return;
            }

            if orog > 0.42 || thick > 42.0 {
                if str_val > 0.08 && thick < 52.0 && l_noise > -0.15 {
                    *lith_out = RockType::Andesite;
                } else if orog > 0.65 || (thick > 48.0 && l_noise > 0.0) {
                    *lith_out = RockType::Granite;
                } else {
                    *lith_out = RockType::GneissSchist;
                }
                return;
            }

            if sed > 0.85 || (elev < config.cont_base_height * 0.88 && max_slope < 0.016) {
                if elev < config.cont_base_height * 0.75
                    && max_slope < 0.015
                    && lat_abs < 0.58 + l_noise * 0.05
                    && l_noise > 0.14
                {
                    *lith_out = RockType::Limestone;
                } else if max_slope < 0.007 && (elev < config.cont_base_height * 0.50 || sed > 1.8)
                {
                    *lith_out = RockType::ShaleClay;
                } else {
                    *lith_out = RockType::Sandstone;
                }
                return;
            }

            if l_noise > 0.42 {
                *lith_out = RockType::Granite;
            } else if l_noise > -0.05 {
                *lith_out = RockType::GneissSchist;
            } else {
                *lith_out = RockType::Sandstone;
            }
        });
}

pub(crate) fn compute_terrain_ruggedness(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
) {
    let num_cells = topology.cells.len();
    geology.terrain_ruggedness.resize(num_cells, 0.0);

    let elevations = &geology.elevations;
    let lithology = &geology.lithology;
    let volcano_mask = &geology.volcano_mask;
    let is_ocean = &geology.is_ocean;
    let terrain_rug = &mut geology.terrain_ruggedness;

    terrain_rug
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, rug_out)| {
            let cell = &topology.cells[i];
            let elev = elevations[i];
            let rock = lithology[i];
            let ocean_cell = is_ocean[i];

            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            let n_count = (end - start).max(1) as f32;

            let mut max_angular_grad = 0.0_f32;
            let mut mean_angular_grad = 0.0_f32;

            for e in start..end {
                let n_id = cache.edge_neighbors[e];
                let step_rad = cache.edge_dists_rad[e];
                let grad = (elev - elevations[n_id]).abs() / step_rad;
                if grad > max_angular_grad {
                    max_angular_grad = grad;
                }
                mean_angular_grad += grad;
            }
            mean_angular_grad /= n_count;

            let slope_score =
                (max_angular_grad * 0.095 + mean_angular_grad * 0.065).clamp(0.0, 0.78);

            let d = cell.center.as_dvec3();
            let crag_a =
                (noise.get([d.x * 26.0 + 13.4, d.y * 26.0 - 27.8, d.z * 26.0 + 41.2]) as f32).abs();
            let crag_b =
                (noise.get([d.x * 52.0 - 31.9, d.y * 52.0 + 18.5, d.z * 52.0 - 22.7]) as f32).abs();
            let micro_crags = (crag_a * 0.65 + crag_b * 0.35).clamp(0.0, 1.0);

            let rock_retention = match rock {
                RockType::Granite => 1.00,
                RockType::Andesite => 0.95,
                RockType::GneissSchist => 0.88,
                RockType::FloodBasalt => 0.80,
                RockType::Limestone => 0.68,
                RockType::Sandstone => 0.50,
                RockType::OceanicBasalt => 0.45,
                RockType::ShaleClay | RockType::PelagicSediment => 0.15,
            };

            let elev_prominence = if !ocean_cell {
                ((elev - 0.08).max(0.0) * 1.45).clamp(0.0, 0.45)
            } else {
                0.0
            };
            let volcano_roughness = if volcano_mask[i] && !ocean_cell {
                0.18
            } else {
                0.0
            };

            let micro_score =
                micro_crags * rock_retention * (0.14 + slope_score * 0.55 + elev_prominence * 0.60);

            *rug_out = (slope_score + elev_prominence * 0.55 + volcano_roughness + micro_score)
                .clamp(0.0, 1.0);
        });
}
