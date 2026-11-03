// src/domains/hydrology/solver/biomes.rs

use super::SurfaceNoise;
use crate::{
    domains::{
        climate::climate_data::ClimateLayer,
        hydrology::{
            hydrology_config::HydrologyConfig,
            hydrology_data::{Biome, HydrologyLayer, SoilType},
        },
        tectonics::tectonics_data::GeologyLayer,
    },
    world::topology::WorldTopology,
};

pub(super) fn compute_biomes(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    noise: &SurfaceNoise,
) {
    use rayon::prelude::*;

    let num_cells = topology.cells.len();
    let mut biomes = std::mem::take(&mut hydrology.biomes);
    biomes.resize(num_cells, Biome::DeepOcean);

    let is_ocean = &geology.is_ocean;
    let elevations = &geology.elevations;
    let t_spring = &climate.spring.temperatures;
    let t_summer = &climate.summer.temperatures;
    let t_winter = &climate.winter.temperatures;
    let p_spring = &climate.spring.precipitation;
    let p_summer = &climate.summer.precipitation;
    let p_winter = &climate.winter.precipitation;
    let temp_jitter = &noise.temp_jitter;
    let moisture_jitter = &noise.moisture_jitter;

    let lake_mask = &hydrology.lake_mask;
    let is_salt_lake = &hydrology.is_salt_lake;
    let soils = &hydrology.soils;
    let soil_texture = &hydrology.soil_texture;
    let flow_accumulation = &hydrology.flow_accumulation;
    let soil_moisture = &hydrology.soil_moisture;

    let mountain_slope_threshold = config.mountain_slope_threshold;
    let wetland_max_slope = config.wetland_max_slope;
    let riparian_river_multiplier = config.riparian_river_multiplier;
    let river_threshold = config.river_threshold;
    let aridity_evap_scale = config.aridity_evap_scale;

    let is_ocean_cell = |i: usize| -> bool {
        is_ocean
            .get(i)
            .copied()
            .unwrap_or_else(|| elevations.get(i).copied().unwrap_or(0.0) <= 0.0)
    };

    biomes
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, biome_out)| {
            let elev = elevations[i];
            let (t_spr, t_sum, t_win) = (t_spring[i], t_summer[i], t_winter[i]);

            let mean_temp = (2.0 * t_spr + t_sum + t_win) * 0.25;
            let warm_temp = t_sum.max(t_win).max(t_spr);
            let cold_temp = t_sum.min(t_win).min(t_spr);

            let t_jit = temp_jitter[i] * 1.5;
            let m_jit = moisture_jitter[i];
            let ice_jit = temp_jitter[i] * 0.8;

            let eff_mean_temp = mean_temp + t_jit;
            let eff_warm_temp = warm_temp + t_jit * 0.85;
            let eff_cold_temp = cold_temp + t_jit * 0.85;

            if is_ocean_cell(i) {
                *biome_out = if mean_temp + ice_jit < -4.5 || warm_temp + ice_jit < -1.0 {
                    Biome::SeaIce
                } else if elev > -0.09 {
                    Biome::ShallowOcean
                } else {
                    Biome::DeepOcean
                };
                return;
            }

            if lake_mask[i] {
                let is_salt = is_salt_lake[i];
                let freeze_thresh = if is_salt { -4.0 } else { -2.0 };
                *biome_out = if mean_temp + ice_jit < freeze_thresh || warm_temp + ice_jit < 0.0 {
                    Biome::LakeIce
                } else if is_salt {
                    Biome::SaltLake
                } else {
                    Biome::FreshwaterLake
                };
                return;
            }

            let annual_precip = (2.0 * p_spring[i] + p_summer[i] + p_winter[i]) * 0.25;
            let (warm_precip, cold_precip) = if t_sum >= t_win {
                (p_summer[i], p_winter[i])
            } else {
                (p_winter[i], p_summer[i])
            };

            let neighbors = &topology.cells[i].neighbors;
            let mut max_slope = 0.0_f32;
            for &n_id in neighbors {
                let diff = (elev - elevations[n_id]).abs();
                if diff > max_slope {
                    max_slope = diff;
                }
            }

            let soil = soils[i];
            let [sand, _silt, clay] = soil_texture[i];
            let flow = flow_accumulation[i];

            if eff_warm_temp < 0.0 || (eff_mean_temp < -9.0 && eff_warm_temp < 3.5) {
                *biome_out = Biome::IceSheet;
                return;
            }
            if soil == SoilType::Leptosol
                && (elev > 0.22 + m_jit * 0.02 || max_slope > mountain_slope_threshold * 1.15)
            {
                *biome_out = Biome::AlpineBare;
                return;
            }
            if eff_warm_temp < 8.5 || soil == SoilType::Gelisol {
                *biome_out = if max_slope > mountain_slope_threshold {
                    Biome::AlpineBare
                } else {
                    Biome::Tundra
                };
                return;
            }

            let sm = soil_moisture[i];
            let base_moisture_idx = (sm / (1.0 - sm * 0.68).max(0.15)).clamp(0.0, 2.5);
            let moisture_idx = (base_moisture_idx * (1.0 + m_jit * 0.12)).clamp(0.0, 2.5);

            let pet = ((mean_temp + 10.0).max(2.0) * 0.013 * aridity_evap_scale).max(0.025);
            let rain_only_sm = annual_precip / (annual_precip + pet * 0.7);
            let river_moisture_surplus = (sm - rain_only_sm).max(0.0);
            let is_major_river = flow >= river_threshold * riparian_river_multiplier;

            if max_slope < wetland_max_slope
                && elev < 0.12
                && (soil == SoilType::Gleysol || clay > 0.36)
                && (sm > 0.74 - m_jit * 0.04 || (is_major_river && sm > 0.58))
            {
                *biome_out = Biome::Wetland;
                return;
            }
            if eff_mean_temp > 5.0
                && rain_only_sm < 0.28
                && (river_moisture_surplus > 0.16 || (is_major_river && sm > 0.36))
            {
                *biome_out = Biome::RiparianOasis;
                return;
            }

            *biome_out = classify_whittaker_biome(
                eff_mean_temp,
                eff_warm_temp,
                eff_cold_temp,
                moisture_idx,
                warm_precip,
                cold_precip,
                sand,
                soil,
            );
        });

    hydrology.biomes = biomes;
}

#[allow(clippy::too_many_arguments)]
fn classify_whittaker_biome(
    mean_temp: f32,
    warm_temp: f32,
    cold_temp: f32,
    moisture_idx: f32,
    warm_precip: f32,
    cold_precip: f32,
    sand: f32,
    soil: SoilType,
) -> Biome {
    if moisture_idx < 0.18 {
        return if mean_temp < 11.0 || cold_temp < -5.0 {
            Biome::ColdDesert
        } else {
            Biome::HotDesert
        };
    }

    if moisture_idx < 0.33 {
        return if mean_temp < 9.0 || cold_temp < -6.0 {
            if soil == SoilType::Chernozem {
                Biome::TemperateGrassland
            } else {
                Biome::ColdDesert
            }
        } else {
            Biome::XericShrubland
        };
    }

    if mean_temp < 5.5 || cold_temp < -12.0 {
        return if moisture_idx < 0.55 && soil != SoilType::Podzol {
            Biome::TemperateGrassland
        } else {
            Biome::BorealTaiga
        };
    }

    if mean_temp < 19.5 {
        let is_mediterranean = cold_temp > 1.5
            && warm_temp > 18.0
            && cold_precip > warm_precip * 1.65
            && moisture_idx < 0.85;

        if is_mediterranean {
            return Biome::MediterraneanShrubland;
        }

        if moisture_idx < 0.58 {
            return Biome::TemperateGrassland;
        }

        if soil == SoilType::Podzol && sand > 0.55 && mean_temp < 11.0 {
            return Biome::BorealTaiga;
        }

        return if moisture_idx > 1.35 && cold_temp > -2.0 {
            Biome::TemperateRainforest
        } else {
            Biome::TemperateDeciduousForest
        };
    }

    let p_min = warm_precip.min(cold_precip);
    let p_max = warm_precip.max(cold_precip).max(1e-4);
    let seasonality_ratio = p_min / p_max;

    if moisture_idx < 0.72 || (moisture_idx < 0.95 && seasonality_ratio < 0.35) {
        Biome::Savanna
    } else if moisture_idx < 1.18 || seasonality_ratio < 0.45 {
        Biome::TropicalSeasonalForest
    } else {
        Biome::TropicalRainforest
    }
}
