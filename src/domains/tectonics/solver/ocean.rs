// src/domains/tectonics/solver/ocean.rs

use super::{TectonicsGeometryCache, smoothstep};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use rayon::prelude::*;
use std::collections::VecDeque;

pub fn compute_oceans_and_continentality(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    geology.ensure_aux_buffers(num_cells);

    let min_ocean_cells = (num_cells as f32 * config.min_ocean_size_ratio).max(1.0) as usize;
    let min_land_cells = (num_cells as f32 * (config.min_ocean_size_ratio * 0.5)).max(1.0) as usize;

    geology.is_ocean.fill(false);
    geology.continentality.resize(num_cells, 0.0);

    let mut visited = vec![false; num_cells];
    let mut is_major_land = vec![false; num_cells];

    // 1. Flood Fill по CSR-кэшу `cache.edge_neighbors` только по связным областям с elev <= 0.0:
    // Проверяем площадь бассейна и наличие хотя бы одной глубоководной (абиссальной) клетки
    for i in 0..num_cells {
        if visited[i] || geology.elevations[i] > 0.0 {
            continue;
        }

        let mut current_body = Vec::new();
        let mut min_elev_in_body = geology.elevations[i];
        let mut stack = vec![i];
        visited[i] = true;

        while let Some(cell_id) = stack.pop() {
            current_body.push(cell_id);
            let h = geology.elevations[cell_id];
            if h < min_elev_in_body {
                min_elev_in_body = h;
            }

            let start = cache.edge_offsets[cell_id];
            let end = cache.edge_offsets[cell_id + 1];
            for e in start..end {
                let neighbor_id = cache.edge_neighbors[e];
                if geology.elevations[neighbor_id] <= 0.0 && !visited[neighbor_id] {
                    visited[neighbor_id] = true;
                    stack.push(neighbor_id);
                }
            }
        }

        let is_true_ocean = current_body.len() >= min_ocean_cells
            && min_elev_in_body <= config.ocean_abyssal_threshold;

        if is_true_ocean {
            for cell_id in current_body {
                geology.is_ocean[cell_id] = true;
            }
        } else {
            // Запертый внутри материка остаточный бассейн (тип Каспия / Тарима / Паннонского моря):
            // аккумулирует мощную толщу озёрно-аллювиальных осадков
            for cell_id in current_body {
                geology.is_ocean[cell_id] = false;
                let h = geology.elevations[cell_id];
                if h < config.trapped_basin_floor_elev {
                    let infill = config.trapped_basin_floor_elev - h;
                    geology.sediment_thickness[cell_id] =
                        (geology.sediment_thickness[cell_id] + infill * 14.0).min(4.5);
                    geology.sediment_supply[cell_id] =
                        (geology.sediment_supply[cell_id] + infill * 8.0).min(3.0);
                    geology.crust_density[cell_id] = geology.crust_density[cell_id].min(2760.0);
                    geology.elevations[cell_id] = config.trapped_basin_floor_elev - infill * 0.12;
                }
            }
        }
    }

    // 2. Крупные массивы суши (включая не-океанические низменности с elev <= 0.0)
    visited.fill(false);
    is_major_land.fill(false);
    for i in 0..num_cells {
        if visited[i] || geology.is_ocean[i] {
            continue;
        }
        let mut land_body = Vec::new();
        let mut stack = vec![i];
        visited[i] = true;
        while let Some(cid) = stack.pop() {
            land_body.push(cid);
            let start = cache.edge_offsets[cid];
            let end = cache.edge_offsets[cid + 1];
            for e in start..end {
                let nbr = cache.edge_neighbors[e];
                if !geology.is_ocean[nbr] && !visited[nbr] {
                    visited[nbr] = true;
                    stack.push(nbr);
                }
            }
        }
        if land_body.len() >= min_land_cells {
            for &cid in &land_body {
                is_major_land[cid] = true;
            }
        }
    }

    // 3. BFS 1: физическое угловое расстояние до настоящего Океана
    let mut dist_to_ocean_steps = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::with_capacity(num_cells / 2);
    for i in 0..num_cells {
        if geology.is_ocean[i] {
            dist_to_ocean_steps[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(cell_id) = queue.pop_front() {
        let next_dist = dist_to_ocean_steps[cell_id].saturating_add(1);
        let start = cache.edge_offsets[cell_id];
        let end = cache.edge_offsets[cell_id + 1];
        for e in start..end {
            let neighbor_id = cache.edge_neighbors[e];
            if dist_to_ocean_steps[neighbor_id] > next_dist {
                dist_to_ocean_steps[neighbor_id] = next_dist;
                queue.push_back(neighbor_id);
            }
        }
    }

    // 4. BFS 2: физическое угловое расстояние до крупного материка (игнорируя мелкие острова)
    let mut dist_to_land_steps = vec![u32::MAX; num_cells];
    queue.clear();
    for i in 0..num_cells {
        if is_major_land[i] {
            dist_to_land_steps[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(cell_id) = queue.pop_front() {
        let next_dist = dist_to_land_steps[cell_id].saturating_add(1);
        let start = cache.edge_offsets[cell_id];
        let end = cache.edge_offsets[cell_id + 1];
        for e in start..end {
            let neighbor_id = cache.edge_neighbors[e];
            if dist_to_land_steps[neighbor_id] > next_dist {
                dist_to_land_steps[neighbor_id] = next_dist;
                queue.push_back(neighbor_id);
            }
        }
    }

    let avg_hex_rad = cache.avg_cell_rad;
    let is_ocean = &geology.is_ocean;
    let ocean_scale = config.ocean_continentality_scale.max(0.01);
    let land_scale = config.land_continentality_scale.max(0.01);

    // 5. Знаковая континентальность [-1.0 .. +1.0] в параллельном проходе
    geology
        .continentality
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, c)| {
            *c = if is_ocean[i] {
                let physical_dist = dist_to_land_steps[i] as f32 * avg_hex_rad;
                let norm = (physical_dist / ocean_scale).min(1.0);
                -smoothstep(norm)
            } else {
                let physical_dist = dist_to_ocean_steps[i] as f32 * avg_hex_rad;
                let norm = (physical_dist / land_scale).min(1.0);
                smoothstep(norm)
            };
        });
}