// src/domains/climate/solver/continentality.rs

use std::collections::VecDeque;
use rayon::prelude::*;

use super::super::{climate_config::ClimateConfig, climate_data::ClimateLayer};
use super::smoothstep;
use crate::{domains::tectonics::tectonics_data::GeologyLayer, world::topology::WorldTopology};

pub(super) fn compute_continentality(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &mut ClimateLayer,
    config: &ClimateConfig,
) {
    let num_cells = topology.cells.len();
    let min_ocean_cells = (num_cells as f32 * config.min_ocean_size_ratio) as usize;
    // Порог для суши: игнорируем мелкие острова (всё, что меньше 0.5% площади планеты)
    let min_land_cells = (num_cells as f32 * (config.min_ocean_size_ratio * 0.5)) as usize;

    let mut is_ocean = vec![false; num_cells];
    let mut is_major_land = vec![false; num_cells];
    let mut visited = vec![false; num_cells];

    // 1. Flood Fill: находим настоящие океаны (отсеиваем мелкие озера)
    // и настоящие континенты (отсеиваем мелкие океанические острова)
    for i in 0..num_cells {
        if visited[i] {
            continue;
        }

        let is_water = geology.elevations[i] <= 0.0;
        let mut current_body = Vec::new();
        let mut stack = vec![i];
        visited[i] = true;

        while let Some(cell_id) = stack.pop() {
            current_body.push(cell_id);
            for &neighbor_id in &topology.cells[cell_id].neighbors {
                let neighbor_is_water = geology.elevations[neighbor_id] <= 0.0;
                if neighbor_is_water == is_water && !visited[neighbor_id] {
                    visited[neighbor_id] = true;
                    stack.push(neighbor_id);
                }
            }
        }

        if is_water {
            if current_body.len() >= min_ocean_cells {
                for &cell_id in &current_body {
                    is_ocean[cell_id] = true;
                }
            }
        } else if current_body.len() >= min_land_cells {
            for &cell_id in &current_body {
                is_major_land[cell_id] = true;
            }
        }
    }

    // BFS 1: расстояние до океана (для всей суши и внутренних озер)
    let mut dist_to_ocean = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::new();
    for i in 0..num_cells {
        if is_ocean[i] {
            dist_to_ocean[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(cell_id) = queue.pop_front() {
        let current_dist = dist_to_ocean[cell_id];
        for &neighbor_id in &topology.cells[cell_id].neighbors {
            if dist_to_ocean[neighbor_id] > current_dist + 1 {
                dist_to_ocean[neighbor_id] = current_dist + 1;
                queue.push_back(neighbor_id);
            }
        }
    }

    // BFS 2: расстояние до КРУПНОЙ суши (для океана, игнорируя мелкие острова!)
    let mut dist_to_land = vec![u32::MAX; num_cells];
    queue.clear();
    for i in 0..num_cells {
        if is_major_land[i] {
            dist_to_land[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(cell_id) = queue.pop_front() {
        let current_dist = dist_to_land[cell_id];
        for &neighbor_id in &topology.cells[cell_id].neighbors {
            if dist_to_land[neighbor_id] > current_dist + 1 {
                dist_to_land[neighbor_id] = current_dist + 1;
                queue.push_back(neighbor_id);
            }
        }
    }

    let avg_hex_rad = (4.0 * std::f32::consts::PI / num_cells as f32).sqrt();

    // Знаковая континентальность: -1 = открытый океан, 0 = берег, +1 = глубь континента
    climate.continentality.par_iter_mut().enumerate().for_each(|(i, out)| {
        let is_water = geology.elevations[i] <= 0.0;
        *out = if is_water {
            let physical_dist = dist_to_land[i] as f32 * avg_hex_rad;
            let norm = (physical_dist / config.ocean_continentality_scale).min(1.0);
            -smoothstep(norm)
        } else {
            let physical_dist = dist_to_ocean[i] as f32 * avg_hex_rad;
            let norm = (physical_dist / config.land_continentality_scale).min(1.0);
            smoothstep(norm)
        };
    });
}
