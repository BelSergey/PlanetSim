// src/domains/tectonics/solver/isostasy.rs

use super::{TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use rayon::prelude::*;

pub fn update_isostasy_and_erosion(
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let rho_mantle = config.mantle_density.max(3100.0);
    let rho_sediment = 2520.0_f32;

    // 1. Калибруем закон изостазии Эйри так, чтобы нормальная континентальная кора (35 км, 2700 кг/м³)
    // ложилась ровно на `config.cont_base_height`, а зрелая океаническая (7 км, 3080 кг/м³) — на `config.ocean_base_height`
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
        let stress = &geology.tectonic_stress;
        let orogeny = &geology.orogeny_root;
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
                let sed_root = sediment[i] * (1.0 - rho_sediment / rho_mantle);
                let total_root = crust_root + sed_root;

                let mut h = (total_root - sea_level_root) * isostatic_scale + plate_offset;

                // Динамический прогиб глубоководного желоба (если stress[i] < -0.5 — сигнал субдукции)
                if stress[i] < -0.5 {
                    let trench_intensity = (-stress[i] - 0.5).clamp(0.0, 1.5);
                    h -= trench_intensity * config.trench_depth_amp;
                }

                // Дополнительный складчатый подъем молодых орогенов
                h += orogeny[i] * 0.16;

                *elev_out = h.max(config.elevation_min_clamp);
            });
    }

    // 2. Термическая эрозия крутых склонов и перенос осадков в низины и на шельфы
    scratch.temp_elevations.copy_from_slice(&geology.elevations);
    scratch.next_thickness.copy_from_slice(&geology.crust_thickness);
    scratch
        .next_sediment
        .copy_from_slice(&geology.sediment_thickness);

    let old_elev = &scratch.temp_elevations;
    let old_thick = &scratch.next_thickness;
    let old_sed = &scratch.next_sediment;
    let ref_rad = cache.avg_cell_rad;

    let elevations = &mut geology.elevations;
    let thickness = &mut geology.crust_thickness;
    let sediment = &mut geology.sediment_thickness;

    elevations
        .par_iter_mut()
        .zip(thickness.par_iter_mut())
        .zip(sediment.par_iter_mut())
        .enumerate()
        .for_each(|(i, ((elev_out, thick_out), sed_out))| {
            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            if start == end {
                return;
            }

            let my_h = old_elev[i];
            let mut net_flux = 0.0_f32;

            for e in start..end {
                let nbr = cache.edge_neighbors[e];
                let step_rad = cache.edge_dists_rad[e];
                // Масштабонезависимый угловой уклон
                let dh = old_elev[nbr] - my_h;
                let angular_slope = (dh.abs() / step_rad) * ref_rad;
                let k = (config.step_erosion_rate * (1.0 + angular_slope * config.slope_erosion_factor))
                    .min(0.45);
                net_flux += dh * k;
            }

            net_flux /= (end - start) as f32;
            let delta_h = net_flux * dt * 4.0;
            *elev_out = (my_h + delta_h).max(config.elevation_min_clamp);

            if delta_h < 0.0 {
                // Клетка теряет высоту (эрозия вершины) -> стачивается кора
                let eroded_km = (-delta_h / isostatic_scale.max(0.01)) * 0.65;
                *thick_out = (old_thick[i] - eroded_km).max(config.ocean_crust_thickness_km);
            } else if delta_h > 0.0 {
                // Клетка принимает снесенный материал (аккумуляция осадков в предгорьях и на шельфе)
                let deposited_km =
                    (delta_h / isostatic_scale.max(0.01)) * config.sediment_deposition_efficiency;
                *sed_out = (old_sed[i] + deposited_km).min(9.5);
            }
        });
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