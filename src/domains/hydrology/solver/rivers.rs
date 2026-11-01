// src/domains/hydrology/solver/rivers.rs

use std::{cmp::Ordering, collections::BinaryHeap, f32::consts::PI};

use rayon::prelude::*;

use super::SurfaceNoise;
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
struct FloodNode {
    elev: f32,
    cell_id: usize,
}

impl Eq for FloodNode {}

// Min-Heap: первыми извлекаются клетки с наименьшей высотой
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

pub(super) fn compute_rivers(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    noise: &SurfaceNoise,
) {
    let num_cells = topology.cells.len();
    if num_cells == 0 {
        return;
    }

    // Физическая площадь одной ячейки на единичной сфере (обеспечивает масштабонезависимость)
    let cell_area = 4.0 * PI / (num_cells as f32);

    // Предрасчет локального смещения меандрирования: работает только на пологих равнинах,
    // не искажая горные хребты и береговую линию
    let mut meander_bias = vec![0.0_f32; num_cells];

    // =========================================================================
    // 1. РАСЧЕТ ПОЛЯ СТОКА (Runoff = Осадки - Эвапотранспирация Будыко + Таяние снега)
    // =========================================================================
    hydrology
        .runoff
        .par_iter_mut()
        .zip(hydrology.flow_dir.par_iter_mut())
        .zip(hydrology.flow_accumulation.par_iter_mut())
        .zip(meander_bias.par_iter_mut())
        .enumerate()
        .for_each(
            |(i, (((runoff_out, flow_dir_out), flow_acc_out), meander_out))| {
                let elev = geology.elevations[i];
                if elev <= 0.0 {
                    *runoff_out = 0.0;
                    *flow_dir_out = None;
                    *flow_acc_out = 0.0;
                    *meander_out = 0.0;
                    return;
                }

                let mut max_slope = 0.0_f32;
                for &n_id in &topology.cells[i].neighbors {
                    let diff = (elev - geology.elevations[n_id]).abs();
                    if diff > max_slope {
                        max_slope = diff;
                    }
                }
                let flat_factor = (1.0 - max_slope / 0.018).clamp(0.0, 1.0);
                *meander_out = noise.meander_field[i] * 0.0075 * flat_factor;

                let t_spring = climate.spring.temperatures[i];
                let t_summer = climate.summer.temperatures[i];
                let t_winter = climate.winter.temperatures[i];
                let mean_temp = (2.0 * t_spring + t_summer + t_winter) * 0.25;

                let p_spring = climate.spring.precipitation[i];
                let p_summer = climate.summer.precipitation[i];
                let p_winter = climate.winter.precipitation[i];
                let annual_precip = (2.0 * p_spring + p_summer + p_winter) * 0.25;

                let cold_temp = t_winter.min(t_spring);
                let warm_temp = t_summer.max(t_spring);
                let cold_precip = if t_winter < t_summer {
                    p_winter
                } else {
                    p_summer
                };
                let snow_share = ClimateLayer::snow_fraction(cold_temp);
                let melt_factor = ((warm_temp - 0.0) / 8.0).clamp(0.0, 1.0);
                let snowmelt_bonus = cold_precip * snow_share * melt_factor * 0.30;

                let whc = hydrology.water_capacity[i];
                let pet =
                    ((mean_temp + 10.0).max(0.5) * 0.016 * config.aridity_evap_scale).max(0.02);
                let aridity_index = (pet / annual_precip.max(1e-4)) * (0.55 + 0.65 * whc);
                let aet_ratio = aridity_index / (1.0 + aridity_index * aridity_index).sqrt();
                let net_rain_runoff = annual_precip * (1.0 - aet_ratio).clamp(0.05, 0.95);
                let karst_loss = geology.lithology[i].karst_permeability() * 0.25;
                let raw_runoff = (net_rain_runoff + snowmelt_bonus) * (1.0 - karst_loss);
                *runoff_out = (raw_runoff * config.runoff_multiplier).max(0.0);
            },
        );

    // =========================================================================
    // 2. ТРАССИРОВКА НАПРАВЛЕНИЙ СТОКА (Priority-Flood + Эродируемость + Меандры)
    // =========================================================================
    let mut visited = vec![false; num_cells];
    let mut filled_elev = vec![0.0_f32; num_cells];
    let mut heap = BinaryHeap::with_capacity(num_cells / 4);
    let mut downstream_order = Vec::with_capacity(num_cells);

    // Инициализируем океан как базовый уровень стока
    for i in 0..num_cells {
        let h = geology.elevations[i];
        if h <= 0.0 {
            visited[i] = true;
            filled_elev[i] = h;
            heap.push(FloodNode {
                elev: h,
                cell_id: i,
            });
        }
    }

    // Обходим планету от береговой линии вверх к водоразделам:
    // мягкие породы и отрицательные фазы meander_bias притягивают русла рек на равнинах.
    while let Some(FloodNode { cell_id: u, .. }) = heap.pop() {
        if geology.elevations[u] > 0.0 {
            downstream_order.push(u);

            // Если клетка не в замкнутой яме, уточняем направление по самому крутому уклону
            // среди уже открытых более низких соседей (с учетом пород и меандрирования)
            let my_filled = filled_elev[u];
            let neighbors = &topology.cells[u].neighbors;
            let mut best_k = hydrology.flow_dir[u];
            let mut max_drop = -1.0_f32;

            for (k, &n_id) in neighbors.iter().enumerate() {
                if visited[n_id] && filled_elev[n_id] < my_filled {
                    let rock_bonus =
                        geology.lithology[n_id].erodibility() * config.lithology_erosion_bias;
                    let meander_pull = -meander_bias[n_id];
                    let drop = (my_filled - filled_elev[n_id]) + rock_bonus + meander_pull;
                    if drop > max_drop {
                        max_drop = drop;
                        best_k = Some(k as u8);
                    }
                }
            }
            hydrology.flow_dir[u] = best_k;
        }

        let u_filled = filled_elev[u];
        for &v in &topology.cells[u].neighbors {
            if !visited[v] {
                visited[v] = true;

                // Находим индекс грани `k_back`, ведущей из `v` обратно в `u`
                let k_back = topology.cells[v]
                    .neighbors
                    .iter()
                    .position(|&id| id == u)
                    .unwrap_or(0) as u8;
                hydrology.flow_dir[v] = Some(k_back);

                // Эффективная высота с учетом податливости породы размыву и меандров на равнине
                let erod_offset =
                    geology.lithology[v].erodibility() * config.lithology_erosion_bias;
                let raw_v_elev = geology.elevations[v] - erod_offset + meander_bias[v];
                let v_filled = raw_v_elev.max(u_filled + 1e-5);
                filled_elev[v] = v_filled;

                heap.push(FloodNode {
                    elev: v_filled,
                    cell_id: v,
                });
            }
        }
    }

    // =========================================================================
    // 3. АККУМУЛЯЦИЯ ПОТОКА (от вершин к океану)
    // =========================================================================
    let runoff = &hydrology.runoff;
    hydrology
        .flow_accumulation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, out)| {
            *out = if geology.elevations[i] > 0.0 {
                runoff[i] * cell_area
            } else {
                0.0
            };
        });

    for &u in downstream_order.iter().rev() {
        let flow = hydrology.flow_accumulation[u];
        if let Some(k) = hydrology.flow_dir[u] {
            let target = topology.cells[u].neighbors[k as usize];
            if geology.elevations[target] > 0.0 {
                hydrology.flow_accumulation[target] += flow;
            }
        }
    }

    // =========================================================================
    // 4. РАСЧЕТ ВЛАЖНОСТИ ПОЧВЫ (Soil Moisture = Осадки в почве + Подпитка от рек)
    // =========================================================================
    let threshold = config.river_threshold.max(1e-6);

    let flow_accumulation = &hydrology.flow_accumulation;
    let water_capacity = &hydrology.water_capacity;
    let soil_texture = &hydrology.soil_texture;
    let permeability = &hydrology.permeability;

    hydrology
        .soil_moisture
        .par_iter_mut()
        .zip(hydrology.soils.par_iter_mut())
        .enumerate()
        .for_each(|(i, (soil_moisture_out, soil_out))| {
            let elev = geology.elevations[i];
            if elev <= 0.0 {
                *soil_moisture_out = 1.0;
                return;
            }

            let t_spring = climate.spring.temperatures[i];
            let t_summer = climate.summer.temperatures[i];
            let t_winter = climate.winter.temperatures[i];
            let mean_temp = (2.0 * t_spring + t_summer + t_winter) * 0.25;

            let p_spring = climate.spring.precipitation[i];
            let p_summer = climate.summer.precipitation[i];
            let p_winter = climate.winter.precipitation[i];
            let annual_precip = (2.0 * p_spring + p_summer + p_winter) * 0.25;

            // 1. Климатическая влага, удержанная почвой (зависит от осадков, испаряемости PET и влагоемкости WHC)
            let whc = water_capacity[i];
            let [sand, _silt, clay] = soil_texture[i];
            let pet = ((mean_temp + 10.0).max(1.5) * 0.013 * config.aridity_evap_scale).max(0.025);

            let retention_mod = 1.0 + (whc - 0.30) * config.soil_biome_influence
                - (sand - 0.45).max(0.0) * 0.35 * config.soil_biome_influence;
            let effective_infiltration = (annual_precip * retention_mod.clamp(0.5, 1.50))
                * (1.0 - geology.lithology[i].karst_permeability() * 0.35);

            let rain_soil_moisture = effective_infiltration / (effective_infiltration + pet * 0.7);

            // 2. Прямое увлажнение почвы рекой в самой клетке русла
            let my_flow = flow_accumulation[i];
            let river_ratio = (my_flow / threshold).max(0.0);
            let direct_river_moisture = if river_ratio > 0.25 {
                (0.18 * river_ratio.powf(0.42) * config.river_groundwater_seepage).clamp(0.0, 0.78)
            } else {
                0.0
            };

            // 3. Латеральная фильтрация грунтовых вод от рек в соседних клетках плоской долины
            let mut valley_seepage = 0.0_f32;
            let mut max_slope = 0.0_f32;
            for &n_id in &topology.cells[i].neighbors {
                let n_elev = geology.elevations[n_id];
                let diff = (elev - n_elev).abs();
                if diff > max_slope {
                    max_slope = diff;
                }

                if n_elev > 0.0 {
                    let n_flow_ratio = (flow_accumulation[n_id] / threshold).max(0.0);
                    let height_above_river = (elev - n_elev).max(0.0);
                    if n_flow_ratio >= 0.8 && height_above_river < 0.022 {
                        let elev_atten = 1.0 - (height_above_river / 0.022);
                        let perm_factor = 0.45 + 0.55 * permeability[i];
                        let seep = 0.14
                            * n_flow_ratio.powf(0.38)
                            * elev_atten
                            * perm_factor
                            * config.river_groundwater_seepage;
                        valley_seepage = valley_seepage.max(seep.clamp(0.0, 0.48));
                    }
                }
            }

            // 4. Застой влаги в плоских глинистых низинах
            let lowland_bonus = if max_slope < 0.010 && clay > 0.35 && rain_soil_moisture > 0.35 {
                0.12 * (1.0 - max_slope / 0.010)
            } else {
                0.0
            };

            let total_sm = (rain_soil_moisture
                + direct_river_moisture * (1.0 - rain_soil_moisture * 0.55)
                + valley_seepage * (1.0 - rain_soil_moisture * 0.65)
                + lowland_bonus)
                .clamp(0.02, 1.0);

            *soil_moisture_out = total_sm;

            if total_sm > 0.72
                && elev < 0.15
                && max_slope < 0.012
                && matches!(
                    *soil_out,
                    SoilType::Aridisol | SoilType::Cambisol | SoilType::Chernozem
                )
            {
                *soil_out = SoilType::Gleysol;
            }
        });
}
