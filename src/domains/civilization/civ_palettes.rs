// src/domains/civilization/civ_palettes.rs

use bevy::color::{Color, ColorToComponents};

use super::{
    civ_config::CivilizationConfig,
    civ_data::{CivilizationLayer, SocLayerKind},
};
use crate::domains::{
    palettes::{LegendEntry, MapLegend, rgb_u8},
    tectonics::tectonics_data::GeologyLayer,
};

/// Превращает произвольный entity_id в яркий, контрастный цвет на сфере
#[inline]
fn entity_id_to_rgb(entity_id: u32) -> [f32; 3] {
let mut h = entity_id.wrapping_mul(0x45d9_f3b);
h = ((h >> 16) ^ h).wrapping_mul(0x45d9_f3b);
    h = (h >> 16) ^ h;

    let hue = (h & 0xFFFF) as f32 / 65535.0 * 360.0;
    let sat = 0.65 + ((h >> 16) & 0xFF) as f32 / 255.0 * 0.25;
    let val = 0.74 + ((h >> 24) & 0xFF) as f32 / 255.0 * 0.22;

    let c = Color::hsv(hue, sat, val).to_srgba();
    [c.red, c.green, c.blue]
}

/// Строит цвета глобуса для конкретного слоя института (0..6)
pub fn build_civ_layer_colors(
    geology: &GeologyLayer,
    civ: &CivilizationLayer,
    config: &CivilizationConfig,
    layer: SocLayerKind,
) -> Vec<[f32; 4]> {
    let num_cells = geology.elevations.len();
    if civ.cells.len() != num_cells {
        return vec![[0.1, 0.1, 0.1, 1.0]; num_cells];
    }

    let k = layer.index();
    let frac_thresh = config.specs[k].fracture_threshold.max(0.15);

    geology
        .elevations
        .iter()
        .enumerate()
        .map(|(i, &elev)| {
            if elev <= 0.0 {
                let depth = (elev / -0.35).clamp(0.0, 1.0);
                return Color::srgb(
                    0.04 * (1.0 - depth * 0.5),
                    0.11 * (1.0 - depth * 0.5),
                    0.24 * (1.0 - depth * 0.4),
                )
                .to_linear()
                .to_f32_array();
            }

            let cell = &civ.cells[i];
            let shade = (0.20 + elev.clamp(0.0, 0.45) * 0.22).clamp(0.18, 0.34);
            let bg = [shade, shade * 0.98, shade * 0.94];

            let ent_id = cell.entity_id[k];
            let s = cell.strength[k].clamp(0.0, 1.0);
            if ent_id == 0 || s < 0.03 {
                return Color::srgb(bg[0], bg[1], bg[2]).to_linear().to_f32_array();
            }

            let fg = entity_id_to_rgb(ent_id);

            // Проверяем, граничит ли клетка с другой плитой в этом слое
            let mut is_border = false;
            if let Some(neighbors) = civ.flat_neighbors.get(i) {
                for &n_u32 in neighbors {
                    if n_u32 == u32::MAX {
                        break;
                    }
                    let n_idx = n_u32 as usize;
                    if geology.elevations[n_idx] > 0.0 && civ.cells[n_idx].entity_id[k] != ent_id {
                        is_border = true;
                        break;
                    }
                }
            }

            // Яркость заливки зависит от силы/плотности института в клетке
            let alpha = (0.42 + 0.58 * s).clamp(0.0, 1.0);
            let mut r = bg[0] * (1.0 - alpha) + fg[0] * alpha;
            let mut g = bg[1] * (1.0 - alpha) + fg[1] * alpha;
            let mut b = bg[2] * (1.0 - alpha) + fg[2] * alpha;

            // Спокойная граница между плитами затемняется контуром
            if is_border {
                r *= 0.72;
                g *= 0.72;
                b *= 0.72;
            }

            // Очаги критического напряжения (войны, восстания, расколы) подсвечиваются алым жаром
            let stress_ratio = (cell.stress[k] / frac_thresh).clamp(0.0, 1.0);
            if stress_ratio > 0.75 {
                let heat = ((stress_ratio - 0.75) / 0.25).clamp(0.0, 1.0) * 0.65;
                r = r * (1.0 - heat) + 0.98 * heat;
                g = g * (1.0 - heat) + 0.18 * heat;
                b = b * (1.0 - heat) + 0.10 * heat;
            }

            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
}

// ==== ЛЕГЕНДЫ КАРТЫ ДЛЯ 7 СЛОЕВ ИНСТИТУТОВ ====

fn make_inst_legend(
    title: &'static str,
    entity_desc: &'static str,
    core_desc: &'static str,
) -> MapLegend {
    MapLegend {
        title,
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.28, 0.68, 0.88),
                label: entity_desc.into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.85, 0.75, 0.25),
                label: core_desc.into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.18, 0.42, 0.55),
                label: "Граница между сущностями (тёмный контур)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.95, 0.20, 0.12),
                label: "Очаг конфликта / войны / раскола (высокий стресс)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.22, 0.21, 0.20),
                label: "Незаселённая суша (институт отсутствует)".into(),
            },
        ],
    }
}

pub fn civ_species_legend() -> MapLegend {
    make_inst_legend(
        "0. Расы и Популяция",
        "Ареал вида / расы (цвет = ID вида)",
        "Густонаселённое ядро (высокая ёмкость среды)",
    )
}

pub fn civ_kinship_legend() -> MapLegend {
    make_inst_legend(
        "1. Род и Кланы",
        "Территория родового племени / клана",
        "Сплочённый родовой центр (до вытеснения гос-вом)",
    )
}

pub fn civ_culture_legend() -> MapLegend {
    make_inst_legend(
        "2. Культуры и Этносы",
        "Ареал культуры (цвет = этноязыковая плита)",
        "Культурная метрополия (высокая традиция)",
    )
}

pub fn civ_religion_legend() -> MapLegend {
    make_inst_legend(
        "3. Мифы и Религии",
        "Конфессиональная зона / культ (цвет = ID веры)",
        "Священный центр / оплот догмы",
    )
}

pub fn civ_economy_legend() -> MapLegend {
    make_inst_legend(
        "4. Хозяйство и Рынки",
        "Торгово-экономическая сеть (общий пул ресурсов)",
        "Промышленно-торговый узел (высокий обмен)",
    )
}

pub fn civ_polity_legend() -> MapLegend {
    make_inst_legend(
        "5. Власть и Государства",
        "Государство / держава (цвет = политический ID)",
        "Столичный регион (высокая власть и стабильность)",
    )
}

pub fn civ_knowledge_legend() -> MapLegend {
    make_inst_legend(
        "6. Знание и Наука",
        "Научно-технологическая школа (сеть знаний)",
        "Академический центр (максимум открытых идей)",
    )
}
