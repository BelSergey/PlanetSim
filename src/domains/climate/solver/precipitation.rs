// src/domains/climate/solver/precipitation.rs

use rayon::prelude::*;

use super::{
    super::{climate_config::ClimateConfig, climate_data::SeasonalSnapshot},
    SeasonScratch, SolverPrecalc,
};
use crate::{domains::tectonics::tectonics_data::GeologyLayer, world::topology::WorldTopology};

#[allow(clippy::too_many_arguments)]
pub(super) fn compute_season_precipitation(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    config: &ClimateConfig,
    precalc: &SolverPrecalc,
    surface_wetness: &[f32],
    w_air: &[f32],
    upwind_weight_sum: &[f32],
    wind_speed: &[f32],
    scratch: &mut SeasonScratch,
    snap: &mut SeasonalSnapshot,
) {
    let num_cells = topology.cells.len();
    let neighbor_ids = &precalc.neighbor_ids_flat;
    let elevs = &geology.elevations;
    let offs = &precalc.neighbor_offsets;
    let tdirs = &precalc.tangent_dirs_flat;
    let hex_rad = precalc.avg_hex_rad;

    let mut upwind_nbrs: Vec<u32> = Vec::with_capacity(neighbor_ids.len());
    let mut upwind_w: Vec<f32> = Vec::with_capacity(neighbor_ids.len());
    let mut upwind_offs: Vec<u32> = Vec::with_capacity(num_cells + 1);
    upwind_offs.push(0);
    for i in 0..num_cells {
        for j in offs[i]..offs[i + 1] {
            let w = w_air[j];
            if w > 0.0 {
                upwind_nbrs.push(neighbor_ids[j]);
                upwind_w.push(w);
            }
        }
        upwind_offs.push(upwind_nbrs.len() as u32);
    }

    let sum_wind: f32 = wind_speed.par_iter().copied().sum();
    let avg_wind = (sum_wind / num_cells.max(1) as f32).max(1e-4);
    let ref_wind = avg_wind * 1.15;

    let oro_strength = (config.orographic_rain_shadow / 7.0).clamp(0.0, 4.0);
    let precip_mult = config.precipitation_rate.max(0.0);
    let recycling = config.land_moisture_recycling.clamp(0.0, 0.90);

    let q_arr = &scratch.specific_humidity;
    let cap_arr = &scratch.moisture_capacity;
    let rh_arr = &snap.humidity;
    let pressures = &snap.pressure;
    let winds_3d = &snap.winds_3d;

    let cloud_source = &mut scratch.prev_humidity;

    cloud_source
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, src_out)| {
            let q = q_arr[i];
            let my_cap = cap_arr[i];
            let rh = rh_arr[i];
            let my_p = pressures[i].clamp(-1.0, 1.0);
            let sp = wind_speed[i];
            let norm_sp = (sp / ref_wind).clamp(0.0, 1.45).sqrt();
            let start = offs[i];
            let end = offs[i + 1];
            let degree = end - start;

            // Запас пара с мягкой зависимостью от температуры (чтобы умеренные широты тоже получали осадки)
            let avail_water = (q + 0.08).powf(0.55);
            let rh_factor = ((rh - 0.18).max(0.0) / 0.82).powf(1.25);

            // --- МЕХАНИЗМ 1: Орографический и береговой подъем ---
            let w_sum = upwind_weight_sum[i];
            let mut upslope_lift = 0.0;
            let mut cooling_condensation = 0.0;

            if sp > 1e-5 && w_sum > 0.0 {
                let my_elev = elevs[i].max(0.0);
                let is_land = elevs[i] > 0.0;
                let vertical_scale = 0.025;

                for j in start..end {
                    let w = w_air[j];
                    if w > 0.0 {
                        let n_id = neighbor_ids[j] as usize;
                        let n_elev = elevs[n_id].max(0.0);
                        let dh = (my_elev - n_elev).max(0.0);

                        let coastal_step = if is_land && elevs[n_id] <= 0.0 {
                            0.05
                        } else {
                            0.0
                        };
                        let slope =
                            ((dh * vertical_scale) / hex_rad).clamp(0.0, 2.5) + coastal_step;
                        upslope_lift += slope * w;

                        let excess = (q_arr[n_id] - my_cap).max(0.0);
                        cooling_condensation += excess * w;
                    }
                }
                upslope_lift /= w_sum;
                cooling_condensation /= w_sum;
            }

            let oro_rh_factor = ((rh - 0.15).max(0.0) / 0.85).powf(1.1);
            let oro_clouds = (avail_water * oro_rh_factor * norm_sp * upslope_lift * 1.45
                + cooling_condensation * 1.15)
                * oro_strength;

            // --- МЕХАНИЗМ 2: Горизонтальная сходимость ветров (ВЗК, муссоны и фронты) ---
            let mut convergence = 0.0;
            let my_wind = winds_3d[i];
            for j in start..end {
                let n_id = neighbor_ids[j] as usize;
                let inflow = (winds_3d[n_id] - my_wind).dot(-tdirs[j]);
                convergence += inflow;
            }
            if degree > 0 {
                let raw_conv = convergence / (degree as f32 * avg_wind * hex_rad * 2.0);
                convergence = raw_conv.clamp(0.0, 2.0);
            }

            // --- МЕХАНИЗМ 3: Плавный барический подъем без резких песочных дыр при p > 0.05 ---
            // Подъем плавно снижается от циклонов (p < 0) к умеренным антициклонам (p = 0.45)
            // и полностью гаснет только в ядрах сильных антициклонов
            let cyclone_lift = ((0.42 - my_p).max(0.0) / 0.85).powf(1.35) * 1.65;
            let inversion_suppression = (1.0 - my_p.max(0.0) * 1.35).clamp(0.0, 1.0);
            let conv_lift = convergence * 1.30 * inversion_suppression;

            let dynamic_lift = cyclone_lift + conv_lift;
            let surface_factor = if elevs[i] <= 0.0 {
                0.75
            } else {
                let eff_wet = surface_wetness[i].max(0.20);
                0.75 + 0.55 * eff_wet * recycling
            };

            let syn_clouds = avail_water * rh_factor * dynamic_lift * 1.30 * surface_factor;
            *src_out = (oro_clouds + syn_clouds) * precip_mult;
        });

    snap.precipitation.copy_from_slice(cloud_source);

    // 3. ПОЛЕТ ТУЧ ПО ВЕТРУ
    let mean_courant = 0.58_f32;
    let cloud_travel_rad = (0.85 * config.moisture_advection_strength).clamp(0.20, 2.00);
    let cloud_steps =
        ((cloud_travel_rad / (hex_rad * mean_courant)).round() as usize).clamp(30, 120);
    let inv_cloud_steps = 1.0 / cloud_steps as f32;

    let smooth_iters = ((0.07 / hex_rad).round() as usize).clamp(4, 18);
    let diff_fraction = (smooth_iters as f32 * inv_cloud_steps).clamp(0.02, 0.25);

    for _ in 0..cloud_steps {
        std::mem::swap(&mut snap.precipitation, &mut scratch.prev_precipitation);
        let prev_c = &scratch.prev_precipitation;
        let out_c = &mut snap.precipitation;

        out_c.par_iter_mut().enumerate().for_each(|(i, c_out)| {
            let sp = wind_speed[i];
            let my_p = pressures[i].clamp(-1.0, 1.0);

            let norm_sp = (sp / ref_wind).clamp(0.08, 1.45).sqrt();
            let step_rate = (norm_sp * 0.72).clamp(0.08, 0.85);
            let travel_step = norm_sp * inv_cloud_steps;
            let w_sum = upwind_weight_sum[i];

            let incoming_c = if sp > 1e-5 && w_sum > 0.0 {
                let mut upwind_c = 0.0_f32;
                for k in upwind_offs[i] as usize..upwind_offs[i + 1] as usize {
                    upwind_c += prev_c[upwind_nbrs[k] as usize] * upwind_w[k];
                }
                upwind_c / w_sum
            } else {
                prev_c[i]
            };

            let mut c = prev_c[i] * (1.0 - step_rate) + incoming_c * step_rate;

            let start = offs[i];
            let end = offs[i + 1];
            let degree = end - start;
            if degree > 0 {
                let chaos_mix = (0.45 - my_p * 0.20) * diff_fraction;
                let mut sum = 0.0f32;
                for j in start..end {
                    sum += prev_c[neighbor_ids[j] as usize];
                }
                let nbr_c = sum / degree as f32;
                c = c * (1.0 - chaos_mix) + nbr_c * chaos_mix;
            }

            let src = cloud_source[i];
            if src > c {
                let build_rate = (5.5 * inv_cloud_steps).clamp(0.03, 0.45);
                c += (src - c) * build_rate;
            } else {
                let anticyclone_dry = if my_p > 0.0 { 1.0 + my_p * 2.2 } else { 0.65 };
                let decay_rate = (0.90 * travel_step * anticyclone_dry).clamp(0.003, 0.16);
                c += (src - c) * decay_rate;
            }

            *c_out = c;
        });
    }

    // 4. Финальная нормализация карты осадков [0.0 .. 1.0]
    snap.precipitation
        .par_iter_mut()
        .zip(snap.humidity.par_iter_mut())
        .enumerate()
        .for_each(|(i, (precip_out, humidity_out))| {
            let raw_p = (*precip_out).max(0.0);
            let norm_precip = (1.0 - (-raw_p * 2.20).exp()).clamp(0.0, 1.0);
            *precip_out = norm_precip;

            if elevs[i] > 0.0 {
                let local_boost = (norm_precip * 0.05 + surface_wetness[i] * 0.05) * recycling;
                *humidity_out = (*humidity_out + local_boost).clamp(0.0, 1.0);
            }
        });
}
