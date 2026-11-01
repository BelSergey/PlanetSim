// src/domains/hydrology/solver/resources.rs

use noise::{Fbm, NoiseFn, Perlin};

use super::super::{
    hydrology_config::HydrologyConfig,
    hydrology_data::{BioResource, Biome, HydrologyLayer, MineralResource, SoilType},
};
use crate::{
    domains::{
        climate::climate_data::ClimateLayer,
        tectonics::tectonics_data::{GeologyLayer, RockType},
    },
    world::topology::WorldTopology,
};

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn compute_resources(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    seed: u32,
) {
    let num_cells = topology.cells.len();

    // Два независимых 3D-генератора шума для геологических жил и ареалов флоры/фауны
    let ore_fbm = Fbm::<Perlin>::new(seed.wrapping_add(0x9E37_79B9));
    let bio_fbm = Fbm::<Perlin>::new(seed.wrapping_add(0x85EB_CA6B));

    let river_thresh = config.river_threshold.max(1e-6);
    let food_mult = config.food_yield_multiplier;
    // Порог богатства жилы: при mineral_density = 1.0 отсекает слабый фон, оставляя четкие провинции
    let ore_cutoff = (0.48 / config.mineral_density.max(0.2)).clamp(0.20, 0.80);
    let bio_cutoff = (0.42 / config.bio_resource_abundance.max(0.2)).clamp(0.18, 0.78);

    // Предрасчет признака золотых/серебряных гор в соседях для образования речных россыпей в долинах
    let mut has_upland_vein = vec![false; num_cells];
    for i in 0..num_cells {
        if geology.elevations[i] > 0.08
            && matches!(
                geology.lithology[i],
                RockType::Granite | RockType::Andesite | RockType::GneissSchist
            )
        {
            let p = topology.cells[i].center.as_dvec3();
            let v_gold =
                ore_fbm.get([p.x * 18.0 + 11.2, p.y * 18.0 - 34.5, p.z * 18.0 + 7.8]) as f32;
            if v_gold > 0.28 {
                has_upland_vein[i] = true;
            }
        }
    }

    for i in 0..num_cells {
        let elev = geology.elevations[i];
        let rock = geology.lithology[i];
        let soil = hydrology.soils[i];
        let biome = hydrology.biomes[i];
        let sm = hydrology.soil_moisture[i];
        let depth = hydrology.soil_depth[i];
        let [_sand, silt, clay] = hydrology.soil_texture[i];
        let flow_ratio = (hydrology.flow_accumulation[i] / river_thresh).max(0.0);

        let t_spring = climate.spring.temperatures[i];
        let t_summer = climate.summer.temperatures[i];
        let t_winter = climate.winter.temperatures[i];
        let mean_temp = (t_spring + t_summer + t_winter) / 3.0;
        let warm_temp = t_summer.max(t_winter).max(t_spring);

        // Локальный уклон рельефа
        let mut max_slope = 0.0_f32;
        let mut near_coast = false;
        let mut near_gold_upland = has_upland_vein[i];
        for &n_id in &topology.cells[i].neighbors {
            let n_elev = geology.elevations[n_id];
            let diff = (elev - n_elev).abs();
            if diff > max_slope {
                max_slope = diff;
            }
            if (elev > 0.0 && n_elev <= 0.0) || (elev <= 0.0 && n_elev > 0.0) {
                near_coast = true;
            }
            if has_upland_vein[n_id] && n_elev > elev {
                near_gold_upland = true;
            }
        }

        let p = topology.cells[i].center.as_dvec3();

        // Независимые гармоники 3D-шума (в диапазоне [0.0 .. 1.0]) для разных типов руд
        let n_ore_a =
            (ore_fbm.get([p.x * 14.0 + 19.1, p.y * 14.0 - 41.3, p.z * 14.0 + 23.7]) as f32 * 0.65
                + 0.5)
                .clamp(0.0, 1.0);
        let n_ore_b =
            (ore_fbm.get([p.x * 16.0 - 52.4, p.y * 16.0 + 13.8, p.z * 16.0 - 39.2]) as f32 * 0.65
                + 0.5)
                .clamp(0.0, 1.0);
        let n_ore_c =
            (ore_fbm.get([p.x * 19.0 + 73.6, p.y * 19.0 + 28.9, p.z * 19.0 - 61.5]) as f32 * 0.65
                + 0.5)
                .clamp(0.0, 1.0);
        let n_basin = (ore_fbm.get([p.x * 7.5 - 15.2, p.y * 7.5 + 64.1, p.z * 7.5 + 33.8]) as f32
            * 0.65
            + 0.5)
            .clamp(0.0, 1.0);

        let n_bio_a =
            (bio_fbm.get([p.x * 10.0 + 31.4, p.y * 10.0 - 18.7, p.z * 10.0 + 52.1]) as f32 * 0.65
                + 0.5)
                .clamp(0.0, 1.0);
        let n_bio_b =
            (bio_fbm.get([p.x * 13.5 - 44.9, p.y * 13.5 + 27.3, p.z * 13.5 - 16.4]) as f32 * 0.65
                + 0.5)
                .clamp(0.0, 1.0);

        // =========================================================================
        // 1. ОКЕАН И МОРСКОЕ ДНО
        // =========================================================================
        if elev <= 0.0 {
            let wk = climate.water_kind.get(i).copied().unwrap_or(WaterKind::Ocean);
            hydrology.arable_potential[i] = 0.0;
            hydrology.pastoral_potential[i] = 0.0;
            hydrology.foraging_potential[i] = 0.0;

            match wk {
                WaterKind::Lake => {
                    // Пресноводное рыболовство: размер озера масштабирует потенциал.
                    // ice_penalty тот же, что и для наземного.
                    let size_norm = (climate.lake_size.get(i).copied().unwrap_or(0) as f32
                        / 500.0).clamp(0.15, 1.0);
                    let ice_penalty = smoothstep(-12.0, 2.0, mean_temp);
                    hydrology.fishery_potential[i] =
                        (0.55 * size_norm * ice_penalty * food_mult).clamp(0.0, 1.0);

                    // Солёные озёра (замкнутые котловины) — источник соли.
                    // Признак: аридный климат + нет стока → пока упрощённо по aridity.
                    let is_salt_lake = smoothstep(0.55, 0.85, climate.summer.temperatures[i] / 30.0)
                        * (1.0 - smoothstep(0.30, 0.55, climate.spring.humidity[i]));
                    if is_salt_lake > 0.5 && n_basin > ore_cutoff {
                        hydrology.minerals[i] = MineralResource::Salt;
                        hydrology.mineral_richness[i] = (is_salt_lake - 0.5) * 2.0;
                    } else {
                        hydrology.minerals[i] = MineralResource::None;
                        hydrology.mineral_richness[i] = 0.0;
                    }
                    hydrology.bio_resources[i] = BioResource::None;
                    hydrology.bio_richness[i] = 0.0;
                }
                WaterKind::Ocean => {
                    if biome == Biome::SeaIce {
                        hydrology.fishery_potential[i] = 0.08 * food_mult;
                    } else {
                        let shelf_factor = smoothstep(-0.14, -0.005, elev);
                        let current_speed = climate.spring.ocean_currents[i].length();
                        let upwelling_bonus = (current_speed * 0.45).clamp(0.0, 0.35);
                        let temp_opt = smoothstep(-2.0, 8.0, mean_temp)
                            * (1.0 - smoothstep(24.0, 32.0, mean_temp) * 0.25);
                        let coast_bonus = if near_coast { 0.20 } else { 0.0 };

                        hydrology.fishery_potential[i] =
                            ((0.12 + shelf_factor * 0.58 + upwelling_bonus + coast_bonus)
                                * temp_opt
                                * food_mult)
                                .clamp(0.0, 1.0);
                    }

                    // Шельфовые углеводороды (нефть и газ) на пассивных окраинах и карбонатных банках
                    if elev > -0.12
                        && matches!(
                            rock,
                            RockType::ShaleClay | RockType::Limestone | RockType::Sandstone
                        )
                        && n_basin > ore_cutoff + 0.08
                    {
                        hydrology.minerals[i] = MineralResource::Hydrocarbons;
                        hydrology.mineral_richness[i] = ((n_basin - ore_cutoff) * 1.8).clamp(0.25, 1.0);
                    } else {
                        hydrology.minerals[i] = MineralResource::None;
                        hydrology.mineral_richness[i] = 0.0;
                    }

                    // Китобойный и зверобойный промысел в холодных морях у кромки льдов
                    if (biome == Biome::SeaIce || (mean_temp < 4.0 && elev > -0.15))
                        && n_bio_a > bio_cutoff + 0.12
                    {
                        hydrology.bio_resources[i] = BioResource::FurIvory;
                        hydrology.bio_richness[i] = ((n_bio_a - bio_cutoff) * 1.6).clamp(0.2, 1.0);
                    } else {
                        hydrology.bio_resources[i] = BioResource::None;
                        hydrology.bio_richness[i] = 0.0;
                    }
                    continue;
                }
                WaterKind::Land => unreachable!(),
            }
            continue;
        }   

        // =========================================================================
        // 2. СУША: ПРОДОВОЛЬСТВЕННЫЙ ПОТЕНЦИАЛ (4 СТОЛПА ЕДЫ)
        // =========================================================================

        // 2.1. Земледелие (Arable: злаки, рис, овощи)
        let soil_fert = match soil {
            SoilType::Chernozem => 1.00,
            SoilType::Andisol => 0.95,
            SoilType::Cambisol => 0.78,
            SoilType::Gleysol => 0.76, // Плодородный речной аллювий (при осушении/ирригации)
            SoilType::Laterite => 0.42, // Быстро истощается без удобрений, но дает рис в долинах
            SoilType::Podzol => 0.32,  // Кислая почва (рожь, овес, ячмень)
            SoilType::Aridisol => 0.25, // Требует речного орошения
            SoilType::Gelisol | SoilType::Leptosol | SoilType::Ocean => 0.04,
        };

        let rugged = geology.terrain_ruggedness[i];
        let silt_loam_bonus = (silt * 0.45).clamp(0.0, 0.22);
        let growing_season = smoothstep(5.0, 16.0, warm_temp) * smoothstep(-14.0, 6.0, mean_temp);
        // Оптимум влаги в почве 0.35..0.80; при переувлажнении в холоде гниют корни, в тропиках — растет рис
        let moisture_curve = smoothstep(0.14, 0.42, sm)
            * (1.0 - smoothstep(0.82, 0.98, sm) * if mean_temp > 18.0 { 0.15 } else { 0.55 });
        let flat_terrain = ((1.0 - max_slope / 0.045) * (1.0 - rugged * 0.75))
            .clamp(0.0, 1.0)
            .powi(2);
        let river_irrigation = if flow_ratio > 0.6 && sm > 0.25 {
            (flow_ratio.sqrt() * 0.12).clamp(0.0, 0.28)
        } else {
            0.0
        };

        hydrology.arable_potential[i] = (((soil_fert + silt_loam_bonus + river_irrigation)
            * growing_season
            * moisture_curve
            * flat_terrain
            * depth.sqrt())
            * food_mult)
            .clamp(0.0, 1.0);

        // 2.2. Пастбищное скотоводство (Pastoral: мясо, молоко, шерсть)
        let biome_pasture = match biome {
            Biome::TemperateGrassland => 0.95,
            Biome::Savanna => 0.85,
            Biome::XericShrubland | Biome::MediterraneanShrubland => 0.62,
            Biome::RiparianOasis => 0.65,
            Biome::Tundra => 0.42, // Оленеводство
            Biome::ColdDesert => 0.35,
            Biome::TemperateDeciduousForest | Biome::TropicalSeasonalForest => 0.38,
            Biome::AlpineBare => 0.28, // Высокогорные альпийские луга (яки, овцы, ламы)
            Biome::HotDesert | Biome::BorealTaiga | Biome::Wetland => 0.14,
            Biome::TropicalRainforest | Biome::TemperateRainforest => 0.10, // Густой лес и муха цеце
            _ => 0.0,
        };
        let slope_pasture = ((1.0 - max_slope / 0.095) * (1.0 - rugged * 0.45)).clamp(0.15, 1.0);
        let temp_pasture = smoothstep(-18.0, -2.0, mean_temp);
        hydrology.pastoral_potential[i] =
            (biome_pasture * slope_pasture * temp_pasture * (0.4 + 0.6 * sm.sqrt()) * food_mult)
                .clamp(0.0, 1.0);

        // 2.3. Собирательство, сады и дичь (Foraging & Orchards: фрукты, ягоды, орехи, виноград, охота)
        let biome_forage = match biome {
            Biome::TropicalSeasonalForest | Biome::MediterraneanShrubland => 0.92, // Цитрусовые, бананы, оливки, инжир, виноград
            Biome::TropicalRainforest => 0.86, // Обилие тропических плодов, клубней и дичи
            Biome::RiparianOasis => 0.88,      // Финики, гранаты, водоплавающая дичь
            Biome::TemperateDeciduousForest => 0.80, // Яблоки, груши, орехи, грибы, олени, кабаны
            Biome::TemperateRainforest => 0.72, // Ягоды, орехи, лососевые реки
            Biome::Savanna => 0.64, // Крупная дичь, корнеплоды, баобабы (колыбель антропогенеза)
            Biome::Wetland => 0.58, // Дикий рис, клюква, птица
            Biome::BorealTaiga => 0.48, // Кедровый орех, брусника, лесная дичь
            Biome::TemperateGrassland | Biome::XericShrubland => 0.34,
            Biome::Tundra => 0.24, // Морошка, северная дичь
            _ => 0.05,
        };
        let microclimate_bonus = smoothstep(-8.0, 14.0, mean_temp) * (0.5 + 0.5 * sm);
        hydrology.foraging_potential[i] =
            (biome_forage * (0.55 + 0.45 * microclimate_bonus) * food_mult).clamp(0.0, 1.0);

        // 2.4. Пресноводное и прибрежное рыболовство на суше (реки, дельты, берега морей)
        let river_fish = if flow_ratio >= 0.4 {
            (flow_ratio.powf(0.45) * 0.32).clamp(0.0, 0.85)
        } else {
            0.0
        };
        let coastal_fish = if near_coast { 0.45 } else { 0.0 };
        let wetland_fish = if biome == Biome::Wetland { 0.25 } else { 0.0 };
        let ice_penalty = smoothstep(-12.0, 2.0, mean_temp);
        hydrology.fishery_potential[i] =
            ((river_fish + coastal_fish + wetland_fish) * ice_penalty * food_mult).clamp(0.0, 1.0);

        // =========================================================================
        // 3. ИСКОПАЕМЫЕ РЕСУРСЫ (15 ТИПОВ: ГЕОЛОГИЯ + ПОЧВЫ + ГИДРОЛОГИЯ)
        // =========================================================================
        let mut best_mineral = MineralResource::None;
        let mut best_m_score = ore_cutoff;
        let outcrop_bonus = 1.0 + rugged * 0.18;

        let mut consider_mineral = |res: MineralResource, score: f32| {
            if score > best_m_score {
                best_m_score = score;
                best_mineral = res;
            }
        };

        // 1. Глина (Clay): речные поймы, глеевые низины и выходы глинистых сланцев
        let clay_score = if soil == SoilType::Gleysol || rock == RockType::ShaleClay {
            (clay * 0.85 + (1.0 - max_slope * 25.0).clamp(0.0, 0.35)) * (0.65 + 0.35 * n_ore_a)
        } else {
            0.0
        };
        consider_mineral(MineralResource::Clay, clay_score);

        // 2. Строительный камень и Известняк (StoneLimestone): карбонатные платформы и скальные выходы
        let stone_score = match rock {
            RockType::Limestone => 0.62 + 0.38 * n_ore_b,
            RockType::Granite | RockType::Sandstone
                if soil == SoilType::Leptosol || max_slope > 0.025 =>
            {
                0.52 + 0.40 * n_ore_b
            }
            _ => 0.0,
        };
        consider_mineral(MineralResource::StoneLimestone, stone_score);

        // 3. Кварцевый песок и Кремний (SilicaQuartz): чистые песчаники и кварцево-гранитные пески
        let silica_score = if rock == RockType::Sandstone
            || (rock == RockType::Granite && hydrology.soil_texture[i][0] > 0.52)
        {
            (hydrology.soil_texture[i][0] * 0.70 + 0.30) * n_ore_c
        } else {
            0.0
        };
        consider_mineral(MineralResource::SilicaQuartz, silica_score);

        // 4. Соль (Salt): пустынные солончаки (Aridisol), бессточные котловины и древние лагуны на известняках
        let salt_score = if soil == SoilType::Aridisol && max_slope < 0.015 {
            (0.68 + (1.0 - sm) * 0.25) * n_ore_a
        } else if rock == RockType::Limestone && (near_coast || sm < 0.35) {
            0.60 * n_ore_a
        } else {
            0.0
        };
        consider_mineral(MineralResource::Salt, salt_score);

        // 5. Медь (Copper): порфировые месторождения андезитовых дуг и медистые траппы/песчаники
        let copper_score = match rock {
            RockType::Andesite => (0.64 + elev.clamp(0.0, 0.35)) * n_ore_a,
            RockType::FloodBasalt => 0.68 * n_ore_b,
            RockType::Sandstone if elev > 0.08 => 0.56 * n_ore_a,
            _ => 0.0,
        };
        consider_mineral(MineralResource::Copper, copper_score * outcrop_bonus);

        // 6. Олово (Tin): редкие грейзеновые жилы в кислых гранитных плутонах
        let tin_score = if rock == RockType::Granite {
            (0.58 + elev.clamp(0.0, 0.35) * 0.5) * n_ore_b.powf(1.35)
        } else {
            0.0
        };
        consider_mineral(MineralResource::Tin, tin_score * outcrop_bonus);

        // 7. Железо (Iron): железистые кварциты щитов (GneissSchist), скарны и болотные руды
        let iron_score = match rock {
            RockType::GneissSchist => 0.74 * n_ore_a,
            RockType::Granite | RockType::FloodBasalt => 0.58 * n_ore_c,
            _ if biome == Biome::Wetland || soil == SoilType::Gleysol => 0.56 * n_ore_b,
            _ => 0.0,
        };
        consider_mineral(MineralResource::Iron, iron_score * outcrop_bonus);

        // 8. Золото и Серебро (PreciousMetals): коренные жилы в горах + аллювиальные россыпи в реках ниже гор
        let gold_vein = if matches!(
            rock,
            RockType::Granite | RockType::Andesite | RockType::GneissSchist
        ) && elev > 0.10
        {
            0.68 * n_ore_c.powf(1.3)
        } else {
            0.0
        };
        let gold_placer = if near_gold_upland && flow_ratio > 0.7 {
            0.76 * n_ore_a
        } else {
            0.0
        };
        consider_mineral(
            MineralResource::PreciousMetals,
            gold_vein.max(gold_placer) * outcrop_bonus,
        );

        // 9. Сера, Селитра и Фосфаты (SulfurNitre): вулканы (сера) и гипераридные пустыни/карст (селитра/фосфориты)
        let sulfur_score = if geology.volcano_mask[i] || rock == RockType::Andesite {
            (if geology.volcano_mask[i] { 0.85 } else { 0.62 }) * n_ore_b
        } else if soil == SoilType::Aridisol && sm < 0.16 {
            0.72 * n_ore_c // Чилийская селитра / фосфориты
        } else {
            0.0
        };
        consider_mineral(MineralResource::SulfurNitre, sulfur_score);

        // 10. Каменный уголь (Coal): осадочные бассейны песчаников и аргиллитов + торфяники
        let coal_score = if matches!(rock, RockType::Sandstone | RockType::ShaleClay) && elev < 0.16
        {
            0.74 * n_basin * n_ore_b
        } else if biome == Biome::Wetland {
            0.62 * n_basin
        } else {
            0.0
        };
        consider_mineral(MineralResource::Coal, coal_score);

        // 11. Нефть и Природный газ (Hydrocarbons): глубокие осадочные прогибы низменностей и предгорий
        let oil_score = if matches!(
            rock,
            RockType::ShaleClay | RockType::Limestone | RockType::Sandstone
        ) && elev < 0.095
            && max_slope < 0.016
        {
            0.80 * n_basin.powf(1.15) * n_ore_c
        } else {
            0.0
        };
        consider_mineral(MineralResource::Hydrocarbons, oil_score);

        // 12. Бокситы / Алюминий (Bauxite): латеритные коры выветривания влажных тропиков
        let bauxite_score = if soil == SoilType::Laterite {
            0.82 * n_ore_a
        } else if mean_temp > 20.0
            && sm > 0.55
            && matches!(
                rock,
                RockType::FloodBasalt | RockType::Andesite | RockType::ShaleClay
            )
        {
            0.66 * n_ore_a
        } else {
            0.0
        };
        consider_mineral(MineralResource::Bauxite, bauxite_score);

        // 13. Легирующие металлы и Титан (AlloyMetals): трапповые провинции (FloodBasalt, тип Норильска) и щиты
        let alloy_score = match rock {
            RockType::FloodBasalt => 0.80 * n_ore_c,
            RockType::GneissSchist => 0.64 * n_ore_b,
            _ => 0.0,
        };
        consider_mineral(MineralResource::AlloyMetals, alloy_score);

        // 14. Уран (Uranium): древние гранито-гнейсовые щиты и контакты с песчаниками (отдельно от РЗМ!)
        let uranium_score = if matches!(rock, RockType::Granite | RockType::GneissSchist) {
            0.70 * n_ore_a.powf(1.45) * n_ore_c
        } else if rock == RockType::Sandstone && elev > 0.07 {
            0.62 * n_ore_a.powf(1.5)
        } else {
            0.0
        };
        consider_mineral(MineralResource::Uranium, uranium_score * outcrop_bonus);

        // 15. Редкоземельные металлы и Литий (RareEarths): щелочные интрузии и солончаково-вулканические котловины
        let ree_score = if (soil == SoilType::Aridisol || elev > 0.12)
            && (geology.volcano_mask[i] || matches!(rock, RockType::Andesite | RockType::Granite))
        {
            0.76 * n_ore_b.powf(1.40) * n_ore_c
        } else if rock == RockType::GneissSchist && elev > 0.10 {
            0.64 * n_ore_b.powf(1.5)
        } else {
            0.0
        };
        consider_mineral(MineralResource::RareEarths, ree_score * outcrop_bonus);

        hydrology.minerals[i] = best_mineral;
        hydrology.mineral_richness[i] = if best_mineral != MineralResource::None {
            ((best_m_score - ore_cutoff) / (1.0 - ore_cutoff).max(0.1)).clamp(0.20, 1.0)
        } else {
            0.0
        };

        // =========================================================================
        // 4. СТРАТЕГИЧЕСКИЕ БИОРЕСУРСЫ (ФЛОРА И ФАУНА, 9 ТИПОВ)
        // =========================================================================
        let mut best_bio = BioResource::None;
        let mut best_b_score = bio_cutoff;

        let mut consider_bio = |res: BioResource, score: f32| {
            if score > best_b_score {
                best_b_score = score;
                best_bio = res;
            }
        };

        match biome {
            Biome::BorealTaiga => {
                // Корабельная сосна/лиственница, сосновая смола/дёготь и ценная пушнина
                consider_bio(BioResource::NavalTimber, (0.55 + 0.35 * sm) * n_bio_a);
                consider_bio(BioResource::ResinRubber, 0.78 * n_bio_b);
                consider_bio(
                    BioResource::FurIvory,
                    (0.72 + (-mean_temp).max(0.0) * 0.015) * (1.0 - n_bio_a * 0.5),
                );
                consider_bio(BioResource::TimberFuel, 0.52);
            }
            Biome::TemperateDeciduousForest | Biome::TemperateRainforest => {
                // Корабельный дуб/кедр, лен/конопля на полянах и обычная древесина
                consider_bio(BioResource::NavalTimber, (0.68 + 0.25 * depth) * n_bio_a);
                consider_bio(BioResource::FiberCrops, 0.66 * n_bio_b);
                consider_bio(BioResource::TimberFuel, 0.58 * (1.0 - n_bio_a * 0.4));
            }
            Biome::TropicalRainforest => {
                // Каучук (гевея), экзотические пряности/какао/красители и ценная древесина
                consider_bio(BioResource::ResinRubber, 0.84 * n_bio_a);
                consider_bio(BioResource::LuxuryFlora, 0.82 * n_bio_b);
                consider_bio(BioResource::TimberFuel, 0.55);
            }
            Biome::TropicalSeasonalForest => {
                // Тиковый корабельный лес, пряности/чай/сахар, хлопок и рабочие/боевые слоны
                consider_bio(BioResource::NavalTimber, 0.72 * n_bio_a);
                consider_bio(BioResource::LuxuryFlora, 0.76 * n_bio_b);
                consider_bio(BioResource::ElephantsDraft, 0.75 * (1.0 - n_bio_a * 0.5));
                consider_bio(BioResource::FiberCrops, 0.68 * (1.0 - n_bio_b * 0.4));
            }
            Biome::Savanna => {
                // Слоны/буйволы, хлопок в увлажненных долинах и лошади в прохладных лесостепях
                if mean_temp > 18.0 {
                    consider_bio(BioResource::ElephantsDraft, 0.78 * n_bio_a);
                    consider_bio(BioResource::FiberCrops, (0.45 + 0.45 * sm) * n_bio_b);
                } else {
                    consider_bio(BioResource::Horses, 0.76 * n_bio_a);
                }
            }
            Biome::TemperateGrassland => {
                // Главный ареал табунов лошадей + технические волокна (лён/конопля/хлопок на юге)
                consider_bio(BioResource::Horses, 0.86 * n_bio_a);
                if sm > 0.42 {
                    consider_bio(BioResource::FiberCrops, 0.72 * n_bio_b);
                }
            }
            Biome::MediterraneanShrubland => {
                // Благовония/красители/оливки/виноград, ливанский кедр в горах и хлопок
                consider_bio(BioResource::LuxuryFlora, 0.78 * n_bio_b);
                if elev > 0.08 {
                    consider_bio(BioResource::NavalTimber, 0.68 * n_bio_a);
                } else {
                    consider_bio(BioResource::FiberCrops, 0.64 * n_bio_a);
                }
            }
            Biome::HotDesert | Biome::ColdDesert | Biome::XericShrubland => {
                // Дромадеры и бактрианы (верблюды)
                consider_bio(BioResource::Camels, 0.82 * n_bio_a);
            }
            Biome::RiparianOasis => {
                // Египетский/среднеазиатский длинноволокнистый хлопок, сахар/пряности и пальмовая древесина
                consider_bio(BioResource::FiberCrops, 0.88 * n_bio_a);
                consider_bio(BioResource::LuxuryFlora, 0.74 * n_bio_b);
                consider_bio(BioResource::TimberFuel, 0.50);
            }
            Biome::Tundra => {
                // Северная пушнина и моржовая/мамонтовая кость
                consider_bio(BioResource::FurIvory, 0.80 * n_bio_a);
            }
            Biome::AlpineBare if elev > 0.14 && mean_temp > -10.0 => {
                // Высокогорные вьючные животные (ламы, альпаки, яки)
                consider_bio(BioResource::ElephantsDraft, 0.68 * n_bio_b);
            }
            Biome::Wetland => {
                consider_bio(BioResource::FiberCrops, 0.58 * n_bio_b);
            }
            _ => {}
        }

        hydrology.bio_resources[i] = best_bio;
        hydrology.bio_richness[i] = if best_bio != BioResource::None {
            ((best_b_score - bio_cutoff) / (1.0 - bio_cutoff).max(0.1)).clamp(0.20, 1.0)
        } else {
            0.0
        };
    }
}
