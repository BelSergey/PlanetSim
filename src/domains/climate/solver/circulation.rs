// src/domains/climate/solver/circulation.rs

use bevy::prelude::*;
use rayon::prelude::*;

use super::super::{climate_config::ClimateConfig, climate_data::SeasonalSnapshot};
use super::{SeasonScratch, SolverPrecalc, smoothstep};
use crate::{domains::tectonics::tectonics_data::GeologyLayer, world::topology::WorldTopology};

/// Поворот касательного вектора вокруг единичной нормали `axis` на угол (cos, sin).
/// Эквивалент `Quat::from_axis_angle(axis, θ) * v`, но без тригонометрии и кватерниона.
#[inline]
fn rodrigues(axis: Vec3, v: Vec3, cos_a: f32, sin_a: f32) -> Vec3 {
    v * cos_a + axis.cross(v) * sin_a
}

pub(super) fn compute_season_pressure_and_winds(
    season_mult: f32,
    topology: &WorldTopology,
    geology: &GeologyLayer,
    continentality: &[f32],
    config: &ClimateConfig,
    precalc: &SolverPrecalc,
    scratch: &mut SeasonScratch,
    snap: &mut SeasonalSnapshot,
) {
    let num_cells = topology.cells.len();
    let avg_hex_rad = precalc.avg_hex_rad;

    // 1. Термическое поле (считается заново каждую итерацию)
    for (i, c) in continentality.iter().enumerate().take(num_cells) {
        let sea_level_temp = snap.temperatures[i] + precalc.mountain_cooling[i];
        let ocean_ref_temp = precalc.ocean_ref_temp[i];
        let temp_diff = (sea_level_temp - ocean_ref_temp) / 12.0;

        let weight = if *c >= 0.0 {
            0.4 + 0.6 * c
        } else {
            0.4 * (1.0 + c * 0.6)
        };
        scratch.thermal_field[i] = (-temp_diff * weight).clamp(-1.2, 1.2);
    }

    // Сглаживание термического поля (параллельно)
    for _ in 0..10 {
        std::mem::swap(&mut scratch.thermal_field, &mut scratch.prev_thermal);
        let prev = &scratch.prev_thermal;
        let out = &mut scratch.thermal_field;
        out.par_iter_mut().enumerate().for_each(|(i, o)| {
            let nbrs = &topology.cells[i].neighbors;
            if nbrs.is_empty() {
                *o = prev[i];
                return;
            }
            let mut sum = 0.0;
            for &n in nbrs {
                sum += prev[n];
            }
            *o = prev[i] * 0.3 + (sum / nbrs.len() as f32) * 0.7;
        });
    }

    // 2. Давление = Широтные пояса (из scratch, посчитаны 1 раз на сезон) + Термика
    for i in 0..num_cells {
        snap.pressure[i] = scratch.latitudinal_pressure[i]
            + scratch.thermal_field[i] * config.thermal_pressure_weight;
    }

    // Сглаживание давления (параллельно)
    for _ in 0..8 {
        std::mem::swap(&mut snap.pressure, &mut scratch.prev_pressure);
        let prev = &scratch.prev_pressure;
        let out = &mut snap.pressure;
        out.par_iter_mut().enumerate().for_each(|(i, o)| {
            let nbrs = &topology.cells[i].neighbors;
            if nbrs.is_empty() {
                *o = prev[i];
                return;
            }
            let mut sum = 0.0;
            for &n in nbrs {
                sum += prev[n];
            }
            *o = prev[i] * 0.3 + (sum / nbrs.len() as f32) * 0.7;
        });
    }

    // 3. Ветра — градиент давления + Кориолис через Rodrigues
    let offs = &precalc.neighbor_offsets;
    let tdirs = &precalc.tangent_dirs_flat;

    for i in 0..num_cells {
        let cell = &topology.cells[i];
        let pos = cell.center;
        let my_elev = (geology.elevations[i] - config.mountain_base_level).max(0.0);
        let east = precalc.east[i];
        let north = precalc.north[i];

        let my_p = snap.pressure[i];
        let mut grad_3d = Vec3::ZERO;

        let start = offs[i];
        let end = offs[i + 1];
        for (j, tdir) in tdirs.iter().enumerate().take(end).skip(start) {
            let k = j - start;
            let n_id = cell.neighbors[k];
            let n_pos = topology.cells[n_id].center;
            let raw_dir = n_pos - pos;
            let safe_dist = raw_dir.length().max(avg_hex_rad * 0.3);
            let diff = (my_p - snap.pressure[n_id]) / safe_dist;
            grad_3d += tdir * diff;
        }

        if !cell.neighbors.is_empty() {
            grad_3d = (grad_3d / cell.neighbors.len() as f32) * 0.22;
        }

        // Кориолис (Rodrigues, cos/sin предвычислены)
        let mut wind_3d = rodrigues(
            pos,
            grad_3d,
            precalc.coriolis_cos[i],
            precalc.coriolis_sin[i],
        );

        // Синоптические волны в струях ветра (сильнее в циклонах my_p < 0 и умеренных широтах)
        let cyclone_instability = (0.45 - my_p * 0.55).clamp(0.15, 1.25);
        let seasonal_wave = precalc.synoptic_noise_a[i] * (1.0 - 0.35 * season_mult.abs())
            + precalc.synoptic_noise_b[i] * (0.60 * season_mult);
        let wave_angle = seasonal_wave * 0.32 * cyclone_instability;
        let (w_sin, w_cos) = wave_angle.sin_cos();
        wind_3d = rodrigues(pos, wind_3d, w_cos, w_sin);

        // Орографический барьер
        let mut mountain_drag = 0.0;
        let mut upslope_grad = Vec3::ZERO;
        let wind_dir = wind_3d.normalize_or_zero();

        for (j, tdir) in tdirs.iter().enumerate().take(end).skip(start) {
            let k = j - start;
            let n_id = cell.neighbors[k];
            let to_neighbor = tdir;
            let n_elev = (geology.elevations[n_id] - config.mountain_base_level).max(0.0);
            let elev_diff = n_elev - my_elev;

            if elev_diff > 0.0 {
                // 1. Вертикальный масштаб: конвертируем абстрактные единицы высоты в физические
                // Если пик 0.5 — это ~8 км, то коэффициент ~0.016.
                // (Подберите 0.01 - 0.03, чтобы горы выглядели физически реально в ветрах)
                let vertical_scale = 0.02;

                // 2. Физический склон: высота / горизонтальная дистанция в тех же единицах (радианах)
                let real_height_diff = elev_diff * vertical_scale;
                let slope = (real_height_diff / avg_hex_rad).clamp(0.0, 2.0);

                upslope_grad += to_neighbor * slope;

                let alignment = wind_dir.dot(*to_neighbor);
                if alignment > 0.0 {
                    mountain_drag += slope * alignment;
                }
            }
        }

        if upslope_grad.length_squared() > 1e-6 {
            let slope_normal = upslope_grad.normalize();
            let into_wall = wind_3d.dot(slope_normal).max(0.0);
            let deflect_strength = (upslope_grad.length() * 0.35).clamp(0.0, 0.75);
            wind_3d -= slope_normal * (into_wall * deflect_strength);
        }

        let plateau_roughness = my_elev * 2.5;
        let total_drag = mountain_drag + plateau_roughness;
        let barrier_factor = 1.0 / (1.0 + total_drag * config.mountain_wind_barrier * 0.15);
        wind_3d = wind_3d * barrier_factor * config.wind_speed_multiplier;

        snap.winds_3d[i] = wind_3d;
        snap.winds[i] = Vec2::new(wind_3d.dot(east), wind_3d.dot(north));
    }
}

pub(super) fn compute_ocean_currents(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    continentality: &[f32],
    config: &ClimateConfig,
    precalc: &SolverPrecalc,
    scratch: &mut SeasonScratch,
    snap: &mut SeasonalSnapshot,
) {
    let num_cells = topology.cells.len();
    let avg_hex_rad = precalc.avg_hex_rad;
    let offs = &precalc.neighbor_offsets;
    let tdirs = &precalc.tangent_dirs_flat;

    for i in 0..num_cells {
        if geology.elevations[i] > 0.0 {
            snap.ocean_currents[i] = Vec3::ZERO;
            continue;
        }

        let cell = &topology.cells[i];
        let pos = cell.center;

        // 1. Ветровое течение
        let mut wind_current = snap.winds_3d[i] * config.ocean_current_speed_multiplier;
        if snap.temperatures[i] <= 0.0 {
            wind_current *= 0.1;
        }

        // 2. Спираль Экмана (Rodrigues)
        wind_current = rodrigues(
            pos,
            wind_current,
            precalc.ekman_cos[i],
            precalc.ekman_sin[i],
        );

        // 3. Геострофическое течение (угол ±π/2 → cos=0, sin=±1, только cross)
        let my_p = snap.pressure[i];
        let mut grad_p = Vec3::ZERO;
        let start = offs[i];
        let end = offs[i + 1];
        for (j, tdir) in tdirs.iter().enumerate().take(end).skip(start) {
            let k = j - start;
            let n_id = cell.neighbors[k];
            if geology.elevations[n_id] <= 0.0 {
                let n_pos = topology.cells[n_id].center;
                let raw_dir = n_pos - pos;
                let safe_dist = raw_dir.length().max(avg_hex_rad * 0.3);
                let diff = (my_p - snap.pressure[n_id]) / safe_dist;
                grad_p += tdir * diff;
            }
        }
        if !cell.neighbors.is_empty() {
            grad_p = (grad_p / cell.neighbors.len() as f32) * 0.5;
        }

        let s = precalc.sin_lat[i];
        let sign = s / (s.abs() + 0.03);
        let geostrophic_current = grad_p.cross(pos) * sign * config.geostrophic_multiplier;

        let mut current_3d = wind_current * 0.4 + geostrophic_current * 0.6;

        // 4. Граничные течения
        let mut coast_normal = Vec3::ZERO;
        for j in start..end {
            let k = j - start;
            let n_id = cell.neighbors[k];
            if geology.elevations[n_id] > 0.0 {
                let raw_dir = pos - topology.cells[n_id].center;
                let tangent_from_coast = (raw_dir - pos * raw_dir.dot(pos)).normalize_or_zero();
                coast_normal += tangent_from_coast;
            }
        }

        let orig_speed = current_3d.length();
        if coast_normal.length_squared() > 1e-6 && orig_speed > 1e-5 {
            coast_normal = coast_normal.normalize();
            let into_coast = current_3d.dot(-coast_normal);
            if into_coast > 0.0 {
                let mut along_coast = current_3d + coast_normal * into_coast;
                if along_coast.length_squared() > 1e-8 {
                    along_coast = along_coast.normalize() * orig_speed;
                }
                let east = precalc.east[i];
                let western_alignment = (-coast_normal).dot(east);
                let boost = config.boundary_current_multiplier
                    * (1.0 + 1.5 * smoothstep((western_alignment + 0.5) / 1.0));
                current_3d = along_coast * boost;
            }
        } else {
            let coastal_proximity = (1.0 - continentality[i].abs() * 3.0).clamp(0.0, 1.0);
            current_3d *=
                1.0 + (config.boundary_current_multiplier - 1.0) * 0.35 * coastal_proximity;
        }

        snap.ocean_currents[i] = current_3d;
    }

    // 5. Лёгкое сглаживание течений (параллельно)
    for _ in 0..2 {
        std::mem::swap(&mut snap.ocean_currents, &mut scratch.prev_currents);
        let prev = &scratch.prev_currents;
        let out = &mut snap.ocean_currents;
        out.par_iter_mut().enumerate().for_each(|(i, o)| {
            if geology.elevations[i] > 0.0 {
                *o = Vec3::ZERO;
                return;
            }
            let nbrs = &topology.cells[i].neighbors;
            if nbrs.is_empty() {
                *o = prev[i];
                return;
            }
            let mut sum = Vec3::ZERO;
            let mut cnt = 0.0_f32;
            for &n in nbrs {
                if geology.elevations[n] > 0.0 {
                    sum += prev[i];
                } else {
                    sum += prev[n];
                }
                cnt += 1.0;
            }
            *o = prev[i] * 0.5 + (sum / cnt) * 0.5;
        });
    }
}
