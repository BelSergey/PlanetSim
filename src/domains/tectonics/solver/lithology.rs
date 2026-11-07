// src/domains/tectonics/solver/lithology.rs

use super::{
    TectonicsGeometryCache, boundaries::fast_domain_warp, ocean::compute_oceans_and_continentality,
    smoothstep,
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
    let num_cells = topology.cells.len();
    geology.ensure_aux_buffers(num_cells);

    if let Some(log) = logger {
        log.start_step("Тектоника: орогенез, вулканизм, поиск океанов и континентальность...");
    }

    {
        let volcanoes = &geology.volcanoes;
        let orogeny = &geology.orogeny_root;
        let orog_age = &geology.orogen_age;
        let sutures = &geology.suture_weakness;
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

                // Мультифрактальный горный шум с учётом возраста орогена (пенепленизация):
                // Молодые хребты имеют острые гребни `1.0 - |noise|`, а старые (Урал/Аппалачи) —
                // сглаженные куполообразные увалы со сниженной амплитудой.
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

                let age_norm = (orog_age[i] / 3.0).clamp(0.0, 1.0);
                let sharp_ridge = (1.0 - raw_m1.abs()) * (0.65 + 0.35 * (1.0 - raw_m2.abs()));
                let weathered_hill = 0.50 + 0.35 * raw_m1 + 0.15 * raw_m2;
                let ridge_crest = sharp_ridge * (1.0 - age_norm * 0.65)
                    + weathered_hill * (age_norm * 0.65);

                let age_amp_atten = 1.0 - age_norm * 0.48;
                let mtn_mod =
                    (ridge_crest.powf(1.20) - 0.24) * config.boundary_noise_amp * age_amp_atten;
                let orog_addition = (orogeny[i] + sutures[i] * 0.12).min(1.0) * mtn_mod;

                // На спрединге в океане добавляем только мелкую шероховатость, не поднимая СОХ выше воды
                let rift_addition = if stress[i] < -0.05 && *elev_out < -0.10 {
                    (-stress[i]).clamp(0.0, 1.0) * (raw_m2 * 0.035)
                } else {
                    0.0
                };

                // Вулканические щиты, траппы (LIP) и стратовулканы
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
                        let is_lip_plateau = v.radius > config.hotspot_radius_base * 1.45;
                        let age_flatten = ((v.age * 1.8) + if is_lip_plateau { 0.45 } else { 0.0 })
                            .clamp(0.0, 0.78);
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
                        if r_norm < 0.55 && (volcano_target_h > 0.015 || is_lip_plateau) {
                            is_volc = true;
                        }
                    }
                }
                *mask_out = is_volc;

                // Детальный шум микрорельефа (без последующего глобального размытия!)
                let d_scale = config.detail_noise_scale;
                let detail_macro =
                    noise.get([w_dvec.x * d_scale, w_dvec.y * d_scale, w_dvec.z * d_scale]) as f32;
                let detail_micro = noise.get([
                    w_dvec.x * (d_scale * 2.7) + 41.2,
                    w_dvec.y * (d_scale * 2.7) - 19.8,
                    w_dvec.z * (d_scale * 2.7) + 63.5,
                ]) as f32;
                let raw_detail = detail_macro * 0.70 + detail_micro * 0.30;

                let is_flat_lowland = base_h > 0.0 && !is_volc && orogeny[i] < 0.25;
                let detail_combined = if is_flat_lowland {
                    (raw_detail.max(-0.25) + 0.05) * config.detail_noise_amp * 0.24
                } else {
                    raw_detail * config.detail_noise_amp * 0.38
                };

                let h = surface_h + orog_addition + rift_addition + detail_combined;
                *elev_out = h.max(config.elevation_min_clamp);
            });
    }

    // Вместо глобального линейного сглаживания всей планеты выполняем только защиту
    // от одиночных экстремальных пиков шума, сохраняя берега и хребты чёткими!
    suppress_extreme_noise_spikes(cache, &mut geology.elevations, config.diffusion_strength);

    compute_oceans_and_continentality(topology, cache, geology, config);

    if let Some(log) = logger {
        log.start_step("Тектоника: классификация пород и расчёт рельефности...");
    }

    classify_physical_lithology(topology, cache, geology, noise, config);
    compute_terrain_ruggedness(topology, cache, geology, noise);
}

/// Убирает только одиночные резкие выбросы (иголки) шума, где клетка выступает над всеми соседями сильнее порога,
/// не размывая пологие склоны, береговую линию и континентальные равнины.
fn suppress_extreme_noise_spikes(
    cache: &TectonicsGeometryCache,
    elevations: &mut [f32],
    diffusion_strength: f32,
) {
    if diffusion_strength <= 1e-4 {
        return;
    }
    let old = elevations.to_vec();
    let max_blend = (diffusion_strength * 0.45).clamp(0.0, 0.28);

    elevations
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, elev_out)| {
            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            if start == end {
                return;
            }
            let my_h = old[i];
            let mut sum_nbr = 0.0_f32;
            let mut max_diff = 0.0_f32;
            for e in start..end {
                let nh = old[cache.edge_neighbors[e]];
                sum_nbr += nh;
                let d = (my_h - nh).abs();
                if d > max_diff {
                    max_diff = d;
                }
            }
            let avg_nbr = sum_nbr / ((end - start) as f32);
            // Срабатывает только на крутых изломах (max_diff > 0.045)
            if max_diff > 0.045 {
                let spike_t = smoothstep((max_diff - 0.045) / 0.14);
                let blend = max_blend * spike_t;
                *elev_out = my_h * (1.0 - blend) + avg_nbr * blend;
            }
        });
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
    let sed_supply = &geology.sediment_supply;
    let stress = &geology.tectonic_stress;
    let orogeny = &geology.orogeny_root;
    let orog_age = &geology.orogen_age;
    let sutures = &geology.suture_weakness;
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
            let supply = sed_supply[i];
            let str_val = stress[i];
            let orog = orogeny[i];
            let o_age = orog_age[i];
            let suture = sutures[i];
            let ocean_cell = is_ocean[i];
            let lat_abs = topology.cells[i].lat_lon.x.abs();

            let warped_dir = fast_domain_warp(topology.cells[i].center, warp_scale, warp_strength);
            let d = warped_dir.as_dvec3();
            let l_noise = noise.get([d.x * 6.5 + 31.7, d.y * 6.5 - 19.3, d.z * 6.5 + 47.1]) as f32;
            let fine_noise =
                noise.get([d.x * 14.0 - 11.2, d.y * 14.0 + 23.8, d.z * 14.0 - 9.4]) as f32;

            // 1. Вулканические постройки, трапповые плато (LIP) и активные дуги
            if volcano_mask[i] {
                *lith_out = if dens >= 2830.0 {
                    RockType::FloodBasalt
                } else if (orog > 0.30 && str_val > 0.02)
                    || (elev > config.cont_base_height + 0.16 && fine_noise > -0.15)
                {
                    RockType::Andesite
                } else {
                    RockType::FloodBasalt
                };
                return;
            }

            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            let mut max_slope = 0.0_f32;
            let mut near_mountain_supply = supply;
            for e in start..end {
                let nbr = cache.edge_neighbors[e];
                let grad = ((elev - elevations[nbr]).abs() / cache.edge_dists_rad[e]) * ref_rad;
                if grad > max_slope {
                    max_slope = grad;
                }
                if sed_supply[nbr] > near_mountain_supply {
                    near_mountain_supply = sed_supply[nbr];
                }
            }

            // 2. Морское дно и континентальный шельф
            if ocean_cell {
                if elev > -0.095 + l_noise * 0.018 && lat_abs < 0.64 + l_noise * 0.08 {
                    *lith_out = RockType::Limestone;
                } else if orog > 0.32 && str_val > 0.04 {
                    *lith_out = RockType::Andesite;
                } else if dens < 2820.0 && (sed > 0.55 || near_mountain_supply > 0.35) {
                    // Конусы выноса на шельфе: вблизи гор — песчаники, мористее и глубже — глинистые сланцы
                    *lith_out = if near_mountain_supply > 0.50 || max_slope > 0.014 {
                        RockType::Sandstone
                    } else {
                        RockType::ShaleClay
                    };
                } else if c_age < 0.28 || str_val < -0.06 {
                    *lith_out = RockType::OceanicBasalt;
                } else if elev < config.ocean_base_height - 0.02 || sed > 0.22 {
                    *lith_out = RockType::PelagicSediment;
                } else {
                    *lith_out = RockType::OceanicBasalt;
                }
                return;
            }

            // 3. Континентальные рифты и зоны растяжения утончённой коры -> Траппы и грабеновые песчаники
            if str_val < -0.07 && thick < 27.5 {
                *lith_out = if l_noise + fine_noise * 0.35 > -0.18 {
                    RockType::FloodBasalt
                } else {
                    RockType::Sandstone
                };
                return;
            }

            // 4. Древние сутуры (швы слияния кратонов) и состаренные орогены (Урал / Аппалачи) -> Метаморфические пояса
            if suture > 0.45 && (orog > 0.18 || o_age > 1.2) {
                *lith_out = if l_noise > 0.35 {
                    RockType::Granite
                } else {
                    RockType::GneissSchist
                };
                return;
            }

            // 5. Молодые и зрелые горные пояса (коллизия и субдукция Андского типа)
            if orog > 0.38 || thick > 41.5 {
                if str_val > 0.06 && thick < 51.0 && o_age < 1.6 && l_noise > -0.18 {
                    // Активная магматическая дуга (Андезиты)
                    *lith_out = RockType::Andesite;
                } else if orog > 0.62 || (thick > 47.5 && l_noise > -0.05) {
                    // Глубоко вскрытые гранитные батолиты осевой зоны хребта
                    *lith_out = RockType::Granite;
                } else {
                    // Метаморфическое обрамление складчатого пояса
                    *lith_out = RockType::GneissSchist;
                }
                return;
            }

            // 6. Предгорные прогибы (Foreland basins), осадочные бассейны и прибрежные платформы
            if sed > 0.75
                || near_mountain_supply > 0.42
                || (elev < config.cont_base_height * 0.90 && max_slope < 0.016)
            {
                // Карбонатные платформы (известняки и карст) на тёплых мелководных равнинах
                if elev < config.cont_base_height * 0.78
                    && max_slope < 0.014
                    && lat_abs < 0.60 + l_noise * 0.08
                    && (l_noise * 0.7 + fine_noise * 0.3) > 0.12
                {
                    *lith_out = RockType::Limestone;
                } else if near_mountain_supply > 0.48 || max_slope > 0.0085 {
                    // Грубообломочный шлейф у подножия хребтов -> Песчаники
                    *lith_out = RockType::Sandstone;
                } else if max_slope < 0.0065
                    && (elev < config.cont_base_height * 0.52 || sed > 1.55)
                {
                    // Плоские озёрно-аллювиальные и древнеморские низменности -> Глины и аргиллиты
                    *lith_out = RockType::ShaleClay;
                } else {
                    *lith_out = RockType::Sandstone;
                }
                return;
            }

            // 7. Возвышенные кристаллические щиты (Фенноскандия, Канадский щит, Сибирская платформа)
            let shield_val = l_noise * 0.75 + fine_noise * 0.25 + suture * 0.25;
            if shield_val > 0.38 {
                *lith_out = RockType::Granite;
            } else if shield_val > -0.08 {
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
    let orog_age = &geology.orogen_age;
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
                (noise.get([d.x * 26.0 + 13.4, d.y * 26.0 - 27.8, d.z * 26.0 + 41.2]) as f32)
                    .abs();
            let crag_b =
                (noise.get([d.x * 52.0 - 31.9, d.y * 52.0 + 18.5, d.z * 52.0 - 22.7]) as f32)
                    .abs();
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

            // Молодые горы имеют высокую расчленённость, древние пенеплены — сглаженную
            let youth_factor = (1.0 - (orog_age[i] / 4.0).clamp(0.0, 0.45)).clamp(0.55, 1.0);

            let elev_prominence = if !ocean_cell {
                ((elev - 0.08).max(0.0) * 1.45 * youth_factor).clamp(0.0, 0.45)
            } else {
                0.0
            };
            let volcano_roughness = if volcano_mask[i] && !ocean_cell {
                0.18
            } else {
                0.0
            };

            let micro_score = micro_crags
                * rock_retention
                * youth_factor
                * (0.14 + slope_score * 0.55 + elev_prominence * 0.60);

            *rug_out = (slope_score + elev_prominence * 0.55 + volcano_roughness + micro_score)
                .clamp(0.0, 1.0);
        });
}