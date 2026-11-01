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
    let num_cells = topology.cells.len();

    for i in 0..num_cells {
        let elev = geology.elevations[i];

        // 1. Сезонные температуры и осадки
        let t_spring = climate.spring.temperatures[i];
        let t_summer = climate.summer.temperatures[i];
        let t_winter = climate.winter.temperatures[i];

        let mean_temp = (2.0 * t_spring + t_summer + t_winter) * 0.25;
        let warm_temp = t_summer.max(t_winter).max(t_spring);
        let cold_temp = t_summer.min(t_winter).min(t_spring);

        // Когерентный шум для рваных экотонов (не меняет сами физические массивы климата)
        let t_jit = noise.temp_jitter[i] * 1.5;
        let m_jit = noise.moisture_jitter[i];

        let eff_mean_temp = mean_temp + t_jit;
        let eff_warm_temp = warm_temp + t_jit * 0.85;
        let eff_cold_temp = cold_temp + t_jit * 0.85;

        // 2. Океанические биомы (с естественной кромкой морского льда)
        if elev <= 0.0 {
            let ice_jitter = noise.temp_jitter[i] * 0.8;
            hydrology.biomes[i] = if mean_temp + ice_jitter < -4.5 || warm_temp + ice_jitter < -1.0
            {
                Biome::SeaIce
            } else if elev > -0.09 {
                Biome::ShallowOcean
            } else {
                Biome::DeepOcean
            };
            continue;
        }

        let p_spring = climate.spring.precipitation[i];
        let p_summer = climate.summer.precipitation[i];
        let p_winter = climate.winter.precipitation[i];
        let annual_precip = (2.0 * p_spring + p_summer + p_winter) * 0.25;

        // Определяем осадки самого теплого и самого холодного сезона (для муссонов и Средиземноморья)
        let (warm_precip, cold_precip) = if t_summer >= t_winter {
            (p_summer, p_winter)
        } else {
            (p_winter, p_summer)
        };

        // 3. Рельеф (уклон)
        let neighbors = &topology.cells[i].neighbors;
        let mut max_slope = 0.0_f32;
        for &n_id in neighbors {
            let diff = (elev - geology.elevations[n_id]).abs();
            if diff > max_slope {
                max_slope = diff;
            }
        }

        let soil = hydrology.soils[i];
        let [sand, _silt, clay] = hydrology.soil_texture[i];
        let flow = hydrology.flow_accumulation[i];

        // 4. Криосфера и высокогорные скалы
        // Вечные льды (ледниковые щиты и снежные вершины)
        if eff_warm_temp < 0.0 || (eff_mean_temp < -9.0 && eff_warm_temp < 3.5) {
            hydrology.biomes[i] = Biome::IceSheet;
            continue;
        }

        // Высокогорные скалы и осыпи (крутые склоны на Leptosol или выше линии леса)
        if soil == SoilType::Leptosol
            && (elev > 0.22 + m_jit * 0.02 || max_slope > config.mountain_slope_threshold * 1.15)
        {
            hydrology.biomes[i] = Biome::AlpineBare;
            continue;
        }

        // Тундра (короткое холодное лето, деревья не могут вырасти, мерзлотный Gelisol)
        if eff_warm_temp < 8.5 || soil == SoilType::Gelisol {
            hydrology.biomes[i] = if max_slope > config.mountain_slope_threshold {
                Biome::AlpineBare
            } else {
                Biome::Tundra
            };
            continue;
        }

        // 5. ГЛАВНЫЙ ИСТОЧНИК ВЛАГИ ДЛЯ БИОМОВ — ВЛАЖНОСТЬ ПОЧВЫ (soil_moisture)
        let sm = hydrology.soil_moisture[i];

        // Переводим влажность почвы [0..1] в эквивалентный индекс увлажнения Уиттекера
        // и возмущаем порог 3D-шумом (±12%) для формирования лесостепи и пятнистых переходов
        let base_moisture_idx = (sm / (1.0 - sm * 0.68).max(0.15)).clamp(0.0, 2.5);
        let moisture_idx = (base_moisture_idx * (1.0 + m_jit * 0.12)).clamp(0.0, 2.5);

        // Оцениваем, сколько влаги пришло исключительно от дождей (без рек)
        let pet = ((mean_temp + 10.0).max(2.0) * 0.013 * config.aridity_evap_scale).max(0.025);
        let rain_only_sm = annual_precip / (annual_precip + pet * 0.7);
        let river_moisture_surplus = (sm - rain_only_sm).max(0.0);

        let is_major_river = flow >= config.river_threshold * config.riparian_river_multiplier;

        // 6. Азональные биомы от переувлажнения почвы и рек:
        // А) Болота и плавни (Wetland): почва перенасыщена водой на плоской глинистой равнине или в дельте
        if max_slope < config.wetland_max_slope
            && elev < 0.12
            && (soil == SoilType::Gleysol || clay > 0.36)
            && (sm > 0.74 - m_jit * 0.04 || (is_major_river && sm > 0.58))
        {
            hydrology.biomes[i] = Biome::Wetland;
            continue;
        }

        // Б) Пойменный оазис / галерейный лес (RiparianOasis):
        if eff_mean_temp > 5.0
            && rain_only_sm < 0.28
            && (river_moisture_surplus > 0.16 || (is_major_river && sm > 0.36))
        {
            hydrology.biomes[i] = Biome::RiparianOasis;
            continue;
        }

        // 7. Классификация Уиттекера по влажности почвы и сезонности
        hydrology.biomes[i] = classify_whittaker_biome(
            eff_mean_temp,
            eff_warm_temp,
            eff_cold_temp,
            moisture_idx,
            warm_precip,
            cold_precip,
            sand,
            soil,
        );
    }
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
    // --- А. ЭКСТРЕМАЛЬНО СУХИЕ ЗОНЫ (Пустыни и полупустыни) ---
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

    // --- Б. БОРЕАЛЬНЫЙ ПОЯС (Холодная зима или прохладное лето -> Тайга / Степь) ---
    if mean_temp < 5.5 || cold_temp < -12.0 {
        return if moisture_idx < 0.55 && soil != SoilType::Podzol {
            Biome::TemperateGrassland
        } else {
            Biome::BorealTaiga
        };
    }

    // --- В. УМЕРЕННЫЙ И СУБТРОПИЧЕСКИЙ ПОЯС (5.5°C .. 19.5°C) ---
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

    // --- Г. ТРОПИЧЕСКИЙ И ЭКВАТОРИАЛЬНЫЙ ПОЯС (> 19.5°C) ---
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
