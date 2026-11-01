// src/domains/tectonics/solver/ocean.rs

use super::{smoothstep, TectonicsGeometryCache};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use std::collections::VecDeque;

pub fn compute_oceans_and_continentality(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let min_ocean_cells = (num_cells as f32 * config.min_ocean_size_ratio).max(1.0) as usize;
    let min_land_cells = (num_cells as f32 * (config.min_ocean_size_ratio * 0.5)).max(1.0) as usize;

    geology.is_ocean.fill(false);
    geology.continentality.resize(num_cells, 0.0);

    let mut visited = vec![false; num_cells];
    let mut is_major_land = vec![false; num_cells];

    // 1. Flood Fill только по связным водным/низменным областям с elev <= 0.0:
    // Проверяем и площадь бассейна, и наличие хотя бы одной глубоководной (абиссальной) клетки
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

            for &neighbor_id in &topology.cells[cell_id].neighbors {
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
            // Запертый внутри материка остаточный бассейн (заполняется осадками снизу до trapped_basin_floor_elev без инверсии краёв)
            for cell_id in current_body {
                geology.is_ocean[cell_id] = false;
                let h = geology.elevations[cell_id];
                if h < config.trapped_basin_floor_elev {
                    let infill = config.trapped_basin_floor_elev - h;
                    geology.sediment_thickness[cell_id] += infill * 14.0;
                    geology.crust_density[cell_id] =
                        geology.crust_density[cell_id].min(2760.0);
                    geology.elevations[cell_id] =
                        config.trapped_basin_floor_elev - infill * 0.12;
                }
            }
        }
    }

    // 2. Вся суша (включая не-океанические низменности с elev <= 0.0) также учитывается в проверке крупных массивов суши
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
            for &nbr in &topology.cells[cid].neighbors {
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

    // 3. BFS 1: физическое угловое расстояние до настоящего Океана (в радианах)
    let mut dist_to_ocean_steps = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::new();
    for i in 0..num_cells {
        if geology.is_ocean[i] {
            dist_to_ocean_steps[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(cell_id) = queue.pop_front() {
        let current_dist = dist_to_ocean_steps[cell_id];
        for &neighbor_id in &topology.cells[cell_id].neighbors {
            if dist_to_ocean_steps[neighbor_id] > current_dist + 1 {
                dist_to_ocean_steps[neighbor_id] = current_dist + 1;
                queue.push_back(neighbor_id);
            }
        }
    }

    // 4. BFS 2: физическое угловое расстояние до крупного материка (игнорируя мелкие острова!)
    let mut dist_to_land_steps = vec![u32::MAX; num_cells];
    queue.clear();
    for i in 0..num_cells {
        if is_major_land[i] {
            dist_to_land_steps[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(cell_id) = queue.pop_front() {
        let current_dist = dist_to_land_steps[cell_id];
        for &neighbor_id in &topology.cells[cell_id].neighbors {
            if dist_to_land_steps[neighbor_id] > current_dist + 1 {
                dist_to_land_steps[neighbor_id] = current_dist + 1;
                queue.push_back(neighbor_id);
            }
        }
    }

    let avg_hex_rad = cache.avg_cell_rad;

    // 5. Знаковая континентальность [-1.0 .. +1.0]
    for i in 0..num_cells {
        geology.continentality[i] = if geology.is_ocean[i] {
            let physical_dist = dist_to_land_steps[i] as f32 * avg_hex_rad;
            let norm = (physical_dist / config.ocean_continentality_scale.max(0.01)).min(1.0);
            -smoothstep(norm)
        } else {
            let physical_dist = dist_to_ocean_steps[i] as f32 * avg_hex_rad;
            let norm = (physical_dist / config.land_continentality_scale.max(0.01)).min(1.0);
            smoothstep(norm)
        };
    }
}