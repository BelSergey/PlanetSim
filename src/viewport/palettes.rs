// src/viewport/palettes.rs

use bevy::color::ColorToComponents;
use bevy::prelude::*;
use rayon::prelude::*;

use crate::domains::{
    climate::climate_data::ClimateLayer,
    hydrology::{
        hydrology_data::{Biome, HydrologyLayer},
        hydrology_palettes::biome_satellite_rgb,
    },
    palettes::{LegendEntry, MapLegend, rgb_u8},
    tectonics::tectonics_data::GeologyLayer,
};

/// Композитная «спутниковая» палитра базового вида (SpaceView).
pub fn build_space_colors(
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &HydrologyLayer,
) -> Vec<[f32; 4]> {
    geology
        .elevations
        .par_iter()
        .enumerate()
        .map(|(i, &h)| {
            let albedo = climate.albedo[i];
            let biome = hydrology.biomes.get(i).copied().unwrap_or(Biome::DeepOcean);
            // Проверяем, настоящий ли это океан (а не просто сухая впадина ниже 0)
            let is_ocean = geology.is_ocean.get(i).copied().unwrap_or(h <= 0.0);

            let (base_r, base_g, base_b, base_albedo) = if is_ocean {
                if biome == Biome::SeaIce {
                    (0.88, 0.94, 0.99, 0.75)
                } else {
                    let depth = (h / -0.35).clamp(0.0, 1.0);
                    let r = 0.02 + (1.0 - depth) * 0.04;
                    let g = 0.10 + (1.0 - depth) * 0.18;
                    let b = 0.28 + (1.0 - depth) * 0.25;
                    (r, g, b, 0.06)
                }
            } else {
                let (r, g, b, b_alb) = biome_satellite_rgb(biome);
                let elev_shade = (1.0 - (h - 0.10).max(0.0) * 0.35).clamp(0.75, 1.0);
                (r * elev_shade, g * elev_shade, b * elev_shade, b_alb)
            };

            let snow_cover =
                ((albedo - base_albedo) / (0.80 - base_albedo).max(0.1)).clamp(0.0, 1.0);

            let r = base_r * (1.0 - snow_cover) + 0.92 * snow_cover;
            let g = base_g * (1.0 - snow_cover) + 0.96 * snow_cover;
            let b = base_b * (1.0 - snow_cover) + 1.00 * snow_cover;

            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

/// Легенда базового вида — по ключевым точкам композитной палитры.
pub fn space_view_legend() -> MapLegend {
    MapLegend {
        title: "Базовый вид (SpaceView)",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.03, 0.12, 0.30),
                label: "Глубокий океан".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.06, 0.28, 0.53),
                label: "Мелководье / шельф".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.88, 0.94, 0.99),
                label: "Морской лёд".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.94, 0.97, 1.00),
                label: "Ледниковый щит".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.07, 0.30, 0.10),
                label: "Тропические леса".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.20, 0.42, 0.16),
                label: "Умеренные леса".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.52, 0.58, 0.26),
                label: "Степи / саванны".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.84, 0.71, 0.46),
                label: "Пустыни".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.45, 0.41, 0.38),
                label: "Голые скалы / тундра".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.94, 0.97, 1.00),
                label: "Сезонный снежный покров".into(),
            },
        ],
    }
}
