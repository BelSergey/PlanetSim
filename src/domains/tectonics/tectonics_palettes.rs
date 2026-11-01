// src/domains/tectonics/tectonics_palettes.rs

use super::tectonics_data::{GeologyLayer, RockType};
use crate::domains::palettes::{rgb_u8, LegendEntry, MapLegend};
use bevy::color::{Color, ColorToComponents};
use bevy::prelude::*;

pub fn build_lithology_colors(geology: &GeologyLayer) -> Vec<[f32; 4]> {
    let has_ocean_mask = geology.is_ocean.len() == geology.lithology.len();

    geology
        .lithology
        .iter()
        .enumerate()
        .map(|(i, &rock)| {
            let is_ocean = if has_ocean_mask {
                geology.is_ocean[i]
            } else {
                geology.elevations[i] <= 0.0
            };

            let base_rgb = match rock {
                RockType::Granite => [0.92, 0.48, 0.58],
                RockType::GneissSchist => [0.64, 0.44, 0.74],
                RockType::Andesite => [0.88, 0.32, 0.18],
                RockType::FloodBasalt => [0.28, 0.25, 0.30],
                RockType::Sandstone => [0.88, 0.76, 0.44],
                RockType::ShaleClay => [0.52, 0.62, 0.45],
                RockType::Limestone => [0.48, 0.84, 0.82],
                RockType::OceanicBasalt => [0.16, 0.22, 0.34],
                RockType::PelagicSediment => [0.26, 0.36, 0.48],
            };

            let shade = if is_ocean { 0.72 } else { 1.0 };
            Color::srgb(
                base_rgb[0] * shade,
                base_rgb[1] * shade,
                base_rgb[2] * shade,
            )
            .to_linear()
            .to_f32_array()
        })
        .collect()
}

/// Генерирует массив цветов по высотам рельефа.
/// Не-океанические впадины ниже уровня моря (`!is_ocean[i] && elev <= 0.0`) красятся как суша!
pub fn build_elevation_colors(geology: &GeologyLayer) -> Vec<[f32; 4]> {
    let has_ocean_mask = geology.is_ocean.len() == geology.elevations.len();

    geology
        .elevations
        .iter()
        .enumerate()
        .map(|(i, &h)| {
            let is_ocean = if has_ocean_mask {
                geology.is_ocean[i]
            } else {
                h <= 0.0
            };
            let effective_h = if !is_ocean && h <= 0.0 {
                0.015 // Сухая низменность ниже уровня моря красится как равнинная суша
            } else {
                h
            };
            elevation_to_color(effective_h).to_linear().to_f32_array()
        })
        .collect()
}

/// Генерирует массив цветов для режима просмотра тектонических плит:
/// каждая плита имеет свой оттенок `color_hue`, при этом континентальная кора плиты светлее,
/// а океаническая часть той же плиты — темнее.
pub fn build_plate_colors(geology: &GeologyLayer) -> Vec<[f32; 4]> {
    let has_density = geology.crust_density.len() == geology.plate_ids.len();

    geology
        .plate_ids
        .iter()
        .enumerate()
        .map(|(i, &plate_id)| {
            if geology.volcano_mask.get(i).copied().unwrap_or(false) {
                return Color::srgb(0.10, 0.10, 0.10).to_linear().to_f32_array();
            }

            let Some(plate) = geology.plates.get(plate_id) else {
                return Color::srgb(1.0, 1.0, 1.0).to_linear().to_f32_array();
            };

            let is_cont = if has_density {
                geology.is_continental_crust(i)
            } else {
                geology.elevations[i] > 0.0
            };

            let hue = plate.color_hue;
            let (sat, light) = if is_cont {
                (0.72, 0.46) // Континентальная часть плиты (яркая)
            } else {
                (0.58, 0.22) // Океаническая часть той же плиты (темная)
            };

            Color::hsl(hue, sat, light).to_linear().to_f32_array()
        })
        .collect()
}

pub fn elevation_to_color(h: f32) -> Color {
    match h {
        x if x <= -0.44 => Color::srgb(0.05, 0.02, 0.01),
        x if x <= -0.16 => Color::srgb(0.15, 0.08, 0.04),
        x if x < -0.02 => Color::srgb(0.30, 0.17, 0.09),
        x if x < 0.01 => Color::srgb(0.50, 0.32, 0.17),
        x if x < 0.12 => Color::srgb(0.46, 0.29, 0.15),
        x if x < 0.32 => Color::srgb(0.62, 0.40, 0.20),
        x if x < 0.50 => Color::srgb(0.78, 0.46, 0.19),
        x if x < 0.64 => Color::srgb(0.87, 0.33, 0.12),
        _ => Color::srgb(0.95, 0.18, 0.10),
    }
}

pub fn build_ruggedness_colors(geology: &GeologyLayer) -> Vec<[f32; 4]> {
    let has_ocean_mask = geology.is_ocean.len() == geology.terrain_ruggedness.len();

    geology
        .terrain_ruggedness
        .iter()
        .enumerate()
        .map(|(i, &rug)| {
            let is_ocean = if has_ocean_mask {
                geology.is_ocean[i]
            } else {
                geology.elevations[i] <= 0.0
            };
            let r_val = rug.clamp(0.0, 1.0);

            if is_ocean {
                let r = 0.04 + r_val * 0.18;
                let g = 0.10 + r_val * 0.28;
                let b = 0.22 + r_val * 0.35;
                return Color::srgb(r, g, b).to_linear().to_f32_array();
            }

            let (r, g, b) = if r_val < 0.25 {
                let t = r_val / 0.25;
                (0.18 + t * 0.52, 0.56 + t * 0.16, 0.24 - t * 0.06)
            } else if r_val < 0.60 {
                let t = (r_val - 0.25) / 0.35;
                (0.70 + t * 0.22, 0.72 - t * 0.42, 0.18 - t * 0.06)
            } else {
                let t = (r_val - 0.60) / 0.40;
                (0.92 - t * 0.05, 0.30 - t * 0.10 + t * 0.55, 0.12 + t * 0.78)
            };

            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

/// Дополнительная карта: Возраст и тип коры (красные молодые СОХ -> синяя старая абиссаль, золото — кратоны)
pub fn build_crust_age_colors(geology: &GeologyLayer) -> Vec<[f32; 4]> {
    geology
        .crust_age
        .iter()
        .enumerate()
        .map(|(i, &age)| {
            if geology.is_continental_crust(i) {
                let t = ((age - 0.5) / 2.5).clamp(0.0, 1.0);
                Color::srgb(0.62 + t * 0.22, 0.48 + t * 0.18, 0.28)
                    .to_linear()
                    .to_f32_array()
            } else {
                let t = (age / 1.2).clamp(0.0, 1.0);
                // От огненно-оранжевого (молодой СОХ) через зеленый к темно-синему (старая абиссаль)
                Color::hsl(12.0 + t * 215.0, 0.80, 0.48 - t * 0.24)
                    .to_linear()
                    .to_f32_array()
            }
        })
        .collect()
}

/// Дополнительная карта: Тектоническое напряжение (синий — спрединг/рифты, красный — субдукция/коллизия)
pub fn build_tectonic_stress_colors(geology: &GeologyLayer) -> Vec<[f32; 4]> {
    geology
        .tectonic_stress
        .iter()
        .map(|&s| {
            let val = s.clamp(-1.0, 1.0);
            if val < 0.0 {
                let t = (-val).clamp(0.0, 1.0);
                Color::srgb(0.18 * (1.0 - t), 0.25 + 0.60 * t, 0.30 + 0.70 * t)
                    .to_linear()
                    .to_f32_array()
            } else {
                let t = val.clamp(0.0, 1.0);
                Color::srgb(0.22 + 0.75 * t, 0.22 * (1.0 - t * 0.5), 0.22 * (1.0 - t))
                    .to_linear()
                    .to_f32_array()
            }
        })
        .collect()
}

// ==== ЛЕГЕНДЫ КАРТЫ ====

pub fn elevation_legend() -> MapLegend {
    MapLegend {
        title: "Рельеф",
        entries: vec![
            LegendEntry { rgb: rgb_u8(0.05, 0.02, 0.01), label: "Абиссаль  (≤ −0.44)".into() },
            LegendEntry { rgb: rgb_u8(0.15, 0.08, 0.04), label: "Глубокий океан  (−0.44…−0.16)".into() },
            LegendEntry { rgb: rgb_u8(0.30, 0.17, 0.09), label: "Океан  (−0.16…−0.02)".into() },
            LegendEntry { rgb: rgb_u8(0.50, 0.32, 0.17), label: "Шельф / берег  (~0)".into() },
            LegendEntry { rgb: rgb_u8(0.46, 0.29, 0.15), label: "Равнины и впадины суши".into() },
            LegendEntry { rgb: rgb_u8(0.62, 0.40, 0.20), label: "Возвышенности  (0.12…0.32)".into() },
            LegendEntry { rgb: rgb_u8(0.78, 0.46, 0.19), label: "Предгорья  (0.32…0.50)".into() },
            LegendEntry { rgb: rgb_u8(0.87, 0.33, 0.12), label: "Горы  (0.50…0.64)".into() },
            LegendEntry { rgb: rgb_u8(0.95, 0.18, 0.10), label: "Пики  (≥ 0.64)".into() },
        ],
    }
}

pub fn lithology_legend() -> MapLegend {
    let rocks: &[(RockType, &str, [f32; 3])] = &[
        (RockType::Granite,         "Гранит",               [0.92, 0.48, 0.58]),
        (RockType::GneissSchist,    "Гнейсы / сланцы",      [0.64, 0.44, 0.74]),
        (RockType::Andesite,        "Андезит",              [0.88, 0.32, 0.18]),
        (RockType::FloodBasalt,     "Траппы (базальт)",     [0.28, 0.25, 0.30]),
        (RockType::Sandstone,       "Песчаник",             [0.88, 0.76, 0.44]),
        (RockType::ShaleClay,       "Глины / сланцы",       [0.52, 0.62, 0.45]),
        (RockType::Limestone,       "Известняк",            [0.48, 0.84, 0.82]),
        (RockType::OceanicBasalt,   "Океанический базальт", [0.16, 0.22, 0.34]),
        (RockType::PelagicSediment, "Пелагические илы",     [0.26, 0.36, 0.48]),
    ];
    let _ = RockType::Granite;
    MapLegend {
        title: "Литология",
        entries: rocks
            .iter()
            .map(|(_, label, rgb)| LegendEntry {
                rgb: rgb_u8(rgb[0], rgb[1], rgb[2]),
                label: (*label).into(),
            })
            .collect(),
    }
}

pub fn plates_legend() -> MapLegend {
    MapLegend {
        title: "Тектонические плиты",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.75, 0.48, 0.22),
                label: "Континентальная кора плиты (светлый тон)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.18, 0.26, 0.36),
                label: "Океаническая кора той же плиты (темный тон)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.10, 0.10, 0.10),
                label: "Цепь вулканов горячей точки".into(),
            },
        ],
    }
}

pub fn ruggedness_legend() -> MapLegend {
    MapLegend {
        title: "Рельефность",
        entries: vec![
            LegendEntry { rgb: rgb_u8(0.18, 0.56, 0.24), label: "Гладкие равнины  (0.00)".into() },
            LegendEntry { rgb: rgb_u8(0.70, 0.72, 0.18), label: "Холмы и овраги  (0.25)".into() },
            LegendEntry { rgb: rgb_u8(0.92, 0.30, 0.12), label: "Скалистые хребты  (0.60)".into() },
            LegendEntry { rgb: rgb_u8(0.87, 0.75, 0.90), label: "Пики и каньоны  (1.00)".into() },
        ],
    }
}

/// Легенда возраста и типа коры — от молодых рифтов СОХ до старой абиссали и древних кратонов.
pub fn crust_age_legend() -> MapLegend {
    MapLegend {
        title: "Возраст и тип коры",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.86, 0.25, 0.10),
                label: "Молодая кора СОХ (0 млн лет)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.55, 0.72, 0.12),
                label: "Зрелая океаническая кора".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.08, 0.45, 0.52),
                label: "Остывшая океаническая кора".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.05, 0.14, 0.43),
                label: "Древняя абиссаль (тяжёлый слэб)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.62, 0.48, 0.28),
                label: "Молодая континентальная кора".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.84, 0.66, 0.28),
                label: "Древний континентальный кратон".into(),
            },
        ],
    }
}

/// Легенда тектонического напряжения на границах плит.
pub fn tectonic_stress_legend() -> MapLegend {
    MapLegend {
        title: "Напряжение границ",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.00, 0.85, 1.00),
                label: "Сильный спрединг / рифт (−1.0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.09, 0.55, 0.65),
                label: "Умеренное растяжение (−0.5)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.20, 0.22, 0.25),
                label: "Спокойная плита (0.0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.60, 0.16, 0.11),
                label: "Зона сжатия / субдукции (+0.5)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.97, 0.11, 0.00),
                label: "Жёсткая коллизия (+1.0)".into(),
            },
        ],
    }
}