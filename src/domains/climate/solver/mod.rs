// src/domains/climate/solver/mod.rs

use bevy::math::Vec3;
use noise::{Fbm, NoiseFn, Perlin};
use rayon::prelude::*;

mod circulation;
mod continentality;
mod moisture;
mod precipitation;
mod thermodynamics;

use super::{
    climate_config::ClimateConfig,
    climate_data::{ClimateLayer, SeasonalSnapshot},
};
use crate::{
    domains::{generator::GenerationLogger, tectonics::tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};

use circulation::{compute_ocean_currents, compute_season_pressure_and_winds};
use continentality::compute_continentality;
use moisture::compute_season_moisture;
use precipitation::compute_season_precipitation;
use thermodynamics::{calc_albedo, calc_radiative_sea_temp, elevation_cooling};

#[inline]
pub(super) fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Предрассчитанные константы сетки и рельефа.
pub(super) struct SolverPrecalc {
    pub avg_hex_rad: f32,
    pub sin_lat: Vec<f32>,
    pub cos_lat: Vec<f32>,
    pub east: Vec<Vec3>,
    pub north: Vec<Vec3>,
    pub mountain_cooling: Vec<f32>,
    pub ocean_equilibrium: Vec<f32>,
    pub ocean_ref_temp: Vec<f32>,
    pub moderation: Vec<f32>,
    pub bathymetry_shift: Vec<f32>,

    // --- Предвычисленные повороты (Rodrigues) ---
    /// cos(-sin_lat * coriolis_strength)
    pub coriolis_cos: Vec<f32>,
    /// sin(-sin_lat * coriolis_strength)
    pub coriolis_sin: Vec<f32>,
    /// cos(-sin_lat * 0.75) — спираль Экмана
    pub ekman_cos: Vec<f32>,
    /// sin(-sin_lat * 0.75)
    pub ekman_sin: Vec<f32>,

    // --- Когерентный 3D-шум синоптических волн (от seed мира) ---
    pub synoptic_noise_a: Vec<f32>,
    pub synoptic_noise_b: Vec<f32>,
    pub pressure_wave_noise: Vec<f32>,

    // --- Плоские касательные направления к соседям ---
    /// Для каждой пары (клетка, сосед) — единичный вектор в касательной плоскости.
    /// Уложены подряд, разбиение по клеткам — через neighbor_offsets.
    pub tangent_dirs_flat: Vec<Vec3>,
    /// Индекс клетки-владельца для каждой записи в плоском списке соседей.
    pub edge_cell_ids: Vec<u32>,
    /// Индексы соседних клеток в том же плоском порядке, что tangent_dirs_flat.
    pub neighbor_ids_flat: Vec<u32>,
    /// neighbor_offsets[i]..neighbor_offsets[i+1] — диапазон в плоских массивах соседей клетки i.
    /// Длина — num_cells + 1.
    pub neighbor_offsets: Vec<usize>,
}

impl SolverPrecalc {
    fn new(
        topology: &WorldTopology,
        geology: &GeologyLayer,
        continentality: &[f32],
        config: &ClimateConfig,
        seed: u32,
    ) -> Self {
        let num_cells = topology.cells.len();
        let avg_hex_rad = (4.0 * std::f32::consts::PI / num_cells as f32).sqrt();
        // На сфере площадь тропиков (cos(lat)) значительно больше площади полюсов,
        // поэтому средняя температура поверхности смещена к экватору (~0.68 вместо 0.50)
        let global_avg = config.pole_temp + (config.equator_temp - config.pole_temp) * 0.68;
        let r = config.marginal_sea_moderation_reduction;

        let synoptic_perlin = Fbm::<Perlin>::new(seed.wrapping_add(0x9E37_79B9));

        let mut sin_lat = Vec::with_capacity(num_cells);
        let mut cos_lat = Vec::with_capacity(num_cells);
        let mut east = Vec::with_capacity(num_cells);
        let mut north = Vec::with_capacity(num_cells);
        let mut mountain_cooling = Vec::with_capacity(num_cells);
        let mut ocean_equilibrium = Vec::with_capacity(num_cells);
        let mut ocean_ref_temp = Vec::with_capacity(num_cells);
        let mut moderation = Vec::with_capacity(num_cells);
        let mut bathymetry_shift = Vec::with_capacity(num_cells);

        let mut coriolis_cos = Vec::with_capacity(num_cells);
        let mut coriolis_sin = Vec::with_capacity(num_cells);
        let mut ekman_cos = Vec::with_capacity(num_cells);
        let mut ekman_sin = Vec::with_capacity(num_cells);

        let mut synoptic_noise_a = Vec::with_capacity(num_cells);
        let mut synoptic_noise_b = Vec::with_capacity(num_cells);
        let mut pressure_wave_noise = Vec::with_capacity(num_cells);

        let mut tangent_dirs_flat = Vec::new();
        let mut edge_cell_ids = Vec::new();
        let mut neighbor_ids_flat = Vec::new();
        let mut neighbor_offsets = Vec::with_capacity(num_cells + 1);
        neighbor_offsets.push(0usize);

        // Each cell's geographic constants, noise samples and tangent neighbor vectors are independent.
        // Compute them in parallel, then flatten the variable-length neighbor lists in stable cell order.
        let precalc_cells: Vec<_> = topology.cells.par_iter().enumerate().map(|(i, cell)| {
            let pos = cell.center;
            let lat = cell.lat_lon.x;
            let elev = geology.elevations[i];
            let c = continentality[i];

            let (s_lat, c_lat) = lat.sin_cos();
            let east = Vec3::Y.cross(pos).normalize_or_zero();
            let north = pos.cross(east).normalize_or_zero();

            let mountain_cooling = elevation_cooling(
                elev,
                config.mountain_base_level,
                config.mountain_blend_width,
            ) * config.altitude_lapse_rate;

            let annual_lat_insolation = c_lat.max(0.0);
            let ocean_lat_baseline = config.pole_temp
                + (config.equator_temp - config.pole_temp) * annual_lat_insolation * 0.94;
            let ocean_equilibrium = ocean_lat_baseline * 0.88 + global_avg * 0.12;
            let ocean_ref_temp = ocean_lat_baseline * 0.78 + global_avg * 0.22;

            let moderation = if c < 0.0 {
                config.base_ocean_moderation * (1.0 - r * (1.0 - c.abs()))
            } else {
                config.base_ocean_moderation * (1.0 - r) * (1.0 - c)
            };

            let bathymetry_shift = if elev <= 0.0 && lat.abs() > 1.0 {
                let deep_water_factor = smoothstep((elev - (-0.06)) / (-0.22 - (-0.06)));
                (deep_water_factor - 0.35) * 6.0
            } else {
                0.0
            };

            let friction_k = if elev <= 0.0 { 3.6 } else { 2.4 };
            let max_cor_rad = if elev <= 0.0 { 1.12 } else { 0.85 };
            let cor_ang = (-(s_lat * friction_k * config.coriolis_strength).atan()
                * (max_cor_rad / std::f32::consts::FRAC_PI_2))
                .clamp(-1.35, 1.35);
            let (coriolis_sin, coriolis_cos) = cor_ang.sin_cos();

            let ek_ang = (-(s_lat * 3.2).atan() * 0.65).clamp(-1.2, 1.2);
            let (ekman_sin, ekman_cos) = ek_ang.sin_cos();

            let p = pos.as_dvec3() * 4.5;
            let synoptic_noise_a =
                synoptic_perlin.get([p.x + 13.5, p.y + 27.1, p.z + 41.9]) as f32;
            let synoptic_noise_b =
                synoptic_perlin.get([p.x - 53.2, p.y + 18.4, p.z - 29.7]) as f32;
            let pressure_wave_noise = synoptic_perlin.get([
                p.x * 0.65 + 71.0,
                p.y * 0.65 - 33.0,
                p.z * 0.65 + 12.0,
            ]) as f32;

            let mut tangent_dirs = Vec::with_capacity(cell.neighbors.len());
            let mut neighbor_ids = Vec::with_capacity(cell.neighbors.len());
            for &neighbor_id in &cell.neighbors {
                let raw = topology.cells[neighbor_id].center - pos;
                tangent_dirs.push((raw - pos * raw.dot(pos)).normalize_or_zero());
                neighbor_ids.push(neighbor_id as u32);
            }

            (
                s_lat, c_lat, east, north, mountain_cooling,
                ocean_equilibrium, ocean_ref_temp, moderation, bathymetry_shift,
                coriolis_cos, coriolis_sin, ekman_cos, ekman_sin,
                synoptic_noise_a, synoptic_noise_b, pressure_wave_noise,
                tangent_dirs,
                vec![i as u32; cell.neighbors.len()],
                neighbor_ids,
            )
        }).collect();

        for (
            s_lat, c_lat, e, n, m_height,
            ocean_eq, ocean_ref, mod_val, b_shift,
            cor_cos, cor_sin, ek_cos, ek_sin,
            noise_a, noise_b, wave_noise, tangent_dirs, cell_ids, neighbor_ids,
        ) in precalc_cells {
            sin_lat.push(s_lat);
            cos_lat.push(c_lat);
            east.push(e);
            north.push(n);
            mountain_cooling.push(m_height);
            ocean_equilibrium.push(ocean_eq);
            ocean_ref_temp.push(ocean_ref);
            moderation.push(mod_val);
            bathymetry_shift.push(b_shift);
            coriolis_cos.push(cor_cos);
            coriolis_sin.push(cor_sin);
            ekman_cos.push(ek_cos);
            ekman_sin.push(ek_sin);
            synoptic_noise_a.push(noise_a);
            synoptic_noise_b.push(noise_b);
            pressure_wave_noise.push(wave_noise);
            tangent_dirs_flat.extend(tangent_dirs);
            edge_cell_ids.extend(cell_ids);
            neighbor_ids_flat.extend(neighbor_ids);
            neighbor_offsets.push(tangent_dirs_flat.len());
        }

        Self {
            avg_hex_rad,
            sin_lat,
            cos_lat,
            east,
            north,
            mountain_cooling,
            ocean_equilibrium,
            ocean_ref_temp,
            moderation,
            bathymetry_shift,
            coriolis_cos,
            coriolis_sin,
            ekman_cos,
            ekman_sin,
            synoptic_noise_a,
            synoptic_noise_b,
            pressure_wave_noise,
            tangent_dirs_flat,
            edge_cell_ids,
            neighbor_ids_flat,
            neighbor_offsets,
        }
    }
}

/// Переиспользуемые буферы памяти на один сезон.
pub(super) struct SeasonScratch {
    pub sea_level_temps: Vec<f32>,
    pub ocean_temps: Vec<f32>,
    pub prev_ocean: Vec<f32>,
    pub advected_temps: Vec<f32>,
    pub prev_temps: Vec<f32>,
    pub thermal_field: Vec<f32>,
    pub prev_thermal: Vec<f32>,
    pub prev_pressure: Vec<f32>,
    pub prev_currents: Vec<Vec3>,

    // --- Постоянное на сезон поле широтных поясов давления ---
    pub latitudinal_pressure: Vec<f32>,

    // --- Буферы переноса влаги и осадков ---
    pub specific_humidity: Vec<f32>,
    pub prev_humidity: Vec<f32>,
    pub moisture_capacity: Vec<f32>,
    pub prev_precipitation: Vec<f32>,
}

impl SeasonScratch {
    fn new(num_cells: usize) -> Self {
        Self {
            sea_level_temps: vec![0.0; num_cells],
            ocean_temps: vec![0.0; num_cells],
            prev_ocean: vec![0.0; num_cells],
            advected_temps: vec![0.0; num_cells],
            prev_temps: vec![0.0; num_cells],
            thermal_field: vec![0.0; num_cells],
            prev_thermal: vec![0.0; num_cells],
            prev_pressure: vec![0.0; num_cells],
            prev_currents: vec![Vec3::ZERO; num_cells],
            latitudinal_pressure: vec![0.0; num_cells],
            specific_humidity: vec![0.0; num_cells],
            prev_humidity: vec![0.0; num_cells],
            moisture_capacity: vec![0.0; num_cells],
            prev_precipitation: vec![0.0; num_cells],
        }
    }
}

/// Главная функция: считает континентальность 1 раз, затем параллельно 3 сезона.
pub fn simulate_all_seasons(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &mut ClimateLayer,
    config: &ClimateConfig,
    seed: u32,
    logger: &GenerationLogger,
) {
    logger.start_step("Климат: расчет континентальности и геометрии сетки...");
    compute_continentality(topology, geology, climate, config);

    let precalc = SolverPrecalc::new(topology, geology, &climate.continentality, config, seed);

    // Деструктуризация для disjoint-заимствований (continentality — immut, snapshots — mut).
    let ClimateLayer {
        continentality,
        surface_albedo,
        surface_wetness,
        surface_heat_capacity,
        spring,
        summer,
        winter,
        ..
    } = climate;

    let cont: &[f32] = continentality;
    let surf_alb: &[f32] = surface_albedo;
    let surf_wet: &[f32] = surface_wetness;
    let surf_hc: &[f32] = surface_heat_capacity;

    logger.start_step("Климат: симуляция 3 сезонов (Весна, Лето, Зима)...");
    rayon::scope(|s| {
        s.spawn(|_| {
            simulate_single_season(
                0.0, topology, geology, cont, surf_alb, surf_wet, surf_hc, config, &precalc, spring,
            );
        });
        s.spawn(|_| {
            simulate_single_season(
                1.0, topology, geology, cont, surf_alb, surf_wet, surf_hc, config, &precalc, summer,
            );
        });
        s.spawn(|_| {
            simulate_single_season(
                -1.0, topology, geology, cont, surf_alb, surf_wet, surf_hc, config, &precalc,
                winter,
            );
        });
    });

    logger.start_step("Климат: индексация 3D-стрелок ветров и течений...");
    let sampled_cells = sample_cells_on_sphere(topology, 900);

    for snap in [spring, summer, winter] {
        snap.wind_arrows_cache = build_arrows_from_samples(&sampled_cells, &snap.winds_3d, None);
        snap.current_arrows_cache = build_arrows_from_samples(
            &sampled_cells,
            &snap.ocean_currents,
            Some(&geology.elevations),
        );
    }

    climate.apply_active_season(config.active_season);
}

/// Среднесуточная солнечная энергия на верхней границе атмосферы.
///
/// Результат нормирован относительно экватора в день равноденствия.
/// Учитывает продолжительность дня, полярный день и полярную ночь.
#[inline]
fn daily_mean_insolation(
    sin_lat: f32,
    cos_lat: f32,
    sin_declination: f32,
    cos_declination: f32,
) -> f32 {
    let cos_lat = cos_lat.max(0.0);

    // Косинус часового угла восхода / захода Солнца.
    let denominator = (cos_lat * cos_declination).max(1e-7);

    let cos_hour_angle = (-(sin_lat * sin_declination) / denominator).clamp(-1.0, 1.0);

    let hour_angle = cos_hour_angle.acos();

    // Интеграл солнечного потока за светлую часть суток.
    let daylight =
        hour_angle * sin_lat * sin_declination + cos_lat * cos_declination * hour_angle.sin();

    daylight.max(0.0)
}

#[allow(clippy::too_many_arguments)]
fn simulate_single_season(
    season_mult: f32,
    topology: &WorldTopology,
    geology: &GeologyLayer,
    continentality: &[f32],
    surface_albedo: &[f32],
    surface_wetness: &[f32],
    surface_heat_capacity: &[f32],
    config: &ClimateConfig,
    precalc: &SolverPrecalc,
    snap: &mut SeasonalSnapshot,
) {
    let season_started = std::time::Instant::now();
    let mut pressure_winds_us = 0u128;
    let mut ocean_currents_us = 0u128;
    let mut ocean_advection_us = 0u128;
    let mut air_advection_us = 0u128;

    let num_cells = topology.cells.len();
    let sun_lat = config.axial_tilt * season_mult;
    let (sin_sun, cos_sun) = sun_lat.sin_cos();

    let mut scratch = SeasonScratch::new(num_cells);

    // --- 1. Первичная оценка температуры от Солнца (ячейки независимы) ---
    snap.albedo.par_iter_mut()
        .zip(snap.temperatures.par_iter_mut())
        .zip(scratch.sea_level_temps.par_iter_mut())
        .enumerate()
        .for_each(|(i, ((albedo_out, temperature_out), sea_temperature_out))| {
            let elev = geology.elevations[i];
            let base_insolation = daily_mean_insolation(
                precalc.sin_lat[i], precalc.cos_lat[i], sin_sun, cos_sun,
            );
            let temp_est = config.pole_temp
                + (config.equator_temp - config.pole_temp) * base_insolation;
            let temp_with_height = temp_est - precalc.mountain_cooling[i];
            *albedo_out = calc_albedo(temp_with_height, elev, surface_albedo[i], config);
            let rad_sea_temp = calc_radiative_sea_temp(
                i, *albedo_out, base_insolation, precalc, config, geology,
            );
            *sea_temperature_out = rad_sea_temp;
            *temperature_out = rad_sea_temp - precalc.mountain_cooling[i];
        });

    // --- 2. Широтные пояса давления (ячейки независимы) ---
    scratch.latitudinal_pressure.par_iter_mut().enumerate().for_each(|(i, pressure_out)| {
        let c = continentality[i];
        let lat = topology.cells[i].lat_lon.x;
        let lon = topology.cells[i].lat_lon.y;
        let belt_lat = lat - sun_lat * 0.55 + precalc.pressure_wave_noise[i] * 0.045;

        let polar_damping = 0.5 + 0.5 * precalc.cos_lat[i];
        let base_belt = -(6.0 * belt_lat).cos() * polar_damping;

        let sin_2lat_abs = (2.0 * precalc.sin_lat[i] * precalc.cos_lat[i]).abs();
        let noise_rossby = precalc.pressure_wave_noise[i] * 0.25 * sin_2lat_abs;

        let belt_envelope = if c < 0.0 {
            let ocean_core = smoothstep(-c);
            let rossby_wave =
                1.0 + (0.30 * (3.0 * lon + belt_lat * 2.0).cos() * sin_2lat_abs) + noise_rossby;
            (0.55 + 0.85 * ocean_core) * rossby_wave
        } else {
            (1.0 - 0.15 * smoothstep(c)) * (1.0 + noise_rossby * 0.5)
        };

        *pressure_out = base_belt * belt_envelope * config.latitudinal_pressure_weight;
    });

    // --- 3. Буферы весов адвекции (аллоцируются 1 раз на сезон) ---
    let neighbor_ids = &precalc.neighbor_ids_flat;
    let elevs = &geology.elevations;
    let offs = &precalc.neighbor_offsets;
    let tdirs = &precalc.tangent_dirs_flat;
    let total_dirs = tdirs.len();

    let mut w_air = vec![0.0f32; total_dirs];
    let mut total_air = vec![0.0f32; num_cells];
    let mut wind_dir = vec![Vec3::ZERO; num_cells];
    let mut wind_speed = vec![0.0f32; num_cells];

    let mut w_ocean = vec![0.0f32; total_dirs];
    let mut total_ocean = vec![0.0f32; num_cells];
    let mut current_dir = vec![Vec3::ZERO; num_cells];
    let mut current_speed = vec![0.0f32; num_cells];

    let ref_current_speed = (1.5 * config.ocean_current_speed_multiplier).max(0.05);
    let ocean_advection_max = config.ocean_advection_rate.min(0.85);

    // --- 4. Цикл равновесия ---
    let iterations = config.equilibrium_iterations.max(1);
    for _ in 0..iterations {
        // А) Давление и ветра
        let phase_started = std::time::Instant::now();
        compute_season_pressure_and_winds(
            season_mult,
            topology,
            geology,
            continentality,
            config,
            precalc,
            &mut scratch,
            snap,
        );
        pressure_winds_us += phase_started.elapsed().as_micros();

        // Б) Течения
        let phase_started = std::time::Instant::now();
        compute_ocean_currents(
            topology,
            geology,
            continentality,
            config,
            precalc,
            &mut scratch,
            snap,
        );
        ocean_currents_us += phase_started.elapsed().as_micros();

        // В) Предвычислить wind_dir / wind_speed (1 раз на итерацию)
        wind_speed
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, sp)| *sp = snap.winds_3d[i].length());
        wind_dir.par_iter_mut().enumerate().for_each(|(i, dir_out)| {
            let sp = wind_speed[i];
            *dir_out = if sp > 0.001 { snap.winds_3d[i] / sp } else { Vec3::ZERO };
        });

        // Веса воздушной адвекции: каждый элемент плоского списка независим.
        w_air.par_iter_mut().enumerate().for_each(|(j, weight_out)| {
            let cell_id = precalc.edge_cell_ids[j] as usize;
            let alignment = tdirs[j].dot(wind_dir[cell_id]);
            *weight_out = if alignment < 0.0 { alignment * alignment } else { 0.0 };
        });
        total_air.par_iter_mut().enumerate().for_each(|(i, total_out)| {
            *total_out = w_air[offs[i]..offs[i + 1]].iter().sum();
        });

        // Г) Предвычислить current_dir / current_speed + веса (только вода)
        current_dir.par_iter_mut()
            .zip(current_speed.par_iter_mut())
            .zip(total_ocean.par_iter_mut())
            .enumerate()
            .for_each(|(i, ((dir_out, speed_out), total_out))| {
                if elevs[i] > 0.0 {
                    *dir_out = Vec3::ZERO;
                    *speed_out = 0.0;
                    *total_out = 0.0;
                } else {
                    let current = snap.ocean_currents[i];
                    let speed = current.length();
                    *speed_out = speed;
                    *dir_out = if speed > 0.001 { current / speed } else { Vec3::ZERO };
                }
            });
        w_ocean.par_iter_mut().enumerate().for_each(|(j, weight_out)| {
            let cell_id = precalc.edge_cell_ids[j] as usize;
            let neighbor_id = neighbor_ids[j] as usize;
            if elevs[cell_id] > 0.0 || elevs[neighbor_id] > 0.0 {
                *weight_out = 0.0;
            } else {
                let alignment = tdirs[j].dot(current_dir[cell_id]);
                *weight_out = if alignment < 0.0 { alignment * alignment } else { 0.0 };
            }
        });
        total_ocean.par_iter_mut().enumerate().for_each(|(i, total_out)| {
            *total_out = if elevs[i] > 0.0 {
                0.0
            } else {
                w_ocean[offs[i]..offs[i + 1]].iter().sum()
            };
        });

        // Д) Океаническая адвекция (15 итераций)
        scratch
            .ocean_temps
            .copy_from_slice(&scratch.sea_level_temps);
        let phase_started = std::time::Instant::now();
        for _ in 0..15 {
            std::mem::swap(&mut scratch.ocean_temps, &mut scratch.prev_ocean);
            let prev = &scratch.prev_ocean;
            let out = &mut scratch.ocean_temps;

            out.par_iter_mut().enumerate().for_each(|(i, o)| {
                if elevs[i] > 0.0 || current_speed[i] < 0.001 || total_ocean[i] <= 0.0 {
                    *o = prev[i];
                    return;
                }
                let start = offs[i];
                let end = offs[i + 1];
                let mut upwind = 0.0;
                for (j, w) in w_ocean.iter().enumerate().take(end).skip(start) {
                    if *w > 0.0 {
                        let n_id = neighbor_ids[j] as usize;
                        upwind += prev[n_id] * w;
                    }
                }
                let incoming = upwind / total_ocean[i];
                let norm_speed = (current_speed[i] / ref_current_speed).clamp(0.0, 1.0);
                let step_rate = norm_speed * ocean_advection_max;
                *o = prev[i] * (1.0 - step_rate) + incoming * step_rate;
            });
        }

        ocean_advection_us += phase_started.elapsed().as_micros();

        // Смешиваем тепло океана в sea_level_temps
        let ocean_temps = &scratch.ocean_temps;
        scratch.sea_level_temps.par_iter_mut().enumerate().for_each(|(i, sea_temp)| {
            if elevs[i] <= 0.0 {
                *sea_temp = *sea_temp * 0.25 + ocean_temps[i] * 0.75;
            }
        });

        // Е) Воздушная адвекция (25 итераций)
        scratch
            .advected_temps
            .copy_from_slice(&scratch.sea_level_temps);
        let phase_started = std::time::Instant::now();
        for _ in 0..25 {
            std::mem::swap(&mut scratch.advected_temps, &mut scratch.prev_temps);
            let prev = &scratch.prev_temps;
            let out = &mut scratch.advected_temps;

            out.par_iter_mut().enumerate().for_each(|(i, o)| {
                let sp = wind_speed[i];
                if sp < 0.001 || total_air[i] <= 0.0 {
                    *o = prev[i];
                    return;
                }
                let start = offs[i];
                let end = offs[i + 1];
                let mut upwind = 0.0;
                for (j, w) in w_air.iter().enumerate().take(end).skip(start) {
                    if *w > 0.0 {
                        let n_id = neighbor_ids[j] as usize;
                        upwind += prev[n_id] * w;
                    }
                }
                let incoming = upwind / total_air[i];
                let step_rate = (sp * 0.06).clamp(0.0, 0.55);
                *o = prev[i] * (1.0 - step_rate) + incoming * step_rate;
            });
        }

        air_advection_us += phase_started.elapsed().as_micros();

        // Ж) Радиационный баланс
        let air_blend = (config.heat_advection_strength * 0.55).clamp(0.0, 0.85);
        let ocean_blend = config.ocean_advection_rate.clamp(0.0, 0.85);

        let ocean_temps = &scratch.ocean_temps;
        let advected_temps = &scratch.advected_temps;
        snap.temperatures.par_iter_mut()
            .zip(snap.albedo.par_iter_mut())
            .zip(scratch.sea_level_temps.par_iter_mut())
            .enumerate()
            .for_each(|(i, ((temperature_out, albedo_out), sea_temp_out))| {
                let base_insolation =
                    (precalc.cos_lat[i] * cos_sun + precalc.sin_lat[i] * sin_sun).max(0.0);
                let rad_temp = calc_radiative_sea_temp(
                    i, *albedo_out, base_insolation, precalc, config, geology,
                );
                let elev = elevs[i];
                if elev <= 0.0 {
                    let transported_water_temp =
                        ocean_temps[i] * 0.8 + advected_temps[i] * 0.2;
                    *sea_temp_out = rad_temp * (1.0 - ocean_blend) + transported_water_temp * ocean_blend;
                } else {
                    *sea_temp_out = rad_temp * (1.0 - air_blend) + advected_temps[i] * air_blend;
                }
                let mut final_temp = *sea_temp_out - precalc.mountain_cooling[i];
                if elev > 0.0 {
                    let hc = surface_heat_capacity[i].clamp(0.0, 1.0);
                    if hc > 0.01 {
                        let annual_mean = config.pole_temp
                            + (config.equator_temp - config.pole_temp) * precalc.cos_lat[i] * 0.82;
                        let damping = (1.0 - hc * 0.55).max(0.35);
                        final_temp = annual_mean + (final_temp - annual_mean) * damping;
                        *sea_temp_out = final_temp + precalc.mountain_cooling[i];
                    }
                }
                *temperature_out = final_temp;
                *albedo_out = calc_albedo(final_temp, elev, surface_albedo[i], config);
            });
    }

    // Финальная синхронизация давления, ветров и течений
    let phase_started = std::time::Instant::now();
    compute_season_pressure_and_winds(
        season_mult,
        topology,
        geology,
        continentality,
        config,
        precalc,
        &mut scratch,
        snap,
    );
    pressure_winds_us += phase_started.elapsed().as_micros();
    let phase_started = std::time::Instant::now();
    compute_ocean_currents(
        topology,
        geology,
        continentality,
        config,
        precalc,
        &mut scratch,
        snap,
    );
    ocean_currents_us += phase_started.elapsed().as_micros();
    // Обновляем веса ветров по финальному полю winds_3d и считаем влажность сезона
    wind_speed
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, sp)| *sp = snap.winds_3d[i].length());
    w_air.par_iter_mut().enumerate().for_each(|(j, weight_out)| {
        let cell_id = precalc.edge_cell_ids[j] as usize;
        let speed = wind_speed[cell_id];
        let dir = if speed > 0.001 {
            snap.winds_3d[cell_id] / speed
        } else {
            Vec3::ZERO
        };
        let alignment = tdirs[j].dot(dir);
        *weight_out = if alignment < 0.0 { alignment * alignment } else { 0.0 };
    });
    total_air.par_iter_mut().enumerate().for_each(|(i, total_out)| {
        *total_out = w_air[offs[i]..offs[i + 1]].iter().sum();
    });

    let moisture_started = std::time::Instant::now();
    compute_season_moisture(
        topology,
        geology,
        config,
        precalc,
        surface_albedo,
        surface_wetness,
        &w_air,
        &total_air,
        &wind_speed,
        &mut scratch,
        snap,
    );
    let moisture_us = moisture_started.elapsed().as_micros();
    let precipitation_started = std::time::Instant::now();

    compute_season_precipitation(
        topology,
        geology,
        config,
        precalc,
        surface_wetness,
        &w_air,
        &total_air,
        &wind_speed,
        &mut scratch,
        snap,
    );
    let precipitation_us = precipitation_started.elapsed().as_micros();
    let total_us = season_started.elapsed().as_micros();
    let measured_us = pressure_winds_us
        + ocean_currents_us
        + ocean_advection_us
        + air_advection_us
        + moisture_us
        + precipitation_us;
    let other_us = total_us.saturating_sub(measured_us);

    bevy::log::info!(
        "CLIMATE_PROFILE: season={}, pressure_winds={} ms, ocean_currents={} ms, ocean_advection={} ms, air_advection={} ms, moisture={} ms, precipitation={} ms, other={} ms, total={} ms",
        season_mult,
        pressure_winds_us / 1000,
        ocean_currents_us / 1000,
        ocean_advection_us / 1000,
        air_advection_us / 1000,
        moisture_us / 1000,
        precipitation_us / 1000,
        other_us / 1000,
        total_us / 1000
    );
}

/// Равномерно распределённые точки на сфере (golden spiral).
fn sample_cells_on_sphere(topology: &WorldTopology, count: usize) -> Vec<usize> {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    let mut result = Vec::with_capacity(count);
    let denom = (count.saturating_sub(1)).max(1) as f32;
    let mut used = std::collections::HashSet::with_capacity(count);

    // Параллельный подбор ближайших клеток для всех точек выборки
    let nearest: Vec<usize> = (0..count).into_par_iter().map(|k| {
        let y = 1.0 - (k as f32 / denom) * 2.0;
        let radius = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * k as f32;
        let sample = bevy::math::Vec3::new(theta.cos() * radius, y, theta.sin() * radius);

        let mut best_i = 0usize;
        let mut best_d = f32::MAX;
        for (i, cell) in topology.cells.iter().enumerate() {
            let distance = sample.distance_squared(cell.center);
            if distance < best_d {
                best_d = distance;
                best_i = i;
            }
        }
        best_i
    }).collect();

    for best_i in nearest {
        if used.insert(best_i) {
            result.push(best_i);
        }
    }
    result
}

/// Строит готовый кэш стрелок (индекс клетки, нормированная скорость) по 95-му процентилю.
fn build_arrows_from_samples(
    samples: &[usize],
    vectors: &[bevy::math::Vec3],
    elevations: Option<&[f32]>,
) -> Vec<(usize, f32)> {
    let mut speeds: Vec<f32> = vectors.par_iter().map(|v| v.length()).collect();
    speeds.par_sort_unstable_by(|a, b| a.total_cmp(b));
    let idx = ((speeds.len() as f32) * 0.95) as usize;
    let max_speed = speeds.get(idx).copied().unwrap_or(0.0001).max(0.0001);

    let mut arrows = Vec::with_capacity(samples.len());
    for &i in samples {
        if let Some(elevs) = elevations
            && elevs[i] > 0.0
        {
            continue;
        }
        let speed = vectors[i].length();
        if speed < max_speed * 0.05 {
            continue;
        }
        let s = (speed / max_speed).clamp(0.0, 1.0).sqrt();
        arrows.push((i, s));
    }
    arrows
}
