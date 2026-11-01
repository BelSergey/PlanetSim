// src/domains/climate/solver/moisture.rs

use rayon::prelude::*;

use super::{
    super::{
        climate_config::ClimateConfig,
        climate_data::SeasonalSnapshot,
    },
    SeasonScratch, SolverPrecalc, smoothstep,
};
use crate::{domains::tectonics::tectonics_data::GeologyLayer, world::topology::WorldTopology};

#[inline]
pub(super) fn saturation_capacity(temp_c: f32) -> f32 {
    (0.25 * (0.052 * temp_c).exp()).clamp(0.004, 10.0)
}

#[inline]
fn open_water_fraction(albedo: f32, ocean_albedo: f32) -> f32 {
    let ice_factor = ((albedo - ocean_albedo) / 0.70).clamp(0.0, 1.0);
    (1.0 - 0.85 * ice_factor).max(0.08)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compute_season_moisture(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    config: &ClimateConfig,
    precalc: &SolverPrecalc,
    surface_albedo: &[f32],
    surface_wetness: &[f32],
    w_air: &[f32],
    upwind_weight_sum: &[f32],
    wind_speed: &[f32],
    scratch: &mut SeasonScratch,
    snap: &mut SeasonalSnapshot,
) {
    let num_cells = topology.cells.len();
    let nbrs = &topology.cells;
    let elevs = &geology.elevations;
    let offs = &precalc.neighbor_offsets;
    let hex_rad = precalc.avg_hex_rad;

    // 1. Опорная скорость ветра по суше (чтобы трение о рельеф не останавливало перенос над материками)
    let mut land_wind_sum = 0.0_f32;
    let mut land_cnt = 0usize;
    for i in 0..num_cells {
        if elevs[i] > 0.0 {
            land_wind_sum += wind_speed[i];
            land_cnt += 1;
        }
    }
    let avg_wind = if land_cnt > 0 {
        (land_wind_sum / land_cnt as f32).max(1e-4)
    } else {
        (wind_speed.iter().copied().sum::<f32>() / num_cells.max(1) as f32).max(1e-4)
    };
    let ref_wind = avg_wind * 1.15;

    // 2. Влагоемкость нижней тропосферы (облачного слоя 850 гПа) и стартовый пар
    for i in 0..num_cells {
        let p = snap.pressure[i].clamp(-1.0, 1.0);
        let sea_level_t = snap.temperatures[i] + precalc.mountain_cooling[i];
        let cloud_layer_t = if elevs[i] <= 0.0 {
            snap.temperatures[i]
        } else {
            (sea_level_t * 0.45 + precalc.ocean_equilibrium[i] * 0.55) - precalc.mountain_cooling[i]
                + p * 2.5
        };

        let cap = saturation_capacity(cloud_layer_t);
        scratch.moisture_capacity[i] = cap;

        let init_rh = {
            if geology.is_ocean_cell(i) {
                let ice_bonus = if surface_wetness[i] < 0.20 { 0.20 } else { 1.0 };
                let open_water =
                    open_water_fraction(snap.albedo[i], config.ocean_albedo) * ice_bonus;
                let base_rh = 0.80 + 0.15 * open_water;
                let subsidence_mod = 1.0 - 0.18 * p.max(0.0);
                (base_rh * subsidence_mod) * config.ocean_evaporation_rate.clamp(0.5, 1.15)
            }
            // WaterKind::Lake => {
            //     // Малые озёра ближе к влажной суше, крупные — к заливу. Freshwater evaporates
            //     // slightly more eagerly than saltwater, but the lake's area is the natural cap.
            //     let open_water = open_water_fraction(snap.albedo[i], config.ocean_albedo);
            //     let size_factor = precalc.lake_moderation_scale[i]; // 0.15..0.45
            //     // 0.15 → ~0.59, 0.45 → ~0.78
            //     let base_rh = 0.50 + 0.62 * size_factor;
            //     let subsidence_mod = 1.0 - 0.18 * p.max(0.0);
            //     (base_rh * open_water * subsidence_mod)
            //         * config.ocean_evaporation_rate.clamp(0.5, 1.15)
            // }
            else {
                let base_alb = surface_albedo[i];
                let wetness = surface_wetness[i].max(0.18);
                let snow_cover = ((snap.albedo[i] - base_alb) / 0.50).clamp(0.0, 1.0);
                0.18 + 0.35 * wetness + 0.35 * snow_cover
            }
        };

        let (lo, hi) = if geology.is_ocean_cell(i) {
            (0.35, 0.96) 
        } else {
            (0.15, 0.85)
        };
        // WaterKind::Lake => (0.30, 0.90),
        scratch.specific_humidity[i] = cap * init_rh.clamp(lo, hi);
    }

    // 3. ЭТАП 1: Чистая направленная адвекция по ветру (без паразитного разбавления боковыми клетками)
    let mean_courant = 0.58_f32;
    let total_advection_rad = (1.65 * config.moisture_advection_strength).clamp(0.30, 3.50);
    let num_steps =
        ((total_advection_rad / (hex_rad * mean_courant)).round() as usize).clamp(35, 360);
    let inv_steps = 1.0 / num_steps as f32;

    let recycling_cfg = config.land_moisture_recycling.clamp(0.0, 0.95);
    let mountain_shadow_rate = (config.orographic_rain_shadow * 0.045).clamp(0.03, 0.75);

    for _ in 0..num_steps {
        std::mem::swap(&mut scratch.specific_humidity, &mut scratch.prev_humidity);
        let prev = &scratch.prev_humidity;
        let cap_arr = &scratch.moisture_capacity;
        let out = &mut scratch.specific_humidity;

        out.par_iter_mut().enumerate().for_each(|(i, q_out)| {
            let is_ocean = geology.is_ocean_cell(i);
            //let is_lake  = wk == WaterKind::Lake;

            let sp = wind_speed[i];
            let my_cap = cap_arr[i];
            let neighbors = &nbrs[i].neighbors;
            let my_p = snap.pressure[i].clamp(-1.0, 1.0);

            // Корень .sqrt() подтягивает умеренные континентальные ветры, чтобы они не застревали у берега
            let norm_sp = (sp / ref_wind).clamp(0.0, 1.50).sqrt();
            let step_rate = (norm_sp * 0.72).clamp(0.06, 0.82);
            let travel_step = norm_sp * inv_steps;
            let w_sum = upwind_weight_sum[i];

            let incoming_q = if sp > 1e-5 && w_sum > 0.0 {
                let start = offs[i];
                let end = offs[i + 1];
                let mut upwind_q = 0.0;
                for j in start..end {
                    let w = w_air[j];
                    if w > 0.0 {
                        upwind_q += prev[neighbors[j - start]] * w;
                    }
                }
                upwind_q / w_sum
            } else {
                prev[i]
            };

            let mut q = prev[i] * (1.0 - step_rate) + incoming_q * step_rate;

            if is_ocean {
                let open_water = open_water_fraction(snap.albedo[i], config.ocean_albedo);
                let base_rh = 0.80 + 0.15 * open_water;
                let subsidence_mod = 1.0
                    - 0.20 * (my_p.max(0.0) * config.subsidence_drying_strength).clamp(0.0, 1.0);
                let target_rh =
                    (base_rh * subsidence_mod * config.ocean_evaporation_rate.clamp(0.5, 1.15))
                        .clamp(0.35, 0.96);
                let target_q = my_cap * target_rh;
                let evap_step = (5.5 * open_water * config.ocean_evaporation_rate * inv_steps)
                    .clamp(0.04, 0.45);
                q += (target_q - q) * evap_step;
            // } else if is_lake {
            //     // Озеро: испаряет охотнее океана на клетку (пресная вода), но занимает мало клеток,
            //     // поэтому суммарный вклад в атмосферу ограничен площадью — не нужно резать per-cell rate.
            //     let open_water = open_water_fraction(snap.albedo[i], config.ocean_albedo);
            //     // Озеро слабее подвержено субсиденции: тонкий слой быстро прогревается и отдаёт пар.
            //     let subsidence_mod = 1.0
            //         - 0.15 * (my_p.max(0.0) * config.subsidence_drying_strength).clamp(0.0, 1.0);
            //     let base_rh = 0.72 + 0.15 * open_water;
            //     let target_rh =
            //         (base_rh * subsidence_mod * config.ocean_evaporation_rate.clamp(0.5, 1.15))
            //             .clamp(0.40, 0.92);
            //     let target_q = my_cap * target_rh;
            //     let evap_step = (6.0 * open_water * config.ocean_evaporation_rate * inv_steps)
            //         .clamp(0.05, 0.40);
            //     q += (target_q - q) * evap_step;
            } else {
                let wetness = surface_wetness[i].max(0.20);
                let local_recycling = (recycling_cfg * (0.45 + 0.55 * wetness)).clamp(0.0, 0.92);

                // Плавное естественное вымывание влаги по мере продвижения вглубь материка
                let base_decay = 0.42 * (1.0 - local_recycling * 0.75) * travel_step;
                q *= (1.0 - base_decay).clamp(0.95, 1.0);

                // Рециклинг и транспирация (биотический насос рек и растительности)
                if snap.temperatures[i] > -6.0 {
                    let evap_target_rh = (0.24 + 0.55 * wetness).clamp(0.22, 0.82);
                    let evap_target_q = my_cap * evap_target_rh;
                    if q < evap_target_q {
                        let evap_rate =
                            (1.2 * wetness * recycling_cfg * travel_step).clamp(0.0, 0.12);
                        q += (evap_target_q - q) * evap_rate;
                    }
                }
            }

            // Орографический сброс лишнего пара при подъеме на холодные горные хребты
            if q > my_cap {
                q -= (q - my_cap) * mountain_shadow_rate;
            }
            *q_out = q.min(my_cap * 1.15);
        });
    }

    // 4. ЭТАП 2: Турбулентная диффузия (ПОСЛЕ адвекции, чтобы не гасить ветровые струи в пути)
    let smooth_iters = ((0.055 / hex_rad).round() as usize).clamp(3, 12);
    for _ in 0..smooth_iters {
        std::mem::swap(&mut scratch.specific_humidity, &mut scratch.prev_humidity);
        let prev = &scratch.prev_humidity;
        let cap_arr = &scratch.moisture_capacity;
        let out = &mut scratch.specific_humidity;

        out.par_iter_mut().enumerate().for_each(|(i, q_out)| {
            let neighbors = &nbrs[i].neighbors;
            if neighbors.is_empty() {
                *q_out = prev[i];
                return;
            }
            let my_p = snap.pressure[i].clamp(-1.0, 1.0);
            let chaos_mix = (0.35 - my_p * 0.15).clamp(0.15, 0.50);
            let sum: f32 = neighbors.iter().map(|&n_id| prev[n_id]).sum();
            let nbr_q = sum / neighbors.len() as f32;
            *q_out = (prev[i] * (1.0 - chaos_mix) + nbr_q * chaos_mix).min(cap_arr[i]);
        });
    }

    // 5. ЭТАП 3: Оседание воздуха (субсиденция) в антициклонах и финальная RH [0.0 .. 1.0]
    for i in 0..num_cells {
        let my_cap = scratch.moisture_capacity[i];
        let mut rh = (scratch.specific_humidity[i] / my_cap).clamp(0.0, 1.0);
        let my_p = snap.pressure[i].clamp(-1.0, 1.0);

        if my_p > 0.05 {
            let sub_strength = config.subsidence_drying_strength.clamp(0.0, 2.0);
            if geology.is_ocean_cell(i) {
                    let ocean_drying = (my_p - 0.05) * 0.22 * sub_strength;
                    rh = (rh * (1.0 - ocean_drying)).max(0.45);
                }
                // WaterKind::Lake => {
                //     // Озеро не высохнет от антициклона так, как суша: поверхность подпитывается.
                //     // Но и не остаётся 0.96, как океан: площадь мала, приток пара ограничен.
                //     let lake_drying = (my_p - 0.05) * 0.30 * sub_strength;
                //     rh = (rh * (1.0 - lake_drying)).max(0.55);
                // }
                else {
                    let cold_protection = smoothstep((8.0 - snap.temperatures[i]) / 25.0);
                    let wetness = surface_wetness[i];
                    let dry_floor = 0.12 + 0.15 * wetness;
                    let land_drying = ((my_p - 0.05)
                        * 0.58
                        * sub_strength
                        * (1.0 - 0.65 * cold_protection))
                        .clamp(0.0, 0.85);
                    if rh > dry_floor {
                        rh -= (rh - dry_floor) * land_drying;
                    }
                }
        } else if my_p < -0.05 && !geology.is_ocean_cell(i) {
            // В циклонах восходящие потоки повышают RH только над сушей —
            // океан/озеро уже близки к насыщению.
            let cyclone_boost = (-my_p - 0.05) * 0.18;
            rh = (rh + (1.0 - rh) * cyclone_boost).clamp(0.0, 1.0);
        }

        snap.humidity[i] = rh.clamp(0.0, 1.0);
        scratch.specific_humidity[i] = rh * my_cap;
    }
}
