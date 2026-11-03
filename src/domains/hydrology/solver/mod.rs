// src/domains/hydrology/solver/mod.rs

mod biomes;
mod lakes;
mod resources;
mod rivers;
mod soils;

use noise::{Fbm, NoiseFn, Perlin};

use super::{
    hydrology_config::HydrologyConfig,
    hydrology_data::{Biome, HydrologyLayer},
    hydrology_palettes::biome_satellite_rgb,
};
use crate::{
    domains::{
        climate::{ClimateConfig, climate_data::ClimateLayer},
        generator::GenerationLogger,
        tectonics::tectonics_data::GeologyLayer,
    },
    world::topology::WorldTopology,
};
use biomes::compute_biomes;
use lakes::compute_lakes_and_basins;
use resources::compute_resources;
use rivers::{compute_runoff, compute_soil_moisture, route_rivers_with_lakes};
use soils::compute_soils;

/// Когерентные 3D-поля шума для экотонов почв/биомов и меандрирования рек
pub(super) struct SurfaceNoise {
    pub temp_jitter: Vec<f32>,
    pub moisture_jitter: Vec<f32>,
    pub meander_field: Vec<f32>,
}

impl SurfaceNoise {
    fn new(topology: &WorldTopology, seed: u32) -> Self {
        let num_cells = topology.cells.len();
        let fbm = Fbm::<Perlin>::new(seed.wrapping_add(0x517C_C1B7));

        let mut temp_jitter = Vec::with_capacity(num_cells);
        let mut moisture_jitter = Vec::with_capacity(num_cells);
        let mut meander_field = Vec::with_capacity(num_cells);

        for cell in topology.cells.iter() {
            let p = cell.center.as_dvec3();

            let t_macro = fbm.get([p.x * 11.0 + 17.3, p.y * 11.0 - 29.1, p.z * 11.0 + 43.7]) as f32;
            let t_micro = fbm.get([p.x * 28.0 - 11.4, p.y * 28.0 + 51.2, p.z * 28.0 - 19.8]) as f32;
            temp_jitter.push(t_macro * 0.70 + t_micro * 0.30);

            let m_macro = fbm.get([p.x * 12.0 - 63.1, p.y * 12.0 + 14.8, p.z * 12.0 - 37.5]) as f32;
            let m_micro = fbm.get([p.x * 29.0 + 41.2, p.y * 29.0 - 23.9, p.z * 29.0 + 68.4]) as f32;
            moisture_jitter.push(m_macro * 0.70 + m_micro * 0.30);

            let r_noise = fbm.get([p.x * 22.0 + 83.5, p.y * 22.0 + 19.2, p.z * 22.0 - 54.6]) as f32;
            meander_field.push(r_noise);
        }

        Self {
            temp_jitter,
            moisture_jitter,
            meander_field,
        }
    }
}

pub fn simulate_hydrology(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
    seed: u32,
    logger: &GenerationLogger,
) {
    hydrology.resize(topology.cells.len());
    let noise = SurfaceNoise::new(topology, seed);

    // Этап 1: Почвы и педотрансферные функции (CLORPT)
    logger.start_step("Биосфера: формирование почвенного покрова (CLORPT)...");
    compute_soils(topology, geology, climate, hydrology, config, &noise);

    // Этап 2: Поверхностный сток, заполнение впадин и баланс озёр (проточные и бессточные)
    logger.start_step("Биосфера: водный баланс котловин и генерация озёр...");
    let meander_bias = compute_runoff(topology, geology, climate, hydrology, config, &noise);
    let lake_routing = compute_lakes_and_basins(topology, geology, climate, hydrology, config);

    // Этап 3: Трассировка речной сети и фильтрация влаги в почву
    logger.start_step("Биосфера: трассировка рек и расчёт влажности почвы...");
    route_rivers_with_lakes(
        topology,
        geology,
        hydrology,
        config,
        &meander_bias,
        &lake_routing,
    );
    compute_soil_moisture(topology, geology, climate, hydrology, config);

    // Этап 4: Биомы (строятся по влажности почвы, озёрам, климату и рельефу)
    logger.start_step("Биосфера: классификация биомов и экосистем...");
    compute_biomes(topology, geology, climate, hydrology, config, &noise);

    // Этап 5: Продовольственный потенциал, полезные ископаемые и биоресурсы
    logger.start_step("Биосфера: генерация агропотенциала, руд и биоресурсов...");
    compute_resources(topology, geology, climate, hydrology, config, seed);

    rebuild_river_flow_cache(topology, hydrology, config);
}

/// Записывает обратную связь от биомов, озёр, влажности почвы и рек обратно в ClimateLayer
pub fn apply_surface_feedback_to_climate(
    geology: &GeologyLayer,
    climate: &mut ClimateLayer,
    hydrology: &HydrologyLayer,
    c_config: &ClimateConfig,
    h_config: &HydrologyConfig,
) {
    let num_cells = geology.elevations.len();
    climate
        .surface_albedo
        .resize(num_cells, c_config.land_albedo);
    climate
        .surface_wetness
        .resize(num_cells, c_config.land_moisture_recycling);
    climate.surface_heat_capacity.resize(num_cells, 0.35);

    if geology.continentality.len() == num_cells {
        climate.continentality.clone_from(&geology.continentality);
    }

    let threshold = h_config.river_threshold.max(1e-6);

    for i in 0..num_cells {
        let biome = hydrology.biomes[i];

        // 1. Океан
        if geology.is_ocean_cell(i) {
            let is_ice = biome == Biome::SeaIce;
            climate.surface_albedo[i] = if is_ice { 0.60 } else { c_config.ocean_albedo };
            climate.surface_wetness[i] = if is_ice { 0.05 } else { 1.0 };
            climate.surface_heat_capacity[i] = if is_ice { 0.25 } else { 0.0 };
            continue;
        }

        // 2. Внутренние озёра и моря (Пресные, Солёные и Замёрзшие)
        if hydrology.is_lake_cell(i) {
            let is_ice = biome == Biome::LakeIce;
            let is_salt = hydrology.is_salt_lake[i];
            let size_cells = hydrology.lake_size[i] as f32;
            let size_factor = (size_cells / (num_cells as f32 * 0.006).max(1.0)).clamp(0.15, 1.0);

            climate.surface_albedo[i] = if is_ice {
                0.58
            } else if is_salt {
                (c_config.ocean_albedo + 0.05).min(0.16)
            } else {
                (c_config.ocean_albedo + 0.02).min(0.12)
            };

            climate.surface_wetness[i] = if is_ice {
                0.12
            } else if is_salt {
                0.82 // Солёная рапа испаряется чуть медленнее пресной воды
            } else {
                0.98
            };

            climate.surface_heat_capacity[i] = if is_ice {
                0.28
            } else {
                (0.55 + 0.40 * size_factor).clamp(0.55, 0.95)
            };

            // Крупные озёра (как Каспий или Байкал) локально смягчают континентальность
            if i < climate.continentality.len() {
                climate.continentality[i] *= 1.0 - 0.55 * size_factor;
            }
            continue;
        }

        // 3. Суша (включая сухие низменности с elev <= 0.0)
        let sm = hydrology.soil_moisture[i];
        let flow_ratio = (hydrology.flow_accumulation[i] / threshold).max(0.0);

        let (_, _, _, biome_albedo) = biome_satellite_rgb(biome);
        let moisture_darkening = sm * 0.035;
        let river_water_darkening = (flow_ratio.sqrt() * 0.015).clamp(0.0, 0.05);
        climate.surface_albedo[i] =
            (biome_albedo - moisture_darkening - river_water_darkening).clamp(0.08, 0.75);

        let vegetation_transpiration = match biome {
            Biome::TropicalRainforest | Biome::TemperateRainforest => 0.48 * sm.sqrt(),
            Biome::Wetland => 0.58,
            Biome::RiparianOasis
            | Biome::TropicalSeasonalForest
            | Biome::TemperateDeciduousForest
            | Biome::BorealTaiga => 0.36 * sm.sqrt(),
            Biome::Savanna | Biome::TemperateGrassland | Biome::MediterraneanShrubland => {
                0.24 * sm.sqrt()
            }
            Biome::Tundra => 0.18 * sm.sqrt(),
            _ => 0.08 * sm.sqrt(),
        };

        let river_evap = if flow_ratio >= 0.3 {
            (0.18 * flow_ratio.powf(0.40)).clamp(0.0, 0.55)
        } else {
            0.0
        };

        let raw_wetness = (sm * 0.55 + vegetation_transpiration + river_evap).clamp(0.08, 1.0);
        climate.surface_wetness[i] =
            (raw_wetness * 0.74 + c_config.land_moisture_recycling * 0.26).clamp(0.06, 1.0);

        let hc_base = match biome {
            Biome::Wetland => 0.92,
            Biome::TropicalRainforest | Biome::TemperateRainforest => 0.55,
            Biome::TemperateDeciduousForest
            | Biome::BorealTaiga
            | Biome::TropicalSeasonalForest => 0.48,
            Biome::RiparianOasis => 0.65,
            Biome::Savanna | Biome::TemperateGrassland => 0.35,
            Biome::MediterraneanShrubland | Biome::XericShrubland => 0.25,
            Biome::HotDesert | Biome::ColdDesert => 0.12,
            Biome::Tundra | Biome::AlpineBare => 0.18,
            Biome::IceSheet => 0.30,
            _ => 0.35,
        };
        climate.surface_heat_capacity[i] = (hc_base * (0.7 + 0.55 * sm)).clamp(0.08, 1.0);
    }
}

pub fn rebuild_river_flow_cache(
    topology: &WorldTopology,
    hydrology: &mut HydrologyLayer,
    config: &HydrologyConfig,
) {
    let num_cells = topology.cells.len();
    let threshold = config.river_threshold.max(1e-6);

    let mut strengths: Vec<f32> = hydrology
        .flow_accumulation
        .iter()
        .enumerate()
        .filter(|&(i, &f)| f >= threshold && !hydrology.is_lake_cell(i))
        .map(|(_, &f)| f / threshold)
        .collect();
    strengths.sort_by(|a, b| a.total_cmp(b));
    let max_strength = strengths
        .get(((strengths.len() as f32) * 0.90) as usize)
        .copied()
        .unwrap_or(1.0)
        .max(1.0);

    let mut cache = Vec::with_capacity(strengths.len());

    for i in 0..num_cells {
        if hydrology.is_lake_cell(i) {
            continue;
        }
        let flow = hydrology.flow_accumulation[i];
        if flow < threshold {
            continue;
        }
        let Some(k) = hydrology.flow_dir[i] else {
            continue;
        };
        let n_id = topology.cells[i].neighbors[k as usize];
        let cell_pos = topology.cells[i].center;
        let nbr_pos = topology.cells[n_id].center;

        let raw = nbr_pos - cell_pos;
        let dir = (raw - cell_pos * raw.dot(cell_pos)).normalize_or_zero();
        if dir.length_squared() < 1e-6 {
            continue;
        }

        let s = ((flow / threshold) / max_strength).clamp(0.0, 1.0).sqrt();
        cache.push((i, dir, s));
    }

    hydrology.river_flow_arrows = cache;
}
