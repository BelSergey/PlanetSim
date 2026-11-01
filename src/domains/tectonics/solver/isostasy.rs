// src/domains/tectonics/solver/isostasy.rs

use super::{TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use rayon::prelude::*;

pub fn update_isostasy_and_erosion(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let rho_mantle = config.mantle_density.max(3100.0);
    let rho_sediment = 2520.0_f32;

    let root_cont = config.cont_crust_thickness_km * (1.0 - config.cont_crust_density / rho_mantle);
    let root_ocean =
        config.ocean_crust_thickness_km * (1.0 - (config.old_ocean_density - 40.0) / rho_mantle);

    let root_span = (root_cont - root_ocean).max(1.0);
    let elev_span = config.cont_base_height - config.ocean_base_height;
    let isostatic_scale = elev_span / root_span;
    let sea_level_root = root_ocean - config.ocean_base_height / isostatic_scale;

    {
        let plates = &geology.plates;
        let plate_ids = &geology.plate_ids;
        let thickness = &geology.crust_thickness;
        let density = &geology.crust_density;
        let sediment = &geology.sediment_thickness;
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

                let crust_root = thickness[i] * (1.0 - density[i] / rho_mantle);
                let sed_root = sediment[i] * (1.0 - rho_sediment / rho_mantle) * 0.35;
                let total_root = crust_root + sed_root;

                let h = (total_root - sea_level_root) * isostatic_scale + plate_offset;
                *elev_out = h.max(config.elevation_min_clamp);
            });
    }

    // Мягкая термическая релаксация склонов на шаге симуляции
    scratch.temp_elevations.copy_from_slice(&geology.elevations);
    let old_elev = &scratch.temp_elevations;
    let ref_rad = cache.avg_cell_rad;

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
            let mut max_slope = 0.0_f32;

            for e in start..end {
                let nbr = cache.edge_neighbors[e];
                let nh = old_elev[nbr];
                avg_nbr += nh;
                let grad = ((nh - my_h).abs() / cache.edge_dists_rad[e]) * ref_rad;
                if grad > max_slope {
                    max_slope = grad;
                }
            }
            avg_nbr /= (end - start) as f32;

            let blend = ((config.step_erosion_rate + max_slope * 0.8) * dt * 2.5).clamp(0.02, 0.32);
            *elev_out = my_h * (1.0 - blend) + avg_nbr * blend;
        });

    let _ = topology;
}

/// Клеточный автомат для диффузии (размытия) высот (параллельно по клеткам)
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