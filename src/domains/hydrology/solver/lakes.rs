// src/domains/hydrology/solver/lakes.rs

use std::{cmp::Ordering, collections::BinaryHeap, collections::HashMap, f32::consts::PI};

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

/// Результаты расчёта котловин и озёр для передачи в трассировщик рек
pub(super) struct LakeRoutingData {
    /// Клетки-приёмники бессточных котловин (куда стекают реки во 2-м проходе наравне с океаном)
    pub endorheic_sinks: Vec<bool>,
    /// Испарение с поверхности клеточного зеркала для проточных озёр (вычитается из транзитного стока)
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

    // =========================================================================
    // 1. ПРОХОД 1 (Priority-Flood по чистому рельефу от Океана):
    // Находим все топографические впадины (где уровень перелива sill_elev выше дна)
    // =========================================================================
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

    // Фоллбэк для полностью сухопутной планеты без океанов
    if heap.is_empty() {
        if let Some((min_id, &min_h)) = geology
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
    }

    let min_depth = config.min_lake_depth.max(0.0005);

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
                    // Клетка `v` лежит ниже уровня перелива `u_sill` — это чаша потенциального озера!
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

    // =========================================================================
    // 2. СЧИТАЕМ СУММАРНЫЙ ПРИТОК ВОДЫ В КАЖДУЮ ВПАДИНУ
    // =========================================================================
    let mut pass1_flow = vec![0.0_f32; num_cells];
    for i in 0..num_cells {
        if !geology.is_ocean_cell(i) {
            pass1_flow[i] = hydrology.runoff[i] * cell_area;
        }
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

    // Группируем ячейки впадин по их точке перелива `outlet`
    let mut basins: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..num_cells {
        let outlet = basin_outlet[i];
        if outlet != usize::MAX {
            basins.entry(outlet).or_default().push(i);
        }
    }

    // =========================================================================
    // 3. ВОДНЫЙ БАЛАНС КОТЛОВИН: ПРОТОЧНЫЕ ПРЕСНЫЕ И БЕССТОЧНЫЕ СОЛЁНЫЕ ОЗЁРА
    // =========================================================================
    for (outlet, mut b_cells) in basins {
        if b_cells.is_empty() {
            continue;
        }

        // Весь приток бассейна впадины проходит через корневые клетки впадины, впадающие прямо в `outlet`
        let mut total_inflow = 0.0_f32;
        for &c in &b_cells {
            if let Some(k) = pass1_dir[c] {
                if topology.cells[c].neighbors[k as usize] == outlet {
                    total_inflow += pass1_flow[c];
                }
            }
        }

        // Сортируем клетки чаши от самого глубокого дна вверх к кромке перелива
        b_cells.sort_by(|&a, &b| geology.elevations[a].total_cmp(&geology.elevations[b]));
        let deepest_cell = b_cells[0];
        let rim_sill_elev = sill_elev[deepest_cell];

        // Оцениваем испарение каждой клетки, если она станет открытым водным зеркалом озера
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

            let pet_lake = ((t_mean + 11.0).max(0.8)
                * 0.020
                * config.aridity_evap_scale
                * config.lake_evaporation_scale)
                .max(0.012);
            // Чистые потери с зеркала озера (испарение минус прямые дожди на воду)
            let net_loss = (pet_lake - p_annual * 0.85).max(pet_lake * 0.16) * cell_area;
            cell_evap.push(net_loss);
            full_basin_evap += net_loss;
        }

        if total_inflow >= full_basin_evap && total_inflow >= config.river_threshold * 0.28 {
            // СЛУЧАЙ А: ПРОТОЧНОЕ ПРЕСНОЕ ОЗЕРО (Exorheic, тип Байкала / Великих озёр / Виктории)
            // Впадина заполняется до самого порога перелива `rim_sill_elev` и сбрасывает избыток рекой!
            let lake_len = b_cells.len() as u32;
            for (idx, &c) in b_cells.iter().enumerate() {
                hydrology.lake_mask[c] = true;
                hydrology.is_salt_lake[c] = false;
                hydrology.lake_depth[c] = (rim_sill_elev - geology.elevations[c]).max(min_depth);
                hydrology.lake_size[c] = lake_len;
                lake_cell_evap_loss[c] = cell_evap[idx];

                // Озёрные донные отложения (пресноводный сапропель и ил)
                hydrology.soils[c] = SoilType::Gleysol;
                hydrology.soil_texture[c] = [0.15, 0.45, 0.40];
                hydrology.water_capacity[c] = 1.0;
                hydrology.permeability[c] = 0.01;
            }
        } else {
            // СЛУЧАЙ Б: БЕССТОЧНАЯ КОТЛОВИНА (Endorheic, тип Каспия / Арала / Балхаша / Такла-Макан)
            // Воды НЕ хватает, чтобы перелиться через `outlet`! Заливаем только самые глубокие клетки на дне.
            let mut accum_evap = 0.0_f32;
            let mut flooded_count = 0usize;

            if total_inflow >= config.river_threshold * 0.35 {
                for &loss in &cell_evap {
                    if accum_evap + loss <= total_inflow * 1.25 || flooded_count == 0 {
                        accum_evap += loss;
                        flooded_count += 1;
                    } else {
                        break;
                    }
                }
            }

            if flooded_count > 0 {
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
                // Сухая бессточная впадина в пустыне (солончак / такыр): вода испаряется на дне
                endorheic_sinks[deepest_cell] = true;
            }
        }
    }

    LakeRoutingData {
        endorheic_sinks,
        lake_cell_evap_loss,
    }
}