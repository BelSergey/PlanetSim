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
    let num_cells = topology.cells.len();

    for i in 0..num_cells {
        let elev = geology.elevations[i];

        // Океаническое дно не имеет наземного почвенного профиля
        if elev <= 0.0 {
            hydrology.soils[i] = SoilType::Ocean;
            hydrology.soil_texture[i] = [0.0, 0.3, 0.7];
            hydrology.soil_depth[i] = 0.0;
            hydrology.water_capacity[i] = 1.0;
            hydrology.permeability[i] = 0.0;
            continue;
        }

        // 1. Климатические показатели за год (Весна/Осень имеет двойной вес в цикле года)
        let t_spring = climate.spring.temperatures[i];
        let t_summer = climate.summer.temperatures[i];
        let t_winter = climate.winter.temperatures[i];
        let mean_temp = (2.0 * t_spring + t_summer + t_winter) * 0.25;
        let warm_season_temp = t_summer.max(t_winter);

        let p_spring = climate.spring.precipitation[i];
        let p_summer = climate.summer.precipitation[i];
        let p_winter = climate.winter.precipitation[i];
        let annual_precip = (2.0 * p_spring + p_summer + p_winter) * 0.25;

        // Потенциальная испаряемость (PET) по температуре и индекс влажности (P / PET)
        let pet = ((mean_temp + 10.0).max(2.0) * 0.022 * config.aridity_evap_scale).max(0.05);
        let moisture_index = annual_precip / pet;

        // Когерентные возмущения для неровных природных границ почвенных поясов
        let t_jit = noise.temp_jitter[i] * 1.4;
        let m_jit = noise.moisture_jitter[i];
        let eff_mean_temp = mean_temp + t_jit;
        let eff_warm_temp = warm_season_temp + t_jit * 0.8;
        let eff_moisture_index = (moisture_index * (1.0 + m_jit * 0.14)).max(0.0);

        // 2. Рельеф: максимальный уклон и локальная вогнутость (низина vs хребет)
        let neighbors = &topology.cells[i].neighbors;
        let mut max_slope = 0.0_f32;
        let mut neighbor_elev_sum = 0.0_f32;

        for &n_id in neighbors {
            let n_elev = geology.elevations[n_id];
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
        // Положительный valley_concavity означает ложбину/котловину, где скапливается ил
        let valley_concavity = (avg_n_elev - elev).clamp(-0.05, 0.05);

        // 3. Материнская порода -> Базовая текстура [sand, silt, clay]
        let rock = geology.lithology[i];
        let [mut sand, mut silt, mut clay] = rock.base_texture();

        // 4. Модификация текстуры климатом и рельефом (CLORPT)
        // Химическое выветривание (жара + влага переводят песок и ил во вторичные глины)
        let heat_factor = ((eff_mean_temp - 5.0) / 25.0).clamp(0.0, 1.2);
        let wet_factor = (eff_moisture_index * 0.6).clamp(0.0, 1.2);
        let chem_weathering = heat_factor * wet_factor * config.chemical_weathering_rate;

        // Физическое морозное выветривание (холод дробит породу в пылеватый ил и песок)
        let frost_weathering = ((5.0 - eff_mean_temp) / 25.0).clamp(0.0, 0.45);

        // Смыв мелких фракций со склонов и аккумуляция в ложбинах
        let slope_wash = (max_slope * config.slope_erosion_sensitivity).clamp(0.0, 0.75);
        let basin_deposition = (valley_concavity * 12.0).clamp(0.0, 0.35);

        sand = (sand - chem_weathering * 0.35 + slope_wash * 0.45 + frost_weathering * 0.10
            - m_jit * 0.03)
            .max(0.05);
        silt = (silt + frost_weathering * 0.25 + basin_deposition * 0.20
            - chem_weathering * 0.15
            - slope_wash * 0.20)
            .max(0.05);
        clay = (clay + chem_weathering * 0.50 + basin_deposition * 0.25 - slope_wash * 0.25
            + m_jit * 0.03)
            .max(0.05);

        let tex_sum = sand + silt + clay;
        sand /= tex_sum;
        silt /= tex_sum;
        clay /= tex_sum;
        hydrology.soil_texture[i] = [sand, silt, clay];

        // 5. Мощность почвенного профиля (глубина)
        let climate_depth_mod =
            (0.45 + 0.55 * (wet_factor * 0.7 + heat_factor * 0.3)).clamp(0.3, 1.0);
        let slope_depth_factor =
            (1.0 - max_slope * config.slope_erosion_sensitivity * 0.85).clamp(0.08, 1.0);
        let depth = (slope_depth_factor * climate_depth_mod).clamp(0.08, 1.0);
        hydrology.soil_depth[i] = depth;

        // 6. Педотрансферные функции: Влагоемкость (WHC) и Водопроницаемость
        let is_permafrost = eff_mean_temp < config.permafrost_temp_threshold || eff_warm_temp < 3.0;

        // Ил и суглинки лучше всего удерживают доступную растениям воду; вулканический пепел (Andisol) повышает WHC
        let volcanic_bonus = if geology.volcano_mask[i] || rock == RockType::Andesite {
            1.25
        } else {
            1.0
        };
        let specific_whc = (0.12 * sand + 0.52 * silt + 0.38 * clay) * volcanic_bonus;
        hydrology.water_capacity[i] = (specific_whc * (0.25 + 0.75 * depth)).clamp(0.05, 1.0);

        // Проницаемость: максимальна в песках и карсте, блокируется глиной и вечной мерзлотой
        let raw_perm = sand * 0.82 + silt * 0.24 + clay * 0.04 + rock.karst_permeability();
        hydrology.permeability[i] = if is_permafrost {
            0.03 // Мерзлота работает как водоупор
        } else {
            (raw_perm * (0.4 + 0.6 * depth)).clamp(0.02, 1.0)
        };

        // 7. Итоговая классификация почвенного типа (SoilType)
        hydrology.soils[i] = classify_soil_type(
            eff_mean_temp,
            eff_warm_temp,
            eff_moisture_index,
            max_slope,
            elev,
            depth,
            sand,
            clay,
            rock,
            geology.volcano_mask[i],
            config,
        );
    }
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
    config: &HydrologyConfig,
) -> SoilType {
    // 1. Крутые горные склоны и скальные гребни -> Литоземы / Скелетные почвы
    if max_slope > config.mountain_slope_threshold || depth < 0.18 {
        return SoilType::Leptosol;
    }

    // 2. Зона вечной мерзлоты и полярных тундр -> Мерзлотные почвы (Гелисоли)
    if mean_temp < config.permafrost_temp_threshold || warm_season_temp < 4.0 {
        return SoilType::Gelisol;
    }

    // 3. Молодые вулканические породы (при достаточном увлажнении) -> Андисоли
    if (is_hotspot || rock == RockType::Andesite) && moisture_index > 0.28 {
        return SoilType::Andisol;
    }

    // 4. Экстремально засушливый климат -> Пустынные почвы (Аридисоли / Сероземы)
    if moisture_index < 0.24 {
        return SoilType::Aridisol;
    }

    // 5. Плоские глинистые переувлажненные низины -> Болотные и пойменные Глееземы
    if elev < 0.08 && max_slope < 0.008 && moisture_index > 0.85 && clay > 0.34 {
        return SoilType::Gleysol;
    }

    // 6. Жаркие влажные тропики и субтропики -> Латериты (Красноземы / Оксисоли)
    if mean_temp > 19.5 && moisture_index > 0.72 {
        return SoilType::Laterite;
    }

    // 7. Умеренно-сухие и семигумидные степи/лесостепи -> Черноземы
    if (1.5..=19.0).contains(&mean_temp) && (0.24..=0.64).contains(&moisture_index) {
        return SoilType::Chernozem;
    }

    // 8. Прохладные влажные хвойные леса и кислые породы -> Подзолы
    if mean_temp < 9.5 && moisture_index >= 0.55
        || (mean_temp < 13.5
            && sand > 0.48
            && matches!(rock, RockType::Granite | RockType::Sandstone))
    {
        return SoilType::Podzol;
    }

    // 9. Умеренно-теплые влажные регионы -> Буроземы (Cambisol)
    SoilType::Cambisol
}
