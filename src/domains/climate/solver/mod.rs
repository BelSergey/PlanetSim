// src/domains/climate/solver/mod.rs

use bevy::math::Vec3;
use noise::{Fbm, NoiseFn, Perlin};
use rayon::prelude::*;

mod circulation;
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
    pub coriolis_cos: Vec<f32>,
    pub coriolis_sin: Vec<f32>,
    pub ekman_cos: Vec<f32>,
    pub ekman_sin: Vec<f32>,

    // --- Когерентный 3D-шум синоптических волн (от seed мира) ---
    pub synoptic_noise_a: Vec<f32>,
    pub synoptic_noise_b: Vec<f32>,
    pub pressure_wave_noise: Vec<f32>,

    // --- Плоские касательные направления к соседям ---
    pub tangent_dirs_flat: Vec<Vec3>,
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
        let mut neighbor_offsets = Vec::with_capacity(num_cells + 1);
        neighbor_offsets.push(0usize);

        for (i, c) in continentality.iter().enumerate().take(num_cells) {
            let cell = &topology.cells[i];
            let pos = cell.center;
            let lat = cell.lat_lon.x;
            let elev = geology.elevations[i];
            let is_ocean = geology.is_ocean_cell(i);

            let (s_lat, c_lat) = lat.sin_cos();
            sin_lat.push(s_lat);
            cos_lat.push(c_lat);

            let e = Vec3::Y.cross(pos).normalize_or_zero();
            let n = pos.cross(e).normalize_or_zero();
            east.push(e);
            north.push(n);

            let m_height = elevation_cooling(
                elev,
                config.mountain_base_level,
                config.mountain_blend_width,
            );
            mountain_cooling.push(m_height * config.altitude_lapse_rate);

            let annual_lat_insolation = c_lat.max(0.0);
            let ocean_lat_baseline = config.pole_temp
                + (config.equator_temp - config.pole_temp) * annual_lat_insolation * 0.94;
            ocean_equilibrium.push(ocean_lat_baseline * 0.88 + global_avg * 0.12);
            ocean_ref_temp.push(ocean_lat_baseline * 0.78 + global_avg * 0.22);

            let mod_val = if *c < 0.0 {
                config.base_ocean_moderation * (1.0 - r * (1.0 - c.abs()))
            } else {
                config.base_ocean_moderation * (1.0 - r) * (1.0 - c)
            };
            moderation.push(mod_val);

            let b_shift = if is_ocean && lat.abs() > 1.0 {
                let deep_water_factor = smoothstep((elev - (-0.06)) / (-0.22 - (-0.06)));
                (deep_water_factor - 0.35) * 6.0
            } else {
                0.0
            };
            bathymetry_shift.push(b_shift);

            let friction_k = if is_ocean { 3.6 } else { 2.4 };
            let max_cor_rad = if is_ocean { 1.12 } else { 0.85 };
            let cor_ang = (-(s_lat * friction_k * config.coriolis_strength).atan()
                * (max_cor_rad / std::f32::consts::FRAC_PI_2))
                .clamp(-1.35, 1.35);
            let (ss, cs) = cor_ang.sin_cos();
            coriolis_cos.push(cs);
            coriolis_sin.push(ss);

            let ek_ang = (-(s_lat * 3.2).atan() * 0.65).clamp(-1.2, 1.2);
            let (se, ce) = ek_ang.sin_cos();
            ekman_cos.push(ce);
            ekman_sin.push(se);

            let p = pos.as_dvec3() * 4.5;
            synoptic_noise_a
                .push(synoptic_perlin.get([p.x + 13.5, p.y + 27.1, p.z + 41.9]) as f32);
            synoptic_noise_b
                .push(synoptic_perlin.get([p.x - 53.2, p.y + 18.4, p.z - 29.7]) as f32);
            pressure_wave_noise.push(synoptic_perlin.get([
                p.x * 0.65 + 71.0,
                p.y * 0.65 - 33.0,
                p.z * 0.65 + 12.0,
            ]) as f32);

            for &n_id in &cell.neighbors {
                let raw = topology.cells[n_id].center - pos;
                tangent_dirs_flat.push((raw - pos * raw.dot(pos)).normalize_or_zero());
            }
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
    pub latitudinal_pressure: Vec<f32>,
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

/// Главная функция: берет готовую континентальность из геологии, затем параллельно считает 3 сезона.
pub fn simulate_all_seasons(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &mut ClimateLayer,
    config: &ClimateConfig,
    seed: u32,
    logger: &GenerationLogger,
) {
    logger.start_step("Климат: синхронизация континентальности и геометрии сетки...");

    let num_cells = topology.cells.len();
    if geology.continentality.len() == num_cells {
        climate.continentality.clone_from(&geology.continentality);
    } else {
        climate.continentality.resize(num_cells, 0.0);
    }

    let precalc = SolverPrecalc::new(topology, geology, &climate.continentality, config, seed);

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
        snap.current_arrows_cache =
            build_arrows_from_samples(&sampled_cells, &snap.ocean_currents, Some(geology));
    }

    climate.apply_active_season(config.active_season);
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
    let num_cells = topology.cells.len();
    let sun_lat = config.axial_tilt * season_mult;
    let (sin_sun, cos_sun) = sun_lat.sin_cos();

    let mut scratch = SeasonScratch::new(num_cells);

    // --- 1. Первичная оценка температуры от Солнца ---
    for (i, surf_alb) in surface_albedo.iter().enumerate().take(num_cells) {
        let is_ocean = geology.is_ocean_cell(i);
        let base_insolation =
            (precalc.cos_lat[i] * cos_sun + precalc.sin_lat[i] * sin_sun).max(0.0);

        let temp_est =
            config.pole_temp + (config.equator_temp - config.pole_temp) * base_insolation;
        let temp_with_height = temp_est - precalc.mountain_cooling[i];

        snap.albedo[i] = calc_albedo(temp_with_height, is_ocean, *surf_alb, config);
        let rad_sea_temp =
            calc_radiative_sea_temp(i, snap.albedo[i], base_insolation, precalc, config, geology);

        scratch.sea_level_temps[i] = rad_sea_temp;
        snap.temperatures[i] = rad_sea_temp - precalc.mountain_cooling[i];
    }

    // --- 2. Широтные пояса давления ---
    for (i, c) in continentality.iter().enumerate().take(num_cells) {
        let lat = topology.cells[i].lat_lon.x;
        let lon = topology.cells[i].lat_lon.y;
        let belt_lat = lat - sun_lat * 0.55 + precalc.pressure_wave_noise[i] * 0.045;

        let polar_damping = 0.5 + 0.5 * precalc.cos_lat[i];
        let base_belt = -(6.0 * belt_lat).cos() * polar_damping;

        let sin_2lat_abs = (2.0 * precalc.sin_lat[i] * precalc.cos_lat[i]).abs();
        let noise_rossby = precalc.pressure_wave_noise[i] * 0.25 * sin_2lat_abs;

        let belt_envelope = if *c < 0.0 {
            let ocean_core = smoothstep(-c);
            let rossby_wave =
                1.0 + (0.30 * (3.0 * lon + belt_lat * 2.0).cos() * sin_2lat_abs) + noise_rossby;
            (0.55 + 0.85 * ocean_core) * rossby_wave
        } else {
            (1.0 - 0.15 * smoothstep(*c)) * (1.0 + noise_rossby * 0.5)
        };

        scratch.latitudinal_pressure[i] =
            base_belt * belt_envelope * config.latitudinal_pressure_weight;
    }

    // --- 3. Буферы весов адвекции ---
    let nbrs = &topology.cells;
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

        compute_ocean_currents(
            topology,
            geology,
            continentality,
            config,
            precalc,
            &mut scratch,
            snap,
        );

        wind_speed
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, sp)| *sp = snap.winds_3d[i].length());
        for i in 0..num_cells {
            let sp = wind_speed[i];
            wind_dir[i] = if sp > 0.001 {
                snap.winds_3d[i] / sp
            } else {
                Vec3::ZERO
            };
        }

        for i in 0..num_cells {
            let dir = wind_dir[i];
            let start = offs[i];
            let end = offs[i + 1];
            let mut total = 0.0;
            for j in start..end {
                let a = tdirs[j].dot(dir);
                let w = if a < 0.0 { a * a } else { 0.0 };
                w_air[j] = w;
                total += w;
            }
            total_air[i] = total;
        }

        // Предвычислить current_dir / current_speed + веса (только океан)
        for i in 0..num_cells {
            if !geology.is_ocean_cell(i) {
                current_speed[i] = 0.0;
                current_dir[i] = Vec3::ZERO;
                total_ocean[i] = 0.0;
                continue;
            }
            let cur = snap.ocean_currents[i];
            let sp = cur.length();
            current_speed[i] = sp;
            current_dir[i] = if sp > 0.001 { cur / sp } else { Vec3::ZERO };

            let dir = current_dir[i];
            let start = offs[i];
            let end = offs[i + 1];
            let mut total = 0.0;
            for j in start..end {
                let k = j - start;
                let n_id = nbrs[i].neighbors[k];
                if !geology.is_ocean_cell(n_id) {
                    w_ocean[j] = 0.0;
                    continue;
                }
                let a = tdirs[j].dot(dir);
                let w = if a < 0.0 { a * a } else { 0.0 };
                w_ocean[j] = w;
                total += w;
            }
            total_ocean[i] = total;
        }

        // Д) Океаническая адвекция (15 итераций)
        scratch
            .ocean_temps
            .copy_from_slice(&scratch.sea_level_temps);
        for _ in 0..15 {
            std::mem::swap(&mut scratch.ocean_temps, &mut scratch.prev_ocean);
            let prev = &scratch.prev_ocean;
            let out = &mut scratch.ocean_temps;

            out.par_iter_mut().enumerate().for_each(|(i, o)| {
                if !geology.is_ocean_cell(i) || current_speed[i] < 0.001 || total_ocean[i] <= 0.0 {
                    *o = prev[i];
                    return;
                }
                let start = offs[i];
                let end = offs[i + 1];
                let mut upwind = 0.0;
                for (j, w) in w_ocean.iter().enumerate().take(end).skip(start) {
                    if *w > 0.0 {
                        let k = j - start;
                        let n_id = nbrs[i].neighbors[k];
                        upwind += prev[n_id] * w;
                    }
                }
                let incoming = upwind / total_ocean[i];
                let norm_speed = (current_speed[i] / ref_current_speed).clamp(0.0, 1.0);
                let step_rate = norm_speed * ocean_advection_max;
                *o = prev[i] * (1.0 - step_rate) + incoming * step_rate;
            });
        }

        for i in 0..num_cells {
            if geology.is_ocean_cell(i) {
                scratch.sea_level_temps[i] =
                    scratch.sea_level_temps[i] * 0.25 + scratch.ocean_temps[i] * 0.75;
            }
        }

        // Е) Воздушная адвекция (25 итераций)
        scratch
            .advected_temps
            .copy_from_slice(&scratch.sea_level_temps);
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
                        let k = j - start;
                        let n_id = nbrs[i].neighbors[k];
                        upwind += prev[n_id] * w;
                    }
                }
                let incoming = upwind / total_air[i];
                let step_rate = (sp * 0.06).clamp(0.0, 0.55);
                *o = prev[i] * (1.0 - step_rate) + incoming * step_rate;
            });
        }

        // Ж) Радиационный баланс
        let air_blend = (config.heat_advection_strength * 0.55).clamp(0.0, 0.85);
        let ocean_blend = config.ocean_advection_rate.clamp(0.0, 0.85);

        for i in 0..num_cells {
            let is_ocean = geology.is_ocean_cell(i);
            let base_insolation =
                (precalc.cos_lat[i] * cos_sun + precalc.sin_lat[i] * sin_sun).max(0.0);
            let rad_temp = calc_radiative_sea_temp(
                i,
                snap.albedo[i],
                base_insolation,
                precalc,
                config,
                geology,
            );

            if is_ocean {
                let transported_water_temp =
                    scratch.ocean_temps[i] * 0.8 + scratch.advected_temps[i] * 0.2;
                scratch.sea_level_temps[i] =
                    rad_temp * (1.0 - ocean_blend) + transported_water_temp * ocean_blend;
            } else {
                scratch.sea_level_temps[i] =
                    rad_temp * (1.0 - air_blend) + scratch.advected_temps[i] * air_blend;
            }

            let mut final_temp = scratch.sea_level_temps[i] - precalc.mountain_cooling[i];

            if !is_ocean {
                let hc = surface_heat_capacity[i].clamp(0.0, 1.0);
                if hc > 0.01 {
                    let annual_mean = config.pole_temp
                        + (config.equator_temp - config.pole_temp) * precalc.cos_lat[i] * 0.82;
                    let damping = (1.0 - hc * 0.55).max(0.35);
                    final_temp = annual_mean + (final_temp - annual_mean) * damping;
                    scratch.sea_level_temps[i] = final_temp + precalc.mountain_cooling[i];
                }
            }

            snap.temperatures[i] = final_temp;
            snap.albedo[i] = calc_albedo(final_temp, is_ocean, surface_albedo[i], config);
        }
    }

    // Финальная синхронизация давления, ветров и течений
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
    compute_ocean_currents(
        topology,
        geology,
        continentality,
        config,
        precalc,
        &mut scratch,
        snap,
    );

    wind_speed
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, sp)| *sp = snap.winds_3d[i].length());
    for i in 0..num_cells {
        let sp = wind_speed[i];
        let dir = if sp > 0.001 {
            snap.winds_3d[i] / sp
        } else {
            Vec3::ZERO
        };
        let start = offs[i];
        let end = offs[i + 1];
        let mut total = 0.0;
        for j in start..end {
            let a = tdirs[j].dot(dir);
            let w = if a < 0.0 { a * a } else { 0.0 };
            w_air[j] = w;
            total += w;
        }
        total_air[i] = total;
    }

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
}

fn sample_cells_on_sphere(topology: &WorldTopology, count: usize) -> Vec<usize> {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    let mut result = Vec::with_capacity(count);
    let denom = (count.saturating_sub(1)).max(1) as f32;
    let mut used = std::collections::HashSet::with_capacity(count);

    for k in 0..count {
        let y = 1.0 - (k as f32 / denom) * 2.0;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * k as f32;
        let sample = bevy::math::Vec3::new(theta.cos() * r, y, theta.sin() * r);

        let mut best_i = 0usize;
        let mut best_d = f32::MAX;
        for (i, cell) in topology.cells.iter().enumerate() {
            let d = sample.distance_squared(cell.center);
            if d < best_d {
                best_d = d;
                best_i = i;
            }
        }
        if used.insert(best_i) {
            result.push(best_i);
        }
    }
    result
}

fn build_arrows_from_samples(
    samples: &[usize],
    vectors: &[bevy::math::Vec3],
    geology: Option<&GeologyLayer>,
) -> Vec<(usize, f32)> {
    let mut speeds: Vec<f32> = vectors.iter().map(|v| v.length()).collect();
    speeds.sort_by(|a, b| a.total_cmp(b));
    let idx = ((speeds.len() as f32) * 0.95) as usize;
    let max_speed = speeds.get(idx).copied().unwrap_or(0.0001).max(0.0001);

    let mut arrows = Vec::with_capacity(samples.len());
    for &i in samples {
        if let Some(geo) = geology
            && !geo.is_ocean_cell(i)
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