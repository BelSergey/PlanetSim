// src/domains/hydrology/solver/lakes.rs

use std::{cmp::Ordering, collections::BinaryHeap, f32::consts::PI};

use crate::{
    domains::{
        climate::climate_data::ClimateLayer,
        hydrology::{
            hydrology_config::HydrologyConfig,
            hydrology_data::{HydrologyLayer, SoilType},
        },
        tectonics::tectonics_data::GeologyLayer,
    },
    world::topology::WorldTopology,
};

#[derive(Copy, Clone, PartialEq)]
pub(super) struct FloodNode {
    pub elev: f32,
    pub cell_id: usize,
}

impl Eq for FloodNode {}

impl Ord for FloodNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .elev
            .total_cmp(&self.elev)
            .then_with(|| self.cell_id.cmp(&other.cell_id))
    }
}

impl PartialOrd for FloodNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) struct LakeRoutingData {
    pub endorheic_sinks: Vec<bool>,
    pub lake_cell_evap_loss: Vec<f32>,
}

pub(super) fn compute_lakes_and_basins(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
) -> LakeRoutingData {
    let num_cells = topology.cells.len();
    let cell_area = 4.0 * PI / (num_cells.max(1) as f32);

    let mut endorheic_sinks = vec![false; num_cells];
    let mut lake_cell_evap_loss = vec![0.0_f32; num_cells];

    if num_cells == 0 {
        return LakeRoutingData {
            endorheic_sinks,
            lake_cell_evap_loss,
        };
    }

    // 1. ПРОХОД 1 (Priority-Flood от Океана)
    let mut visited = vec![false; num_cells];
    let mut sill_elev = vec![0.0_f32; num_cells];
    let mut pass1_dir: Vec<Option<u8>> = vec![None; num_cells];
    let mut basin_outlet = vec![usize::MAX; num_cells];
    let mut pass1_order = Vec::with_capacity(num_cells);
    let mut heap = BinaryHeap::with_capacity(num_cells / 4);

    for i in 0..num_cells {
        if geology.is_ocean_cell(i) {
            visited[i] = true;
            sill_elev[i] = geology.elevations[i];
            heap.push(FloodNode {
                elev: geology.elevations[i],
                cell_id: i,
            });
        }
    }

    if heap.is_empty()
        && let Some((min_id, &min_h)) = geology
            .elevations
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
    {
        visited[min_id] = true;
        sill_elev[min_id] = min_h;
        endorheic_sinks[min_id] = true;
        heap.push(FloodNode {
            elev: min_h,
            cell_id: min_id,
        });
    }

    let min_depth = config.min_lake_depth.max(0.002);

    while let Some(FloodNode { cell_id: u, .. }) = heap.pop() {
        if !geology.is_ocean_cell(u) {
            pass1_order.push(u);
        }

        let u_sill = sill_elev[u];
        let u_basin = basin_outlet[u];

        for &v in &topology.cells[u].neighbors {
            if !visited[v] {
                visited[v] = true;

                let k_back = topology.cells[v]
                    .neighbors
                    .iter()
                    .position(|&id| id == u)
                    .unwrap_or(0) as u8;
                pass1_dir[v] = Some(k_back);

                let v_raw = geology.elevations[v];
                if v_raw + min_depth < u_sill {
                    sill_elev[v] = u_sill;
                    basin_outlet[v] = if u_basin != usize::MAX { u_basin } else { u };
                } else {
                    sill_elev[v] = v_raw.max(u_sill + 1e-6);
                    basin_outlet[v] = usize::MAX;
                }

                heap.push(FloodNode {
                    elev: sill_elev[v],
                    cell_id: v,
                });
            }
        }
    }

    // 2. СЧИТАЕМ ПРИТОК ВОДЫ В КАЖДУЮ КОТЛОВИНУ
    let mut pass1_flow = vec![0.0_f32; num_cells];
    let is_ocean = &geology.is_ocean;
    let runoff = &hydrology.runoff;
    for (i, flow) in pass1_flow.iter_mut().enumerate().take(num_cells) {
        *flow = if !is_ocean[i] {
            runoff[i] * cell_area
        } else {
            0.0
        };
    }

    for &u in pass1_order.iter().rev() {
        let f = pass1_flow[u];
        if let Some(k) = pass1_dir[u] {
            let target = topology.cells[u].neighbors[k as usize];
            if !geology.is_ocean_cell(target) {
                pass1_flow[target] += f;
            }
        }
    }

    let mut basins: Vec<Vec<usize>> = vec![Vec::new(); num_cells];
    for (i, &outlet) in basin_outlet.iter().enumerate().take(num_cells) {
        if outlet != usize::MAX {
            basins[outlet].push(i);
        }
    }

    // Максимальный размер одного озера (~0.35% клеток планеты), чтобы не затапливать материки
    let max_fresh_lake_cells = ((num_cells as f32 * 0.0022) as usize).clamp(4, 180);
    let max_salt_lake_cells = ((num_cells as f32 * 0.0038) as usize).clamp(6, 320);
    // Порог эрозионного прорыва (реки прорезают мелкие неровности рельефа глубиной < 0.024)
    let river_incision_depth = (config.min_lake_depth * 9.5).max(0.022);

    // 3. ВОДНЫЙ БАЛАНС И ЭРОЗИОННЫЙ ПРОРЫВ ПОРОГОВ (SILL BREACHING)
    for (outlet, mut b_cells) in basins.into_iter().enumerate() {
        if b_cells.is_empty() {
            continue;
        }

        let mut total_inflow = 0.0_f32;
        for &c in &b_cells {
            if let Some(k) = pass1_dir[c]
                && topology.cells[c].neighbors[k as usize] == outlet
            {
                total_inflow += pass1_flow[c];
            }
        }

        b_cells.sort_by(|&a, &b| geology.elevations[a].total_cmp(&geology.elevations[b]));
        let deepest_cell = b_cells[0];
        let bottom_elev = geology.elevations[deepest_cell];
        let rim_sill_elev = sill_elev[deepest_cell];
        let max_basin_depth = rim_sill_elev - bottom_elev;

        // Если впадина мелкая (обычная волна шума на равнине), река прорезает порог и спускает её целиком!
        if max_basin_depth < river_incision_depth && bottom_elev > -0.005 {
            continue;
        }

        // Если в котловину почти не течет вода (меньше порога ручья), озеро не образуется вообще!
        if total_inflow < config.river_threshold * 1.15 {
            // В совсем сухой глубокой замкнутой яме ставим точку сухого стока (солончак)
            if max_basin_depth >= river_incision_depth * 1.5 && total_inflow < 1e-6 {
                endorheic_sinks[deepest_cell] = true;
            }
            continue;
        }

        let mut cell_evap = Vec::with_capacity(b_cells.len());
        let mut full_basin_evap = 0.0_f32;

        for &c in &b_cells {
            let t_mean = (2.0 * climate.spring.temperatures[c]
                + climate.summer.temperatures[c]
                + climate.winter.temperatures[c])
                * 0.25;
            let p_annual = (2.0 * climate.spring.precipitation[c]
                + climate.summer.precipitation[c]
                + climate.winter.precipitation[c])
                * 0.25;

            let pet_lake = ((t_mean + 12.0).max(1.0)
                * 0.025
                * config.aridity_evap_scale
                * config.lake_evaporation_scale)
                .max(0.018);
            let net_loss = (pet_lake - p_annual * 0.75).max(pet_lake * 0.25) * cell_area;
            cell_evap.push(net_loss);
            full_basin_evap += net_loss;
        }

        if total_inflow >= full_basin_evap {
            // СЛУЧАЙ А: ПРОТОЧНОЕ ПРЕСНОЕ ОЗЕРО (вытекающая река прорезает порог перелива на глубину incision,
            // поэтому озеро заполняет только глубокое ядро долины!)
            let incised_sill =
                rim_sill_elev - (max_basin_depth * 0.62).min(river_incision_depth * 1.6);
            let flooded_cells: Vec<usize> = b_cells
                .iter()
                .copied()
                .filter(|&c| geology.elevations[c] < incised_sill)
                .take(max_fresh_lake_cells)
                .collect();

            if flooded_cells.len() >= 2 {
                let lake_len = flooded_cells.len() as u32;
                let water_level = geology.elevations[*flooded_cells.last().unwrap()];
                for (idx, &c) in flooded_cells.iter().enumerate() {
                    hydrology.lake_mask[c] = true;
                    hydrology.is_salt_lake[c] = false;
                    hydrology.lake_depth[c] = (water_level - geology.elevations[c]).max(min_depth);
                    hydrology.lake_size[c] = lake_len;
                    lake_cell_evap_loss[c] = cell_evap[idx];

                    hydrology.soils[c] = SoilType::Gleysol;
                    hydrology.soil_texture[c] = [0.15, 0.45, 0.40];
                    hydrology.water_capacity[c] = 1.0;
                    hydrology.permeability[c] = 0.01;
                }
            }
        } else {
            // СЛУЧАЙ Б: БЕССТОЧНОЕ СОЛЁНОЕ ОЗЕРО (заливает только ту площадь дна, которую может прокормить приток!)
            let mut accum_evap = 0.0_f32;
            let mut flooded_count = 0usize;

            for &loss in &cell_evap {
                if accum_evap + loss <= total_inflow && flooded_count < max_salt_lake_cells {
                    accum_evap += loss;
                    flooded_count += 1;
                } else {
                    break;
                }
            }

            if flooded_count >= 2 {
                let water_level = geology.elevations[b_cells[flooded_count - 1]];
                let lake_len = flooded_count as u32;
                for &c in &b_cells[..flooded_count] {
                    hydrology.lake_mask[c] = true;
                    hydrology.is_salt_lake[c] = true;
                    hydrology.lake_depth[c] =
                        (water_level - geology.elevations[c]).max(min_depth * 0.5);
                    hydrology.lake_size[c] = lake_len;
                    endorheic_sinks[c] = true;

                    hydrology.soils[c] = SoilType::Gleysol;
                    hydrology.soil_texture[c] = [0.20, 0.40, 0.40];
                    hydrology.water_capacity[c] = 1.0;
                    hydrology.permeability[c] = 0.01;
                }
            } else {
                // Приток слишком мал для озера — оставляем сухое дно впадины (солончак)
                endorheic_sinks[deepest_cell] = true;
            }
        }
    }

    LakeRoutingData {
        endorheic_sinks,
        lake_cell_evap_loss,
    }
}
