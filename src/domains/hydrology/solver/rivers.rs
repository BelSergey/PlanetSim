// src/domains/hydrology/solver/rivers.rs

use std::{collections::BinaryHeap, f32::consts::PI};

use super::{
    SurfaceNoise,
    lakes::{FloodNode, LakeRoutingData},
};
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

/// 1. Вычисляет локальный поверхностный сток (Runoff) для всех не-океанических клеток
/// и возвращает поле меандрирования `meander_bias`.
pub(super) fn compute_runoff(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    noise: &SurfaceNoise,
) -> Vec<f32> {
    let num_cells = topology.cells.len();
    let mut meander_bias = vec![0.0_f32; num_cells];

    for i in 0..num_cells {
        let elev = geology.elevations[i];
        if geology.is_ocean_cell(i) {
            hydrology.runoff[i] = 0.0;
            hydrology.flow_dir[i] = None;
            hydrology.flow_accumulation[i] = 0.0;
            continue;
        }

        let mut max_slope = 0.0_f32;
        for &n_id in &topology.cells[i].neighbors {
            let diff = (elev - geology.elevations[n_id]).abs();
            if diff > max_slope {
                max_slope = diff;
            }
        }
        let flat_factor = (1.0 - max_slope / 0.018).clamp(0.0, 1.0);
        meander_bias[i] = noise.meander_field[i] * 0.0075 * flat_factor;

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
        let pet = ((mean_temp + 10.0).max(0.5) * 0.016 * config.aridity_evap_scale).max(0.02);

        // В настоящих пустынях (где осадки ничтожны по сравнению с испаряемостью PET)
        // вся дождевая вода мгновенно уходит в песок и испаряется — поверхностный сток равен 0.0!
        if annual_precip < 0.045 && snowmelt_bonus < 0.01 {
            hydrology.runoff[i] = 0.0;
            continue;
        }

        let aridity_index = (pet / annual_precip.max(1e-4)) * (0.55 + 0.65 * whc);
        let aet_ratio = aridity_index / (1.0 + aridity_index * aridity_index).sqrt();
        // Убрали нижний порог 0.05! При высоком aridity_index сток плавно падает в чистый 0.0
        let runoff_fraction = ((1.0 - aet_ratio) - 0.12).clamp(0.0, 0.90);
        let net_rain_runoff = annual_precip * runoff_fraction;

        let karst_loss = geology.lithology[i].karst_permeability() * 0.25;
        let raw_runoff = (net_rain_runoff + snowmelt_bonus) * (1.0 - karst_loss);
        hydrology.runoff[i] = (raw_runoff * config.runoff_multiplier).max(0.0);
    }

    meander_bias
}

/// 2. Финальная трассировка рек (с учётом океана, бессточных озёр и перелива проточных озёр)
pub(super) fn route_rivers_with_lakes(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    meander_bias: &[f32],
    lake_routing: &LakeRoutingData,
) {
    let num_cells = topology.cells.len();
    if num_cells == 0 {
        return;
    }

    let cell_area = 4.0 * PI / (num_cells as f32);

    let mut visited = vec![false; num_cells];
    let mut filled_elev = vec![0.0_f32; num_cells];
    let mut heap = BinaryHeap::with_capacity(num_cells / 4);
    let mut downstream_order = Vec::with_capacity(num_cells);

    // Базисом эрозии служат как клетки Океана, так и бессточные озёра (чтобы реки в бессточных
    // котловинах текли вниз к солёному озеру со всех склонов!)
    for i in 0..num_cells {
        if geology.is_ocean_cell(i) || lake_routing.endorheic_sinks[i] {
            let h = geology.elevations[i];
            visited[i] = true;
            filled_elev[i] = h;
            hydrology.flow_dir[i] = None;
            heap.push(FloodNode {
                elev: h,
                cell_id: i,
            });
        }
    }

    while let Some(FloodNode { cell_id: u, .. }) = heap.pop() {
        let is_terminal = geology.is_ocean_cell(u) || lake_routing.endorheic_sinks[u];
        if !is_terminal {
            downstream_order.push(u);

            // Если клетка не внутри проточного озера, выбираем самый крутой спуск среди соседей
            if !hydrology.is_lake_cell(u) {
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
        }

        let u_filled = filled_elev[u];
        for &v in &topology.cells[u].neighbors {
            if !visited[v] {
                visited[v] = true;

                let k_back = topology.cells[v]
                    .neighbors
                    .iter()
                    .position(|&id| id == u)
                    .unwrap_or(0) as u8;
                hydrology.flow_dir[v] = Some(k_back);

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

    // Аккумуляция потока от водоразделов вниз к озёрам и океану
    for i in 0..num_cells {
        hydrology.flow_accumulation[i] = if !geology.is_ocean_cell(i) {
            hydrology.runoff[i] * cell_area
        } else {
            0.0
        };
    }

    for &u in downstream_order.iter().rev() {
        // В клетках проточных озёр вычитаем испарение с водного зеркала перед передачей в устье
        let evap_loss = lake_routing.lake_cell_evap_loss[u];
        let flow = (hydrology.flow_accumulation[u] - evap_loss).max(0.0);
        hydrology.flow_accumulation[u] = flow;

        if let Some(k) = hydrology.flow_dir[u] {
            let target = topology.cells[u].neighbors[k as usize];
            if !geology.is_ocean_cell(target) {
                hydrology.flow_accumulation[target] += flow;
            }
        }
    }
}

/// 3. Расчёт влажности почвы (от осадков, рек и фильтрации берегов озёр)
pub(super) fn compute_soil_moisture(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
) {
    let num_cells = topology.cells.len();
    let threshold = config.river_threshold.max(1e-6);

    for i in 0..num_cells {
        if geology.is_ocean_cell(i) || hydrology.is_lake_cell(i) {
            hydrology.soil_moisture[i] = 1.0;
            continue;
        }

        let elev = geology.elevations[i];
        let t_spring = climate.spring.temperatures[i];
        let t_summer = climate.summer.temperatures[i];
        let t_winter = climate.winter.temperatures[i];
        let mean_temp = (2.0 * t_spring + t_summer + t_winter) * 0.25;

        let p_spring = climate.spring.precipitation[i];
        let p_summer = climate.summer.precipitation[i];
        let p_winter = climate.winter.precipitation[i];
        let annual_precip = (2.0 * p_spring + p_summer + p_winter) * 0.25;

        let whc = hydrology.water_capacity[i];
        let [sand, _silt, clay] = hydrology.soil_texture[i];
        let pet = ((mean_temp + 10.0).max(1.5) * 0.013 * config.aridity_evap_scale).max(0.025);

        let retention_mod = 1.0 + (whc - 0.30) * config.soil_biome_influence
            - (sand - 0.45).max(0.0) * 0.35 * config.soil_biome_influence;
        let effective_infiltration = (annual_precip * retention_mod.clamp(0.5, 1.50))
            * (1.0 - geology.lithology[i].karst_permeability() * 0.35);

        let rain_soil_moisture = effective_infiltration / (effective_infiltration + pet * 0.7);

        // Прямое увлажнение почвы рекой в самой клетке русла
        let my_flow = hydrology.flow_accumulation[i];
        let river_ratio = (my_flow / threshold).max(0.0);
        let direct_river_moisture = if river_ratio > 0.25 {
            (0.18 * river_ratio.powf(0.42) * config.river_groundwater_seepage).clamp(0.0, 0.78)
        } else {
            0.0
        };

        // Латеральная фильтрация грунтовых вод от соседних рек и берегов озёр
        let mut valley_seepage = 0.0_f32;
        let mut max_slope = 0.0_f32;
        for &n_id in &topology.cells[i].neighbors {
            let n_elev = geology.elevations[n_id];
            let diff = (elev - n_elev).abs();
            if diff > max_slope {
                max_slope = diff;
            }

            if hydrology.is_lake_cell(n_id) {
                // Берега пресных озёр получают сильную грунтовую подпитку, берега солёных — умеренную
                let lake_seep = if hydrology.is_salt_lake[n_id] {
                    0.22
                } else {
                    0.42
                } * config.river_groundwater_seepage;
                valley_seepage = valley_seepage.max(lake_seep.clamp(0.0, 0.55));
            } else if !geology.is_ocean_cell(n_id) {
                let n_flow_ratio = (hydrology.flow_accumulation[n_id] / threshold).max(0.0);
                let height_above_river = (elev - n_elev).max(0.0);
                if n_flow_ratio >= 0.8 && height_above_river < 0.022 {
                    let elev_atten = 1.0 - (height_above_river / 0.022);
                    let perm_factor = 0.45 + 0.55 * hydrology.permeability[i];
                    let seep = 0.14
                        * n_flow_ratio.powf(0.38)
                        * elev_atten
                        * perm_factor
                        * config.river_groundwater_seepage;
                    valley_seepage = valley_seepage.max(seep.clamp(0.0, 0.48));
                }
            }
        }

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

        hydrology.soil_moisture[i] = total_sm;

        if total_sm > 0.72
            && elev < 0.15
            && max_slope < 0.012
            && matches!(
                hydrology.soils[i],
                SoilType::Aridisol | SoilType::Cambisol | SoilType::Chernozem
            )
        {
            hydrology.soils[i] = SoilType::Gleysol;
        }
    }
}