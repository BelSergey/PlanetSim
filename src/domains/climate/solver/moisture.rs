// src/domains/climate/solver/moisture.rs

use rayon::prelude::*;
use std::time::Instant;

use super::{
    super::{climate_config::ClimateConfig, climate_data::SeasonalSnapshot},
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

/// Параметры переноса, которые не меняются между временными шагами адвекции.
/// Вычисляются один раз на клетку, чтобы не повторять sqrt/clamp/expensive коэффициенты
/// до 360 раз на каждом сезонном расчёте.
#[derive(Clone, Copy, Default)]
struct MoistureTransportParams {
    step_rate: f32,
    decay_factor: f32,
    evaporation_target: f32,
    evaporation_rate: f32,
    is_ocean: bool,
    land_can_evaporate: bool,
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
    let total_started = Instant::now();
    let preparation_started = Instant::now();

    let num_cells = topology.cells.len();
    let neighbor_ids = &precalc.neighbor_ids_flat;
    let offs = &precalc.neighbor_offsets;
    let hex_rad = precalc.avg_hex_rad;
    // Плоский список только апвинд-вкладов (w > 0)
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

    // 1. Опорная скорость ветра по суше (чтобы трение о рельеф не останавливало перенос над материками)
    let (land_wind_sum, land_cnt) = wind_speed
        .par_iter()
        .enumerate()
        .filter(|(i, _)| !geology.is_ocean_cell(*i))
        .map(|(_, speed)| (*speed, 1usize))
        .reduce(|| (0.0_f32, 0usize), |a, b| (a.0 + b.0, a.1 + b.1));
    let avg_wind = if land_cnt > 0 {
        (land_wind_sum / land_cnt as f32).max(1e-4)
    } else {
        (wind_speed.par_iter().copied().sum::<f32>() / num_cells.max(1) as f32).max(1e-4)
    };
    let ref_wind = avg_wind * 1.15;

    // 2. Влагоемкость нижней тропосферы (облачного слоя 850 гПа) и стартовый пар
    scratch
        .specific_humidity
        .par_iter_mut()
        .zip(scratch.moisture_capacity.par_iter_mut())
        .enumerate()
        .for_each(|(i, (humidity_out, capacity_out))| {
            let p = snap.pressure[i].clamp(-1.0, 1.0);
            // Температура воздушной массы на высоте переноса облаков не испытывает резкого скачка
            // на линии пляжа (смешиваем приземную температуру уровня моря с широтным фоном океана
            // и вычитаем реальное горное охлаждение mountain_cooling)
            let sea_level_t = snap.temperatures[i] + precalc.mountain_cooling[i];
            let is_ocean = geology.is_ocean_cell(i);
            let cloud_layer_t = if is_ocean {
                snap.temperatures[i]
            } else {
                (sea_level_t * 0.45 + precalc.ocean_equilibrium[i] * 0.55)
                    - precalc.mountain_cooling[i]
                    + p * 2.5
            };

            let cap = saturation_capacity(cloud_layer_t);
            *capacity_out = cap;

            if is_ocean {
                let ice_bonus = if surface_wetness[i] < 0.20 { 0.20 } else { 1.0 };
                let open_water =
                    open_water_fraction(snap.albedo[i], config.ocean_albedo) * ice_bonus;
                let base_rh = 0.80 + 0.15 * open_water;
                let subsidence_mod = 1.0 - 0.18 * p.max(0.0);
                let init_rh =
                    (base_rh * subsidence_mod) * config.ocean_evaporation_rate.clamp(0.5, 1.15);
                *humidity_out = cap * init_rh.clamp(0.35, 0.96);
            } else {
                let base_alb = surface_albedo[i];
                let wetness = surface_wetness[i].max(0.18);
                let snow_cover = ((snap.albedo[i] - base_alb) / 0.50).clamp(0.0, 1.0);
                let init_land_rh = (0.18 + 0.35 * wetness + 0.35 * snow_cover).clamp(0.15, 0.85);
                *humidity_out = cap * init_land_rh;
            }
        });

    // 3. ЭТАП 1: Чистая направленная адвекция по ветру (без паразитного разбавления боковыми клетками)
    let mean_courant = 0.58_f32;
    let total_advection_rad = (1.65 * config.moisture_advection_strength).clamp(0.30, 3.50);
    let num_steps =
        ((total_advection_rad / (hex_rad * mean_courant)).round() as usize).clamp(35, 90);
    let inv_steps = 1.0 / num_steps as f32;

    let recycling_cfg = config.land_moisture_recycling.clamp(0.0, 0.95);
    let mountain_shadow_rate = (config.orographic_rain_shadow * 0.045).clamp(0.03, 0.75);

    let preparation_ms = preparation_started.elapsed().as_millis();
    let coefficients_started = Instant::now();

    // Эти величины постоянны для клетки на протяжении всех шагов адвекции.
    // Считаем их один раз вместо повторения дорогих операций внутри каждого шага.
    let mut transport_params = vec![MoistureTransportParams::default(); num_cells];
    transport_params
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, params)| {
            let sp = wind_speed[i];
            let norm_sp = (sp / ref_wind).clamp(0.0, 1.50).sqrt();
            let step_rate = (norm_sp * 0.72).clamp(0.06, 0.82);
            let my_cap = scratch.moisture_capacity[i];

            if geology.is_ocean_cell(i) {
                let my_p = snap.pressure[i].clamp(-1.0, 1.0);
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

                *params = MoistureTransportParams {
                    step_rate,
                    decay_factor: 1.0,
                    evaporation_target: target_q,
                    evaporation_rate: evap_step,
                    is_ocean: true,
                    land_can_evaporate: false,
                };
            } else {
                let wetness = surface_wetness[i].max(0.20);
                let local_recycling = (recycling_cfg * (0.45 + 0.55 * wetness)).clamp(0.0, 0.92);
                let travel_step = norm_sp * inv_steps;
                let base_decay = 0.42 * (1.0 - local_recycling * 0.75) * travel_step;
                let decay_factor = (1.0 - base_decay).clamp(0.95, 1.0);
                let evap_target_rh = (0.24 + 0.55 * wetness).clamp(0.22, 0.82);
                let evap_target_q = my_cap * evap_target_rh;
                let evap_rate = (1.2 * wetness * recycling_cfg * travel_step).clamp(0.0, 0.12);

                *params = MoistureTransportParams {
                    step_rate,
                    decay_factor,
                    evaporation_target: evap_target_q,
                    evaporation_rate: evap_rate,
                    is_ocean: false,
                    land_can_evaporate: snap.temperatures[i] > -6.0,
                };
            }
        });

    let coefficients_ms = coefficients_started.elapsed().as_millis();
    let advection_started = Instant::now();

    for _ in 0..num_steps {
        std::mem::swap(&mut scratch.specific_humidity, &mut scratch.prev_humidity);
        let prev = &scratch.prev_humidity;
        let cap_arr = &scratch.moisture_capacity;
        let out = &mut scratch.specific_humidity;

        out.par_iter_mut().enumerate().for_each(|(i, q_out)| {
            let params = transport_params[i];
            let my_cap = cap_arr[i];
            let w_sum = upwind_weight_sum[i];

            let incoming_q = if wind_speed[i] > 1e-5 && w_sum > 0.0 {
                let mut upwind_q = 0.0_f32;
                for k in upwind_offs[i] as usize..upwind_offs[i + 1] as usize {
                    upwind_q += prev[upwind_nbrs[k] as usize] * upwind_w[k];
                }
                upwind_q / w_sum
            } else {
                prev[i]
            };

            let mut q = prev[i] * (1.0 - params.step_rate) + incoming_q * params.step_rate;

            if params.is_ocean {
                q += (params.evaporation_target - q) * params.evaporation_rate;
            } else {
                // Плавное естественное вымывание влаги по мере продвижения вглубь материка
                q *= params.decay_factor;

                // Рециклинг и транспирация (биотический насос рек и растительности)
                if params.land_can_evaporate && q < params.evaporation_target {
                    q += (params.evaporation_target - q) * params.evaporation_rate;
                }
            }

            // Орографический сброс лишнего пара при подъеме на холодные горные хребты
            if q > my_cap {
                q -= (q - my_cap) * mountain_shadow_rate;
            }
            *q_out = q.min(my_cap * 1.15);
        });
    }

    let advection_ms = advection_started.elapsed().as_millis();

    // 4. ЭТАП 2: Турбулентная диффузия (ПОСЛЕ адвекции, чтобы не гасить ветровые струи в пути)
    let smooth_iters = ((0.045 / hex_rad).round() as usize).clamp(3, 8);
    let diffusion_started = Instant::now();

    for _ in 0..smooth_iters {
        std::mem::swap(&mut scratch.specific_humidity, &mut scratch.prev_humidity);
        let prev = &scratch.prev_humidity;
        let cap_arr = &scratch.moisture_capacity;
        let out = &mut scratch.specific_humidity;

        out.par_iter_mut().enumerate().for_each(|(i, q_out)| {
            let start = offs[i];
            let end = offs[i + 1];
            let degree = end - start;
            if degree == 0 {
                *q_out = prev[i];
                return;
            }
            let my_p = snap.pressure[i].clamp(-1.0, 1.0);
            let chaos_mix = (0.35 - my_p * 0.15).clamp(0.15, 0.50);
            let mut sum = 0.0f32;
            for j in start..end {
                sum += prev[neighbor_ids[j] as usize];
            }
            let nbr_q = sum / degree as f32;
            *q_out = (prev[i] * (1.0 - chaos_mix) + nbr_q * chaos_mix).min(cap_arr[i]);
        });
    }

    let diffusion_ms = diffusion_started.elapsed().as_millis();
    let final_rh_started = Instant::now();

    // 5. ЭТАП 3: Оседание воздуха (субсиденция) в антициклонах и финальная RH [0.0 .. 1.0]
    snap.humidity
        .par_iter_mut()
        .zip(scratch.specific_humidity.par_iter_mut())
        .enumerate()
        .for_each(|(i, (humidity_out, specific_humidity_out))| {
            let my_cap = scratch.moisture_capacity[i];
            let mut rh = (*specific_humidity_out / my_cap).clamp(0.0, 1.0);
            let my_p = snap.pressure[i].clamp(-1.0, 1.0);

            if my_p > 0.05 {
                let cold_protection = smoothstep((8.0 - snap.temperatures[i]) / 25.0);
                let sub_strength = config.subsidence_drying_strength.clamp(0.0, 2.0);
                if geology.is_ocean_cell(i) {
                    // Над океаном антициклон лишь умеренно снижает влажность пограничного слоя
                    let ocean_drying = (my_p - 0.05) * 0.22 * sub_strength;
                    rh = (rh * (1.0 - ocean_drying)).max(0.45);
                } else {
                    // Над сушей нисходящие потоки антициклона формируют аридные пояса и пустыни
                    let wetness = surface_wetness[i];
                    let dry_floor = 0.12 + 0.15 * wetness;
                    let land_drying =
                        ((my_p - 0.05) * 0.58 * sub_strength * (1.0 - 0.65 * cold_protection))
                            .clamp(0.0, 0.85);
                    if rh > dry_floor {
                        rh -= (rh - dry_floor) * land_drying;
                    }
                }
            } else if my_p < -0.05 && !geology.is_ocean_cell(i) {
                // В циклонах восходящие потоки приближают воздух к точке росы
                let cyclone_boost = (-my_p - 0.05) * 0.18;
                rh = (rh + (1.0 - rh) * cyclone_boost).clamp(0.0, 1.0);
            }

            *humidity_out = rh.clamp(0.0, 1.0);
            *specific_humidity_out = rh * my_cap;
        });

    let final_rh_ms = final_rh_started.elapsed().as_millis();
    let total_ms = total_started.elapsed().as_millis();

    bevy::log::info!(
        "MOISTURE_PROFILE: steps={}, diffusion_steps={}, preparation={} ms, coefficients={} ms, advection={} ms, diffusion={} ms, final_rh={} ms, total={} ms",
        num_steps,
        smooth_iters,
        preparation_ms,
        coefficients_ms,
        advection_ms,
        diffusion_ms,
        final_rh_ms,
        total_ms
    );
}
