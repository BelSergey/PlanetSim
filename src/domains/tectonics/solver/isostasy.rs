// src/domains/tectonics/solver/isostasy.rs

use super::{smoothstep, TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use rayon::prelude::*;

/// Пассивный гипсометрический профиль от центра кратона (c = 1.0) через шельф к океану (c = 0.0)
#[inline]
fn passive_hypsometric_curve(c: f32, cont_base: f32, ocean_base: f32) -> f32 {
    let shelf_edge_h = (-0.065_f32).max(ocean_base * 0.35);
    if c >= 0.55 {
        let t = smoothstep((c - 0.55) / 0.45);
        t * cont_base
    } else if c >= 0.28 {
        let t = smoothstep((c - 0.28) / 0.27);
        shelf_edge_h * (1.0 - t)
    } else {
        let t = smoothstep(c / 0.28);
        ocean_base * (1.0 - t) + shelf_edge_h * t
    }
}

pub fn update_isostasy_and_erosion(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    // Калибруем влияние плотностей коры и мантии из конфига (закон Архимеда / Эйри)
    let rho_m = config.mantle_density.max(3000.0);
    let cont_buoyancy_ratio =
        ((rho_m - config.cont_crust_density) / (3300.0 - 2700.0)).clamp(0.4, 2.0);
    let ocean_sink_ratio =
        ((config.old_ocean_density - config.young_ocean_density) / (3130.0 - 2890.0))
            .clamp(0.3, 2.2);

    let eff_cont_base = config.cont_base_height * cont_buoyancy_ratio;
    let eff_ocean_base = config.ocean_base_height;

    {
        let plates = &geology.plates;
        let plate_ids = &geology.plate_ids;
        let age = &geology.crust_age;
        let stress = &geology.tectonic_stress;
        let shifts = &geology.subcell_shift;
        let elevations = &mut geology.elevations;

        elevations
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, elev_out)| {
                let p_id = plate_ids[i];
                let plate_offset = plates
                    .get(p_id)
                    .map(|p| p.base_elevation_offset)
                    .unwrap_or(0.0);

                let boundary_elev_delta = shifts[i][0];
                let c1 = shifts[i][1];

                let boundary_fade = 1.0 - stress[i].abs().clamp(0.0, 1.0);
                let eff_plate_offset = plate_offset * boundary_fade * 0.5;

                // 1. Базовая высота из пассивного профиля (Кратон -> Шельф -> Склон -> Океан)
                let mut h =
                    passive_hypsometric_curve(c1, eff_cont_base, eff_ocean_base) + eff_plate_offset;

                // 2. Остывание и проседание старой океанической коры (закон Парсонса — Склейтера)
                if c1 < 0.28 {
                    let ocean_weight = 1.0 - (c1 / 0.28);
                    let age_norm = (age[i] / config.oceanic_mature_age.max(0.1)).clamp(0.0, 1.0);
                    let cooling_subsidence =
                        (age_norm.sqrt() - 0.5) * 0.10 * ocean_sink_ratio * ocean_weight;
                    h -= cooling_subsidence;
                }

                // 3. Тектоническая деформация границ (горы, дуги, желоба, рифты, СОХ)
                h += boundary_elev_delta;

                *elev_out = h.max(config.elevation_min_clamp);
            });
    }

    // Пошаговая термическая эрозия (без нижнего зажима 0.01, чтобы ползунок конфига работал с 0.0)
    let blend = (config.step_erosion_rate * dt * 8.0).clamp(0.0, 0.15);
    if blend > 1e-5 {
        scratch.temp_elevations.copy_from_slice(&geology.elevations);
        let old_elev = &scratch.temp_elevations;

        geology
            .elevations
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, elev_out)| {
                let start = cache.edge_offsets[i];
                let end = cache.edge_offsets[i + 1];
                if start == end {
                    return;
                }

                let my_h = old_elev[i];
                let mut avg_nbr = 0.0_f32;
                for e in start..end {
                    avg_nbr += old_elev[cache.edge_neighbors[e]];
                }
                avg_nbr /= (end - start) as f32;

                *elev_out = my_h * (1.0 - blend) + avg_nbr * blend;
            });
    }

    let _ = topology;
}

pub fn step_elevation_diffusion(
    topology: &WorldTopology,
    elevations: &mut [f32],
    strength: f32,
    slope_erosion_factor: f32,
    max_strength: f32,
) {
    let old_elevations = elevations.to_vec();

    elevations.par_iter_mut().enumerate().for_each(|(i, elev)| {
        let cell = &topology.cells[i];
        if cell.neighbors.is_empty() {
            return;
        }
        let neighbor_sum: f32 = cell
            .neighbors
            .iter()
            .map(|&n_id| old_elevations[n_id])
            .sum();
        let avg_neighbor = neighbor_sum / cell.neighbors.len() as f32;

        let mut max_slope = 0.0_f32;
        for &n_id in &cell.neighbors {
            let diff = (old_elevations[i] - old_elevations[n_id]).abs();
            if diff > max_slope {
                max_slope = diff;
            }
        }

        let local_strength = (strength + max_slope * slope_erosion_factor).min(max_strength);
        *elev = old_elevations[i] * (1.0 - local_strength) + avg_neighbor * local_strength;
    });
}