// src/domains/hydrology/solver/soils.rs

use super::SurfaceNoise;
use crate::{
    domains::{
        climate::climate_data::ClimateLayer,
        hydrology::{
            hydrology_config::HydrologyConfig,
            hydrology_data::{HydrologyLayer, SoilType},
        },
        tectonics::tectonics_data::{GeologyLayer, RockType},
    },
    world::topology::WorldTopology,
};

pub(super) fn compute_soils(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    noise: &SurfaceNoise,
) {
    use rayon::prelude::*;

    let num_cells = topology.cells.len();
    let mut soils = std::mem::take(&mut hydrology.soils);
    let mut soil_texture = std::mem::take(&mut hydrology.soil_texture);
    let mut soil_depth = std::mem::take(&mut hydrology.soil_depth);
    let mut water_capacity = std::mem::take(&mut hydrology.water_capacity);
    let mut permeability = std::mem::take(&mut hydrology.permeability);

    soils.resize(num_cells, SoilType::Ocean);
    soil_texture.resize(num_cells, [0.33, 0.34, 0.33]);
    soil_depth.resize(num_cells, 0.0);
    water_capacity.resize(num_cells, 0.0);
    permeability.resize(num_cells, 0.0);

    let elevations = &geology.elevations;
    let lithology = &geology.lithology;
    let terrain_rug = &geology.terrain_ruggedness;
    let volcano_mask = &geology.volcano_mask;
    let is_ocean = &geology.is_ocean;
    let t_spring = &climate.spring.temperatures;
    let t_summer = &climate.summer.temperatures;
    let t_winter = &climate.winter.temperatures;
    let p_spring = &climate.spring.precipitation;
    let p_summer = &climate.summer.precipitation;
    let p_winter = &climate.winter.precipitation;
    let temp_jitter = &noise.temp_jitter;
    let moisture_jitter = &noise.moisture_jitter;

    let aridity_evap_scale = config.aridity_evap_scale;
    let chemical_weathering_rate = config.chemical_weathering_rate;
    let slope_erosion_sensitivity = config.slope_erosion_sensitivity;
    let permafrost_temp_threshold = config.permafrost_temp_threshold;
    let mountain_slope_threshold = config.mountain_slope_threshold;

    soils
        .par_iter_mut()
        .zip(soil_texture.par_iter_mut())
        .zip(soil_depth.par_iter_mut())
        .zip(water_capacity.par_iter_mut())
        .zip(permeability.par_iter_mut())
        .enumerate()
        .for_each(
            |(i, ((((soil_out, tex_out), depth_out), whc_out), perm_out))| {
                if is_ocean
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| elevations.get(i).copied().unwrap_or(0.0) <= 0.0)
                {
                    *soil_out = SoilType::Ocean;
                    *tex_out = [0.0, 0.3, 0.7];
                    *depth_out = 0.0;
                    *whc_out = 1.0;
                    *perm_out = 0.0;
                    return;
                }

                let elev = elevations[i];
                let mean_temp = (2.0 * t_spring[i] + t_summer[i] + t_winter[i]) * 0.25;
                let warm_season_temp = t_summer[i].max(t_winter[i]);
                let annual_precip = (2.0 * p_spring[i] + p_summer[i] + p_winter[i]) * 0.25;
                let pet = ((mean_temp + 10.0).max(2.0) * 0.022 * aridity_evap_scale).max(0.05);
                let moisture_index = annual_precip / pet;

                let t_jit = temp_jitter[i] * 1.4;
                let m_jit = moisture_jitter[i];
                let eff_mean_temp = mean_temp + t_jit;
                let eff_warm_temp = warm_season_temp + t_jit * 0.8;
                let eff_moisture_index = (moisture_index * (1.0 + m_jit * 0.14)).max(0.0);

                let neighbors = &topology.cells[i].neighbors;
                let mut max_slope = 0.0_f32;
                let mut neighbor_elev_sum = 0.0_f32;
                for &n_id in neighbors {
                    let n_elev = elevations[n_id];
                    neighbor_elev_sum += n_elev;
                    let diff = (elev - n_elev).abs();
                    if diff > max_slope {
                        max_slope = diff;
                    }
                }
                let avg_n_elev = if !neighbors.is_empty() {
                    neighbor_elev_sum / neighbors.len() as f32
                } else {
                    elev
                };
                let valley_concavity = (avg_n_elev - elev).clamp(-0.05, 0.05);

                let rugged = terrain_rug.get(i).copied().unwrap_or(0.0);
                let effective_slope = max_slope.max(rugged * 0.055);

                let rock = lithology[i];
                let [mut sand, mut silt, mut clay] = rock.base_texture();

                let heat_factor = ((eff_mean_temp - 5.0) / 25.0).clamp(0.0, 1.2);
                let wet_factor = (eff_moisture_index * 0.6).clamp(0.0, 1.2);
                let chem_weathering = heat_factor * wet_factor * chemical_weathering_rate;
                let frost_weathering = ((5.0 - eff_mean_temp) / 25.0).clamp(0.0, 0.45);
                let slope_wash = (effective_slope * slope_erosion_sensitivity).clamp(0.0, 0.75);
                let basin_deposition = (valley_concavity * 12.0).clamp(0.0, 0.35);

                sand =
                    (sand - chem_weathering * 0.35 + slope_wash * 0.45 + frost_weathering * 0.10
                        - m_jit * 0.03)
                        .max(0.05);
                silt = (silt + frost_weathering * 0.25 + basin_deposition * 0.20
                    - chem_weathering * 0.15
                    - slope_wash * 0.20)
                    .max(0.05);
                clay = (clay + chem_weathering * 0.50 + basin_deposition * 0.25
                    - slope_wash * 0.25
                    + m_jit * 0.03)
                    .max(0.05);

                let tex_sum = sand + silt + clay;
                sand /= tex_sum;
                silt /= tex_sum;
                clay /= tex_sum;
                *tex_out = [sand, silt, clay];

                let climate_depth_mod =
                    (0.45 + 0.55 * (wet_factor * 0.7 + heat_factor * 0.3)).clamp(0.3, 1.0);
                let slope_depth_factor =
                    (1.0 - effective_slope * slope_erosion_sensitivity * 0.85).clamp(0.08, 1.0);
                let depth = (slope_depth_factor * climate_depth_mod).clamp(0.08, 1.0);
                *depth_out = depth;

                let is_permafrost =
                    eff_mean_temp < permafrost_temp_threshold || eff_warm_temp < 3.0;
                let volcanic_bonus = if volcano_mask[i] || rock == RockType::Andesite {
                    1.25
                } else {
                    1.0
                };
                let specific_whc = (0.12 * sand + 0.52 * silt + 0.38 * clay) * volcanic_bonus;
                *whc_out = (specific_whc * (0.25 + 0.75 * depth)).clamp(0.05, 1.0);

                let raw_perm = sand * 0.82 + silt * 0.24 + clay * 0.04 + rock.karst_permeability();
                *perm_out = if is_permafrost {
                    0.03
                } else {
                    (raw_perm * (0.4 + 0.6 * depth)).clamp(0.02, 1.0)
                };

                *soil_out = classify_soil_type(
                    eff_mean_temp,
                    eff_warm_temp,
                    eff_moisture_index,
                    effective_slope,
                    elev,
                    depth,
                    sand,
                    clay,
                    rock,
                    volcano_mask[i],
                    permafrost_temp_threshold,
                    mountain_slope_threshold,
                );
            },
        );

    hydrology.soils = soils;
    hydrology.soil_texture = soil_texture;
    hydrology.soil_depth = soil_depth;
    hydrology.water_capacity = water_capacity;
    hydrology.permeability = permeability;
}

#[allow(clippy::too_many_arguments)]
fn classify_soil_type(
    mean_temp: f32,
    warm_season_temp: f32,
    moisture_index: f32,
    max_slope: f32,
    elev: f32,
    depth: f32,
    sand: f32,
    clay: f32,
    rock: RockType,
    is_hotspot: bool,
    permafrost_temp_threshold: f32,
    mountain_slope_threshold: f32,
) -> SoilType {
    if max_slope > mountain_slope_threshold || depth < 0.18 {
        return SoilType::Leptosol;
    }
    if mean_temp < permafrost_temp_threshold || warm_season_temp < 4.0 {
        return SoilType::Gelisol;
    }
    if (is_hotspot || rock == RockType::Andesite) && moisture_index > 0.28 {
        return SoilType::Andisol;
    }
    if moisture_index < 0.24 {
        return SoilType::Aridisol;
    }
    if elev < 0.08 && max_slope < 0.008 && moisture_index > 0.85 && clay > 0.34 {
        return SoilType::Gleysol;
    }
    if mean_temp > 19.5 && moisture_index > 0.72 {
        return SoilType::Laterite;
    }
    if (1.5..=19.0).contains(&mean_temp) && (0.24..=0.64).contains(&moisture_index) {
        return SoilType::Chernozem;
    }
    if mean_temp < 9.5 && moisture_index >= 0.55
        || (mean_temp < 13.5
            && sand > 0.48
            && matches!(rock, RockType::Granite | RockType::Sandstone))
    {
        return SoilType::Podzol;
    }
    SoilType::Cambisol
}
