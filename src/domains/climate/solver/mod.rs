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
    /// Source-cell index for every directed edge in the flattened neighbor arrays.
    pub edge_cell_ids: Vec<u32>,
    /// Flat neighbor indices aligned with `tangent_dirs_flat`.
    pub neighbor_ids_flat: Vec<u32>,
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
        let avg_hex_rad = (4.0 * std::f32::consts::PI / num_cells.max(1) as f32).sqrt();
        let global_avg = config.pole_temp + (config.equator_temp - config.pole_temp) * 0.68;
        let r = config.marginal_sea_moderation_reduction;
        let synoptic_perlin = Fbm::<Perlin>::new(seed.wrapping_add(0x9E37_79B9));

        // Все величины на клетку и направленные связи независимы.
        // Считаем локальные записи параллельно, затем уплощаем их в стабильном порядке клеток.
        let precalc_cells: Vec<_> = topology
            .cells
            .par_iter()
            .enumerate()
            .map(|(i, cell)| {
                let pos = cell.center;
                let lat = cell.lat_lon.x;
                let elev = geology.elevations.get(i).copied().unwrap_or(0.0);
                let is_ocean = geology.is_ocean_cell(i);
                let c = continentality.get(i).copied().unwrap_or(0.0);

                let (sin_lat, cos_lat) = lat.sin_cos();
                let east = Vec3::Y.cross(pos).normalize_or_zero();
                let north = pos.cross(east).normalize_or_zero();

                let mountain_cooling = elevation_cooling(
                    elev,
                    config.mountain_base_level,
                    config.mountain_blend_width,
                ) * config.altitude_lapse_rate;

                let annual_lat_insolation = cos_lat.max(0.0);
                let ocean_lat_baseline = config.pole_temp
                    + (config.equator_temp - config.pole_temp) * annual_lat_insolation * 0.94;
                let ocean_equilibrium = ocean_lat_baseline * 0.88 + global_avg * 0.12;
                let ocean_ref_temp = ocean_lat_baseline * 0.78 + global_avg * 0.22;

                let moderation = if c < 0.0 {
                    config.base_ocean_moderation * (1.0 - r * (1.0 - c.abs()))
                } else {
                    config.base_ocean_moderation * (1.0 - r) * (1.0 - c)
                };

                let bathymetry_shift = if is_ocean && lat.abs() > 1.0 {
                    let deep_water_factor = smoothstep((elev - (-0.06)) / (-0.22 - (-0.06)));
                    (deep_water_factor - 0.35) * 6.0
                } else {
                    0.0
                };

                let friction_k = if is_ocean { 3.6 } else { 2.4 };
                let max_cor_rad = if is_ocean { 1.12 } else { 0.85 };
                let cor_ang = (-(sin_lat * friction_k * config.coriolis_strength).atan()
                    * (max_cor_rad / std::f32::consts::FRAC_PI_2))
                    .clamp(-1.35, 1.35);
                let (coriolis_sin, coriolis_cos) = cor_ang.sin_cos();

                let ek_ang = (-(sin_lat * 3.2).atan() * 0.65).clamp(-1.2, 1.2);
                let (ekman_sin, ekman_cos) = ek_ang.sin_cos();

                let p = pos.as_dvec3() * 4.5;
                let noise_a = synoptic_perlin.get([p.x + 13.5, p.y + 27.1, p.z + 41.9]) as f32;
                let noise_b = synoptic_perlin.get([p.x - 53.2, p.y + 18.4, p.z - 29.7]) as f32;
                let pressure_wave_noise =
                    synoptic_perlin.get([p.x * 0.65 + 71.0, p.y * 0.65 - 33.0, p.z * 0.65 + 12.0])
                        as f32;

                let mut tangent_dirs = Vec::with_capacity(cell.neighbors.len());
                let mut neighbor_ids = Vec::with_capacity(cell.neighbors.len());
                for &neighbor_id in &cell.neighbors {
                    let raw = topology.cells[neighbor_id].center - pos;
                    tangent_dirs.push((raw - pos * raw.dot(pos)).normalize_or_zero());
                    neighbor_ids.push(neighbor_id as u32);
                }

                (
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
                    noise_a,
                    noise_b,
                    pressure_wave_noise,
                    tangent_dirs,
                    vec![i as u32; cell.neighbors.len()],
                    neighbor_ids,
                )
            })
            .collect();

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

        for (
            s_lat,
            c_lat,
            e,
            n,
            m_height,
            ocean_eq,
            ocean_ref,
            mod_val,
            b_shift,
            cor_cos,
            cor_sin,
            ek_cos,
            ek_sin,
            noise_a,
            noise_b,
            wave_noise,
            tangent_dirs,
            cell_ids,
            neighbor_ids,
        ) in precalc_cells
        {
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
    pub latitudinal_pressure: Vec<f32>,
    pub specific_humidity: Vec<f32>,
    pub prev_humidity: Vec<f32>,
    pub moisture_capacity: Vec<f32>,
    pub prev_precipitation: Vec<f32>,
}

impl SeasonScratch {
    fn empty() -> Self {
        Self {
            sea_level_temps: Vec::new(),
            ocean_temps: Vec::new(),
            prev_ocean: Vec::new(),
            advected_temps: Vec::new(),
            prev_temps: Vec::new(),
            thermal_field: Vec::new(),
            prev_thermal: Vec::new(),
            prev_pressure: Vec::new(),
            prev_currents: Vec::new(),
            latitudinal_pressure: Vec::new(),
            specific_humidity: Vec::new(),
            prev_humidity: Vec::new(),
            moisture_capacity: Vec::new(),
            prev_precipitation: Vec::new(),
        }
    }

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

/// Кэш между последовательными вызовами `simulate_all_seasons`.
///
/// Использование: генератор создаёт один экземпляр на всю операцию регенерации
/// и передаёт `Some(&mut cache)` в каждый вызов `simulate_all_seasons`.
/// Это позволяет переиспользовать `SolverPrecalc` (экономит ~80 мс) и
/// три `SeasonScratch` (экономит ~30-100 мс аллокаций).
pub struct ClimateSolverCache {
    pub(super) precalc: Option<SolverPrecalc>,
    pub(super) scratch_spring: SeasonScratch,
    pub(super) scratch_summer: SeasonScratch,
    pub(super) scratch_winter: SeasonScratch,
    pub(super) scratch_capacity: usize,
}

impl ClimateSolverCache {
    pub fn new() -> Self {
        Self {
            precalc: None,
            scratch_spring: SeasonScratch::empty(),
            scratch_summer: SeasonScratch::empty(),
            scratch_winter: SeasonScratch::empty(),
            scratch_capacity: 0,
        }
    }

    fn ensure_capacity(&mut self, num_cells: usize) {
        if self.scratch_capacity != num_cells {
            self.scratch_spring = SeasonScratch::new(num_cells);
            self.scratch_summer = SeasonScratch::new(num_cells);
            self.scratch_winter = SeasonScratch::new(num_cells);
            self.scratch_capacity = num_cells;
            // precalc привязан к числу клеток
            self.precalc = None;
        }
    }

    /// Явно сбрасывает `SolverPrecalc`. Вызывайте, если изменились
    /// `topology`, `geology.continentality`, `geology.elevations` или `config`.
    pub fn invalidate_precalc(&mut self) {
        self.precalc = None;
    }
}

impl Default for ClimateSolverCache {
    fn default() -> Self {
        Self::new()
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
    cache: Option<&mut ClimateSolverCache>,   // ← НОВЫЙ параметр
) {
    let num_cells = topology.cells.len();
    if geology.continentality.len() == num_cells {
        climate.continentality.clone_from(&geology.continentality);
    } else {
        climate.continentality.resize(num_cells, 0.0);
    }

    // 1. Получаем или создаём локальный кэш
    let mut local_cache;
    let cache: &mut ClimateSolverCache = match cache {
        Some(c) => {
            c.ensure_capacity(num_cells);
            c
        }
        None => {
            local_cache = ClimateSolverCache::new();
            local_cache.ensure_capacity(num_cells);
            &mut local_cache
        }
    };

    // 2. Precalc — переиспользуем из кэша, если уже есть
    let precalc_cached = cache.precalc.is_some();
    if precalc_cached {
        logger.start_step("Климат: переиспользование предрассчитанной геометрии сетки...");
    } else {
        logger.start_step("Климат: синхронизация континентальности и геометрии сетки...");
        cache.precalc = Some(SolverPrecalc::new(
            topology,
            geology,
            &climate.continentality,
            config,
            seed,
        ));
    }

    // 3. Извлекаем disjoint-поля climate (Rust позволяет, т.к. это отдельные поля)
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

    // 4. Пред-извлечение precalc и scratch — три разных поля кэша, borrow checker ОК
    let precalc: &SolverPrecalc = cache.precalc.as_ref().unwrap();
    let scratch_spring = &mut cache.scratch_spring;
    let scratch_summer = &mut cache.scratch_summer;
    let scratch_winter = &mut cache.scratch_winter;

    // 5. Три сезона параллельно, каждый со своим scratch
    logger.start_step("Климат: симуляция 3 сезонов (Весна, Лето, Зима)...");
    rayon::scope(|s| {
        s.spawn(|_| {
            simulate_single_season(
                0.0, topology, geology, cont, surf_alb, surf_wet, surf_hc, config, precalc,
                scratch_spring, spring,
            );
        });
        s.spawn(|_| {
            simulate_single_season(
                1.0, topology, geology, cont, surf_alb, surf_wet, surf_hc, config, precalc,
                scratch_summer, summer,
            );
        });
        s.spawn(|_| {
            simulate_single_season(
                -1.0, topology, geology, cont, surf_alb, surf_wet, surf_hc, config, precalc,
                scratch_winter, winter,
            );
        });
    });

logger.start_step("Климат: индексация 3D-стрелок ветров и течений...");
for snap in [spring, summer, winter] {
    snap.wind_arrows_cache = build_arrow_cache(topology, &snap.winds_3d, None, 35);
    snap.current_arrows_cache = build_arrow_cache(topology, &snap.ocean_currents, Some(geology), 35);
}

    climate.apply_active_season(config.active_season);
}

#[inline]
fn daily_mean_insolation(
    sin_lat: f32,
    cos_lat: f32,
    sin_declination: f32,
    cos_declination: f32,
) -> f32 {
    let cos_lat = cos_lat.max(0.0);
    let denominator = (cos_lat * cos_declination).max(1e-7);
    let cos_hour_angle = (-(sin_lat * sin_declination) / denominator).clamp(-1.0, 1.0);
    let hour_angle = cos_hour_angle.acos();
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
    mut scratch: &mut SeasonScratch,   // ← было: создавался внутри
    snap: &mut SeasonalSnapshot,
) {
    let sun_lat = config.axial_tilt * season_mult;
    let num_cells = topology.cells.len();
    let (sin_sun, cos_sun) = sun_lat.sin_cos();


    // --- 1. Первичная оценка температуры от Солнца ---
    let mountain_cooling = &precalc.mountain_cooling;
    let sin_lat = &precalc.sin_lat;
    let cos_lat = &precalc.cos_lat;
    snap.albedo
        .par_iter_mut()
        .zip(snap.temperatures.par_iter_mut())
        .zip(scratch.sea_level_temps.par_iter_mut())
        .zip(surface_albedo.par_iter())
        .enumerate()
        .for_each(
            |(i, (((albedo_out, temperature_out), sea_temperature_out), surf_alb))| {
                let is_ocean = geology.is_ocean_cell(i);
                let base_insolation =
                    daily_mean_insolation(sin_lat[i], cos_lat[i], sin_sun, cos_sun);
                let temp_est =
                    config.pole_temp + (config.equator_temp - config.pole_temp) * base_insolation;
                let temp_with_height = temp_est - mountain_cooling[i];
                *albedo_out = calc_albedo(temp_with_height, is_ocean, *surf_alb, config);
                let rad_sea_temp = calc_radiative_sea_temp(
                    i,
                    *albedo_out,
                    base_insolation,
                    precalc,
                    config,
                    geology,
                );
                *sea_temperature_out = rad_sea_temp;
                *temperature_out = rad_sea_temp - mountain_cooling[i];
            },
        );

    // --- 2. Широтные пояса давления ---
    scratch
        .latitudinal_pressure
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, pressure_out)| {
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

    // --- 3. Буферы весов адвекции ---
    let offs = &precalc.neighbor_offsets;
    let tdirs = &precalc.tangent_dirs_flat;
    let neighbor_ids = &precalc.neighbor_ids_flat;
    let edge_cell_ids = &precalc.edge_cell_ids;
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
            .zip(wind_dir.par_iter_mut())
            .enumerate()
            .for_each(|(i, (speed_out, dir_out))| {
                let wind = snap.winds_3d[i];
                let speed = wind.length();
                *speed_out = speed;
                *dir_out = if speed > 0.001 {
                    wind / speed
                } else {
                    Vec3::ZERO
                };
            });

        w_air.par_iter_mut().enumerate().for_each(|(j, weight)| {
            let i = edge_cell_ids[j] as usize;
            let alignment = tdirs[j].dot(wind_dir[i]);
            *weight = if alignment < 0.0 {
                alignment * alignment
            } else {
                0.0
            };
        });
        total_air.par_iter_mut().enumerate().for_each(|(i, total)| {
            *total = w_air[offs[i]..offs[i + 1]].iter().sum();
        });

        // Предвычислить current_dir / current_speed + веса (только океан)
        current_speed
            .par_iter_mut()
            .zip(current_dir.par_iter_mut())
            .enumerate()
            .for_each(|(i, (speed_out, dir_out))| {
                let current = if geology.is_ocean_cell(i) {
                    snap.ocean_currents[i]
                } else {
                    Vec3::ZERO
                };
                let speed = current.length();
                *speed_out = speed;
                *dir_out = if speed > 0.001 {
                    current / speed
                } else {
                    Vec3::ZERO
                };
            });

        w_ocean.par_iter_mut().enumerate().for_each(|(j, weight)| {
            let i = edge_cell_ids[j] as usize;
            let n_id = neighbor_ids[j] as usize;
            if !geology.is_ocean_cell(i) || !geology.is_ocean_cell(n_id) {
                *weight = 0.0;
                return;
            }
            let alignment = tdirs[j].dot(current_dir[i]);
            *weight = if alignment < 0.0 {
                alignment * alignment
            } else {
                0.0
            };
        });
        total_ocean
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, total)| {
                *total = if geology.is_ocean_cell(i) {
                    w_ocean[offs[i]..offs[i + 1]].iter().sum()
                } else {
                    0.0
                };
            });

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

        let ocean_temps = &scratch.ocean_temps;
        scratch
            .sea_level_temps
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, sea_temp)| {
                if geology.is_ocean_cell(i) {
                    *sea_temp = *sea_temp * 0.25 + ocean_temps[i] * 0.75;
                }
            });

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
                        let n_id = neighbor_ids[j] as usize;
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

        let ocean_temps = &scratch.ocean_temps;
        let advected_temps = &scratch.advected_temps;
        let mountain_cooling = &precalc.mountain_cooling;
        let cos_lat = &precalc.cos_lat;
        snap.temperatures
            .par_iter_mut()
            .zip(snap.albedo.par_iter_mut())
            .zip(scratch.sea_level_temps.par_iter_mut())
            .zip(surface_albedo.par_iter())
            .zip(surface_heat_capacity.par_iter())
            .enumerate()
            .for_each(
                |(i, ((((temperature_out, albedo_out), sea_temp), surf_alb), heat_capacity))| {
                    let is_ocean = geology.is_ocean_cell(i);
                    let base_insolation = (cos_lat[i] * cos_sun + sin_lat[i] * sin_sun).max(0.0);
                    let rad_temp = calc_radiative_sea_temp(
                        i,
                        *albedo_out,
                        base_insolation,
                        precalc,
                        config,
                        geology,
                    );

                    if is_ocean {
                        let transported = ocean_temps[i] * 0.8 + advected_temps[i] * 0.2;
                        *sea_temp = rad_temp * (1.0 - ocean_blend) + transported * ocean_blend;
                    } else {
                        *sea_temp = rad_temp * (1.0 - air_blend) + advected_temps[i] * air_blend;
                    }

                    let mut final_temp = *sea_temp - mountain_cooling[i];
                    if !is_ocean {
                        let hc = heat_capacity.clamp(0.0, 1.0);
                        if hc > 0.01 {
                            let annual_mean = config.pole_temp
                                + (config.equator_temp - config.pole_temp) * cos_lat[i] * 0.82;
                            let damping = (1.0 - hc * 0.55).max(0.35);
                            final_temp = annual_mean + (final_temp - annual_mean) * damping;
                            *sea_temp = final_temp + mountain_cooling[i];
                        }
                    }

                    *temperature_out = final_temp;
                    *albedo_out = calc_albedo(final_temp, is_ocean, *surf_alb, config);
                },
            );
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
        .for_each(|(i, speed)| *speed = snap.winds_3d[i].length());
    w_air.par_iter_mut().enumerate().for_each(|(j, weight)| {
        let i = edge_cell_ids[j] as usize;
        let speed = wind_speed[i];
        let direction = if speed > 0.001 {
            snap.winds_3d[i] / speed
        } else {
            Vec3::ZERO
        };
        let alignment = tdirs[j].dot(direction);
        *weight = if alignment < 0.0 {
            alignment * alignment
        } else {
            0.0
        };
    });
    total_air.par_iter_mut().enumerate().for_each(|(i, total)| {
        *total = w_air[offs[i]..offs[i + 1]].iter().sum();
    });

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

/// Строит кэш стрелок O(N) через equal-area bucketing по широтным полосам.
/// Заменяет пару `sample_cells_on_sphere` + `build_arrows_from_samples`.
fn build_arrow_cache(
    topology: &WorldTopology,
    vectors: &[Vec3],
    geology: Option<&GeologyLayer>,
    num_lat_bins: usize,
) -> Vec<(usize, f32)> {
    use std::f32::consts::PI;
    let num_cells = topology.cells.len();
    let lat_bins = num_lat_bins.max(4);
    let max_buckets = lat_bins * lat_bins;

    let mut best_cell = vec![u32::MAX; max_buckets];
    let mut best_speed = vec![0.0f32; max_buckets];

for (i, &v) in vectors.iter().enumerate().take(num_cells) {
    if let Some(geo) = geology
        && !geo.is_ocean_cell(i)
    {
        continue;
    }
    let speed = v.length();
        if speed < 1e-4 {
            continue;
        }

        let cell = &topology.cells[i];
        let sin_lat = cell.lat_lon.x.sin();
        let lat_bin =
            ((sin_lat + 1.0) * 0.5 * lat_bins as f32).min(lat_bins as f32 - 0.001) as usize;

        let lat_center = (lat_bin as f32 + 0.5) / lat_bins as f32 * 2.0 - 1.0;
        let cos_lat = (1.0 - lat_center * lat_center).max(0.0).sqrt();
        let lon_per_band = (lat_bins as f32 * cos_lat).round().max(1.0) as usize;

        let lon_norm = (cell.lat_lon.y + PI) / (2.0 * PI);
        let lon_bin =
            (lon_norm * lon_per_band as f32).min(lon_per_band as f32 - 0.001) as usize;

        let bucket = lat_bin * lat_bins + lon_bin;
        if bucket < max_buckets && speed > best_speed[bucket] {
            best_speed[bucket] = speed;
            best_cell[bucket] = i as u32;
        }
    }

    // 95-й перцентиль вместо максимума — устойчив к единичным пикам
    let mut speeds: Vec<f32> = best_speed.iter().copied().filter(|&s| s > 0.0).collect();
    speeds.sort_by(|a, b| a.total_cmp(b));
    let max_speed = speeds
        .get((speeds.len() as f32 * 0.95) as usize)
        .copied()
        .unwrap_or(0.0001)
        .max(0.0001);

    let mut result = Vec::with_capacity(max_buckets);
    for bucket in 0..max_buckets {
        if best_cell[bucket] == u32::MAX {
            continue;
        }
        let norm = (best_speed[bucket] / max_speed).clamp(0.0, 1.0).sqrt();
        if norm < 0.05 {
            continue;
        }
        result.push((best_cell[bucket] as usize, norm));
    }
    result
}
