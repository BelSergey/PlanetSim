// src/domains/hydrology/hydrology_palettes.rs

use crate::{
    domains::{
        palettes::{LegendEntry, MapLegend, rgb_u8},
        tectonics::tectonics_data::GeologyLayer,
    },
    viewport::{Globe, ViewportViewConfig},
    world::{WorldConfig, topology::WorldTopology},
};
use bevy::color::{Color, ColorToComponents};
use bevy::prelude::*;
use rayon::prelude::*;

use super::hydrology_data::{BioResource, Biome, HydrologyLayer, MineralResource, SoilType};

pub fn build_soil_moisture_colors(
    geology: &GeologyLayer,
    hydrology: &HydrologyLayer,
) -> Vec<[f32; 4]> {
    hydrology
        .soil_moisture
        .par_iter()
        .enumerate()
        .map(|(i, &sm)| {
            if hydrology.is_water_cell(geology, i) {
                return Color::srgb(0.06, 0.12, 0.24).to_linear().to_f32_array();
            }
            let m = sm.clamp(0.0, 1.0);
            let (r, g, b) = if m < 0.25 {
                let t = m / 0.25;
                (0.88 - t * 0.10, 0.68 + t * 0.10, 0.38 - t * 0.05)
            } else if m < 0.55 {
                let t = (m - 0.25) / 0.30;
                (0.78 - t * 0.55, 0.78 - t * 0.08, 0.33 + t * 0.12)
            } else {
                let t = (m - 0.55) / 0.45;
                (0.23 - t * 0.15, 0.70 - t * 0.35, 0.45 + t * 0.48)
            };
            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

pub fn build_soil_colors(hydrology: &HydrologyLayer) -> Vec<[f32; 4]> {
    hydrology
        .soils
        .par_iter()
        .map(|&soil| {
            let [r, g, b] = match soil {
                SoilType::Ocean => [0.08, 0.16, 0.30],
                SoilType::Leptosol => [0.52, 0.50, 0.48],
                SoilType::Gelisol => [0.72, 0.80, 0.84],
                SoilType::Podzol => [0.68, 0.64, 0.74],
                SoilType::Cambisol => [0.62, 0.46, 0.28],
                SoilType::Chernozem => [0.22, 0.18, 0.15],
                SoilType::Aridisol => [0.90, 0.80, 0.55],
                SoilType::Laterite => [0.82, 0.30, 0.18],
                SoilType::Andisol => [0.38, 0.22, 0.36],
                SoilType::Gleysol => [0.30, 0.52, 0.46],
            };
            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

pub fn build_biome_colors(hydrology: &HydrologyLayer) -> Vec<[f32; 4]> {
    hydrology
        .biomes
        .par_iter()
        .map(|&biome| {
            let [r, g, b] = match biome {
                // Океан
                Biome::DeepOcean => [0.05, 0.14, 0.32],
                Biome::ShallowOcean => [0.12, 0.34, 0.56],
                Biome::SeaIce => [0.85, 0.93, 0.98],

                // Внутренние озёра
                Biome::FreshwaterLake => [0.14, 0.48, 0.82],
                Biome::SaltLake => [0.16, 0.62, 0.68],
                Biome::LakeIce => [0.78, 0.90, 0.96],

                // Холодные и горные
                Biome::IceSheet => [0.95, 0.98, 1.00],
                Biome::AlpineBare => [0.52, 0.49, 0.47],
                Biome::Tundra => [0.62, 0.70, 0.60],
                Biome::BorealTaiga => [0.14, 0.36, 0.24],

                // Умеренные
                Biome::TemperateDeciduousForest => [0.24, 0.56, 0.22],
                Biome::TemperateRainforest => [0.10, 0.46, 0.32],
                Biome::MediterraneanShrubland => [0.58, 0.56, 0.24],
                Biome::TemperateGrassland => [0.66, 0.76, 0.32],

                // Аридные
                Biome::ColdDesert => [0.76, 0.70, 0.58],
                Biome::HotDesert => [0.93, 0.80, 0.48],
                Biome::XericShrubland => [0.82, 0.66, 0.42],

                // Тропические
                Biome::Savanna => [0.76, 0.72, 0.26],
                Biome::TropicalSeasonalForest => [0.18, 0.62, 0.20],
                Biome::TropicalRainforest => [0.05, 0.38, 0.12],

                // Гидрологические азональные
                Biome::Wetland => [0.22, 0.48, 0.44],
                Biome::RiparianOasis => [0.20, 0.74, 0.36],
            };
            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

/// Естественные спутниковые цвета биомов `(r, g, b, base_albedo)` для режима SpaceView ("Базовый")
#[inline]
pub fn biome_satellite_rgb(biome: Biome) -> (f32, f32, f32, f32) {
    match biome {
        Biome::DeepOcean | Biome::ShallowOcean => (0.03, 0.12, 0.30, 0.06),
        Biome::SeaIce => (0.88, 0.94, 0.99, 0.75),
        Biome::FreshwaterLake => (0.05, 0.24, 0.48, 0.08),
        Biome::SaltLake => (0.08, 0.34, 0.46, 0.12),
        Biome::LakeIce => (0.84, 0.92, 0.98, 0.70),
        Biome::IceSheet => (0.94, 0.97, 1.00, 0.80),
        Biome::AlpineBare => (0.45, 0.41, 0.38, 0.28),
        Biome::Tundra => (0.48, 0.52, 0.40, 0.26),
        Biome::BorealTaiga => (0.12, 0.28, 0.16, 0.15),
        Biome::TemperateDeciduousForest => (0.20, 0.42, 0.16, 0.18),
        Biome::TemperateRainforest => (0.10, 0.34, 0.20, 0.15),
        Biome::MediterraneanShrubland => (0.46, 0.45, 0.22, 0.22),
        Biome::TemperateGrassland => (0.52, 0.58, 0.26, 0.24),
        Biome::ColdDesert => (0.68, 0.62, 0.50, 0.32),
        Biome::HotDesert => (0.84, 0.71, 0.46, 0.38),
        Biome::XericShrubland => (0.72, 0.58, 0.38, 0.30),
        Biome::Savanna => (0.58, 0.56, 0.24, 0.24),
        Biome::TropicalSeasonalForest => (0.15, 0.44, 0.14, 0.16),
        Biome::TropicalRainforest => (0.07, 0.30, 0.10, 0.13),
        Biome::Wetland => (0.18, 0.38, 0.30, 0.16),
        Biome::RiparianOasis => (0.18, 0.50, 0.22, 0.18),
    }
}

pub fn build_food_potential_colors(
    geology: &GeologyLayer,
    hydrology: &HydrologyLayer,
) -> Vec<[f32; 4]> {
    geology
        .elevations
        .par_iter()
        .enumerate()
        .map(|(i, _)| {
            let fish = hydrology.fishery_potential.get(i).copied().unwrap_or(0.0);
            if hydrology.is_water_cell(geology, i) {
                let r = 0.04 + fish * 0.14;
                let g = 0.12 + fish * 0.58;
                let b = 0.26 + fish * 0.64;
                return Color::srgb(r, g, b).to_linear().to_f32_array();
            }

            let arable = hydrology.arable_potential.get(i).copied().unwrap_or(0.0);
            let pastoral = hydrology.pastoral_potential.get(i).copied().unwrap_or(0.0);
            let foraging = hydrology.foraging_potential.get(i).copied().unwrap_or(0.0);

            let base_r = 0.24;
            let base_g = 0.22;
            let base_b = 0.20;

            let r = (base_r + pastoral * 0.68 + foraging * 0.32 - arable * 0.10).clamp(0.05, 1.0);
            let g = (base_g + arable * 0.72 + pastoral * 0.38 + foraging * 0.20).clamp(0.05, 1.0);
            let b = (base_b + foraging * 0.58 + fish * 0.35 - pastoral * 0.12).clamp(0.05, 1.0);

            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

pub fn build_mineral_colors(geology: &GeologyLayer, hydrology: &HydrologyLayer) -> Vec<[f32; 4]> {
    geology
        .elevations
        .par_iter()
        .enumerate()
        .map(|(i, &elev)| {
            let res = hydrology
                .minerals
                .get(i)
                .copied()
                .unwrap_or(MineralResource::None);
            let richness = hydrology
                .mineral_richness
                .get(i)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);

            let bg = if hydrology.is_water_cell(geology, i) {
                [0.06, 0.11, 0.20]
            } else {
                let shade = (0.18 + elev.clamp(0.0, 0.5) * 0.25).clamp(0.16, 0.34);
                [shade, shade * 0.98, shade * 0.95]
            };

            if res == MineralResource::None {
                return Color::srgb(bg[0], bg[1], bg[2]).to_linear().to_f32_array();
            }

            let fg = match res {
                MineralResource::None => bg,
                MineralResource::Clay => [0.76, 0.48, 0.30],
                MineralResource::StoneLimestone => [0.78, 0.80, 0.82],
                MineralResource::SilicaQuartz => [0.60, 0.88, 0.92],
                MineralResource::Salt => [0.96, 0.96, 1.00],
                MineralResource::Copper => [0.90, 0.45, 0.15],
                MineralResource::Tin => [0.55, 0.68, 0.78],
                MineralResource::Iron => [0.78, 0.18, 0.16],
                MineralResource::PreciousMetals => [1.00, 0.86, 0.12],
                MineralResource::SulfurNitre => [0.92, 0.95, 0.18],
                MineralResource::Coal => [0.12, 0.12, 0.14],
                MineralResource::Hydrocarbons => [0.52, 0.12, 0.68],
                MineralResource::Bauxite => [0.94, 0.36, 0.32],
                MineralResource::AlloyMetals => [0.24, 0.52, 0.92],
                MineralResource::Uranium => [0.18, 0.98, 0.28],
                MineralResource::RareEarths => [0.98, 0.18, 0.75],
            };

            let t = (0.45 + 0.55 * richness).clamp(0.0, 1.0);
            let r = bg[0] * (1.0 - t) + fg[0] * t;
            let g = bg[1] * (1.0 - t) + fg[1] * t;
            let b = bg[2] * (1.0 - t) + fg[2] * t;
            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

pub fn build_bio_resource_colors(
    geology: &GeologyLayer,
    hydrology: &HydrologyLayer,
) -> Vec<[f32; 4]> {
    geology
        .elevations
        .par_iter()
        .enumerate()
        .map(|(i, &elev)| {
            let res = hydrology
                .bio_resources
                .get(i)
                .copied()
                .unwrap_or(BioResource::None);
            let richness = hydrology
                .bio_richness
                .get(i)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);

            let bg = if hydrology.is_water_cell(geology, i) {
                [0.06, 0.12, 0.22]
            } else {
                let shade = (0.20 + elev.clamp(0.0, 0.5) * 0.20).clamp(0.18, 0.32);
                [shade * 0.95, shade, shade * 0.92]
            };

            if res == BioResource::None {
                return Color::srgb(bg[0], bg[1], bg[2]).to_linear().to_f32_array();
            }

            let fg = match res {
                BioResource::None => bg,
                BioResource::NavalTimber => [0.08, 0.48, 0.22],
                BioResource::TimberFuel => [0.36, 0.58, 0.28],
                BioResource::FiberCrops => [0.94, 0.94, 0.86],
                BioResource::ResinRubber => [0.16, 0.78, 0.72],
                BioResource::LuxuryFlora => [0.88, 0.20, 0.52],
                BioResource::Horses => [0.82, 0.48, 0.16],
                BioResource::Camels => [0.94, 0.80, 0.36],
                BioResource::ElephantsDraft => [0.56, 0.46, 0.68],
                BioResource::FurIvory => [0.68, 0.86, 0.98],
            };

            let t = (0.45 + 0.55 * richness).clamp(0.0, 1.0);
            let r = bg[0] * (1.0 - t) + fg[0] * t;
            let g = bg[1] * (1.0 - t) + fg[1] * t;
            let b = bg[2] * (1.0 - t) + fg[2] * t;
            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

pub(super) fn draw_river_flow_gizmos(
    mut gizmos: Gizmos,
    topology: Res<WorldTopology>,
    geology: Res<GeologyLayer>,
    hydrology: Res<HydrologyLayer>,
    view_config: Res<ViewportViewConfig>,
    world_config: Res<WorldConfig>,
    globe_query: Query<&Transform, With<Globe>>,
) {
    if !view_config.show_river_flow || hydrology.river_flow_arrows.is_empty() {
        return;
    }
    let Ok(globe_transform) = globe_query.single() else {
        return;
    };

    let r = world_config.radius;
    let height_scale = world_config.height_scale();
    let base_len = r * 0.002;

    for &(i, dir, s) in &hydrology.river_flow_arrows {
        let cell_pos = topology.cells[i].center;
        let real_h = geology.elevations[i];
        let elev = if view_config.flat_water && geology.is_ocean_cell(i) {
            real_h.max(0.0)
        } else {
            real_h
        };

        let local_start = cell_pos * (r + elev * height_scale + r * 0.005);
        let arrow_len = base_len;
        let local_end = local_start + dir * arrow_len;

        let world_start = globe_transform.transform_point(local_start);
        let world_end = globe_transform.transform_point(local_end);

        let alpha = 0.35 + 0.55 * s;
        let color = Color::srgb(1.0, 0.70 + 0.15 * s, 0.30 + 0.15 * s).with_alpha(alpha);

        gizmos
            .arrow(world_start, world_end, color)
            .with_tip_length(arrow_len * 2.0);
    }
}

// ==== ЛЕГЕНДЫ БИОСФЕРЫ И ПОВЕРХНОСТИ ====

pub fn soil_moisture_legend() -> MapLegend {
    MapLegend {
        title: "Влажность почвы",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.06, 0.12, 0.24),
                label: "Океан / озеро".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.88, 0.68, 0.38),
                label: "Сухая  (0.00)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.78, 0.78, 0.33),
                label: "Умеренно сухая  (0.25)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.23, 0.70, 0.45),
                label: "Умеренно влажная  (0.55)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.08, 0.35, 0.93),
                label: "Насыщенная  (1.00)".into(),
            },
        ],
    }
}

pub fn soils_legend() -> MapLegend {
    MapLegend {
        title: "Типы почв",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.08, 0.16, 0.30),
                label: "Океан".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.52, 0.50, 0.48),
                label: "Лептосоль (каменистая)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.72, 0.80, 0.84),
                label: "Гелисоль (мерзлотная)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.68, 0.64, 0.74),
                label: "Подзол (хвойные леса)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.62, 0.46, 0.28),
                label: "Камбисоль (бурая)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.22, 0.18, 0.15),
                label: "Чернозём".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.90, 0.80, 0.55),
                label: "Аридосоль (пустынная)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.82, 0.30, 0.18),
                label: "Латерит (тропическая)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.38, 0.22, 0.36),
                label: "Андисоль (вулканическая)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.30, 0.52, 0.46),
                label: "Глейсоль (болотная / озёрная)".into(),
            },
        ],
    }
}

pub fn biomes_legend() -> MapLegend {
    MapLegend {
        title: "Биомы",
        entries: vec![
            // Океан и Озёра
            LegendEntry {
                rgb: rgb_u8(0.05, 0.14, 0.32),
                label: "Глубокий океан".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.12, 0.34, 0.56),
                label: "Мелководный океан".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.85, 0.93, 0.98),
                label: "Морской лёд".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.14, 0.48, 0.82),
                label: "Пресное озеро (проточное)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.16, 0.62, 0.68),
                label: "Солёное озеро (бессточное)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.78, 0.90, 0.96),
                label: "Замёрзшее озеро".into(),
            },
            // Холодные / горные
            LegendEntry {
                rgb: rgb_u8(0.95, 0.98, 1.00),
                label: "Ледниковый щит".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.52, 0.49, 0.47),
                label: "Альпийские голые породы".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.62, 0.70, 0.60),
                label: "Тундра".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.14, 0.36, 0.24),
                label: "Бореальная тайга".into(),
            },
            // Умеренные
            LegendEntry {
                rgb: rgb_u8(0.24, 0.56, 0.22),
                label: "Широколиственный лес".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.10, 0.46, 0.32),
                label: "Умеренный дождевой лес".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.58, 0.56, 0.24),
                label: "Средиземноморские кустарники".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.66, 0.76, 0.32),
                label: "Умеренные степи".into(),
            },
            // Аридные
            LegendEntry {
                rgb: rgb_u8(0.76, 0.70, 0.58),
                label: "Холодная пустыня".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.93, 0.80, 0.48),
                label: "Жаркая пустыня".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.82, 0.66, 0.42),
                label: "Ксерофитные кустарники".into(),
            },
            // Тропические
            LegendEntry {
                rgb: rgb_u8(0.76, 0.72, 0.26),
                label: "Саванна".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.18, 0.62, 0.20),
                label: "Тропический сезонный лес".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.05, 0.38, 0.12),
                label: "Тропический дождевой лес".into(),
            },
            // Азональные
            LegendEntry {
                rgb: rgb_u8(0.22, 0.48, 0.44),
                label: "Водно-болотные угодья".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.20, 0.74, 0.36),
                label: "Пойма / оазис".into(),
            },
        ],
    }
}

pub fn food_potential_legend() -> MapLegend {
    MapLegend {
        title: "Продовольственный потенциал",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.24, 0.22, 0.20),
                label: "Бесплодные земли".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.10, 0.90, 0.20),
                label: "Пашня (изумруд)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.90, 0.75, 0.20),
                label: "Пастбища (охра)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.60, 0.40, 0.85),
                label: "Собирательство / дичь".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.05, 0.40, 0.90),
                label: "Рыболовство (шельф / озёра)".into(),
            },
        ],
    }
}

pub fn minerals_legend() -> MapLegend {
    MapLegend {
        title: "Полезные ископаемые",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.76, 0.48, 0.30),
                label: "Глина".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.78, 0.80, 0.82),
                label: "Камень / известняк".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.60, 0.88, 0.92),
                label: "Кремнезём / кварц".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.96, 0.96, 1.00),
                label: "Соль".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.90, 0.45, 0.15),
                label: "Медь".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.55, 0.68, 0.78),
                label: "Олово".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.78, 0.18, 0.16),
                label: "Железо".into(),
            },
            LegendEntry {
                rgb: rgb_u8(1.00, 0.86, 0.12),
                label: "Драгоценные металлы".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.92, 0.95, 0.18),
                label: "Сера / селитра".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.12, 0.12, 0.14),
                label: "Уголь".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.52, 0.12, 0.68),
                label: "Углеводороды".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.94, 0.36, 0.32),
                label: "Бокситы".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.24, 0.52, 0.92),
                label: "Легирующие металлы".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.18, 0.98, 0.28),
                label: "Уран".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.98, 0.18, 0.75),
                label: "РЗМ / литий".into(),
            },
        ],
    }
}

pub fn bio_resources_legend() -> MapLegend {
    MapLegend {
        title: "Биоресурсы",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.08, 0.48, 0.22),
                label: "Корабельный лес".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.36, 0.58, 0.28),
                label: "Строительный лес / топливо".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.94, 0.94, 0.86),
                label: "Волокнистые культуры".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.16, 0.78, 0.72),
                label: "Смолы / каучук".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.88, 0.20, 0.52),
                label: "Пряности / чай / кофе".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.82, 0.48, 0.16),
                label: "Лошади".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.94, 0.80, 0.36),
                label: "Верблюды".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.56, 0.46, 0.68),
                label: "Слоны / вьючные".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.68, 0.86, 0.98),
                label: "Пушнина / кость".into(),
            },
        ],
    }
}
