// src/domains/tectonics/tectonics_data.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum PlateType {
    Oceanic,
    Continental,
}

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct Plate {
    pub center_dir: Vec3,
    pub plate_type: PlateType,
    pub euler_pole: Vec3,
    pub angular_vel: f32,
}

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct Volcano {
    pub center: Vec3,
    pub height: f32,
    pub radius: f32,
}

/// Литологические классы материнских пород
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum RockType {
    #[default]
    OceanicBasalt, // Океанический базальт (MORB, молодая океаническая кора)
    PelagicSediment, // Глубоководные осадки (кремнистые/глинистые илы абиссали и желобов)
    Granite,         // Кислые интрузии (ядра коллизии и батолиты континентальных дуг)
    GneissSchist, // Метаморфиты (гнейсы, кристалланские сланцы складчатых поясов и древних щитов)
    Andesite,     // Средние/кислые вулканиты (островные и континентальные дуги, стратовулканы)
    FloodBasalt,  // Континентальные и островные базальты (рифты, траппы, щитовые вулканы плюмов)
    Sandstone, // Обломочные осадочные (песчаники предгорных прогибов, пассивных окраин и платформ)
    ShaleClay, // Глинистые осадочные (аргиллиты, глинистые сланцы низменностей и древних морей)
    Limestone, // Карбонатные породы (известняки и доломиты теплых шельфов и платформ)
}

impl RockType {
    /// Базовая текстура выветривания породы: `[sand, silt, clay]` (в сумме = 1.0) для модели почв CLORPT
    #[inline]
    pub fn base_texture(self) -> [f32; 3] {
        match self {
            RockType::Granite => [0.65, 0.20, 0.15], // Крупнозернистый кварцевый песок, мало глины
            RockType::Sandstone => [0.75, 0.15, 0.10], // Очень песчаная, легкая почва
            RockType::GneissSchist => [0.45, 0.35, 0.20], // Супесь / легкий суглинок
            RockType::Andesite => [0.35, 0.45, 0.20], // Пылеватый суглинок (вулканический пепел/ил)
            RockType::Limestone => [0.25, 0.45, 0.30], // Карбонатный суглинок
            RockType::FloodBasalt | RockType::OceanicBasalt => [0.20, 0.35, 0.45], // Выветривается в богатые глины
            RockType::ShaleClay | RockType::PelagicSediment => [0.15, 0.30, 0.55], // Тяжелая глина
        }
    }

    /// Податливость породы размыву реками (0.1 — сверхтвердый гранит, 1.0 — мягкие глины)
    #[inline]
    pub fn erodibility(self) -> f32 {
        match self {
            RockType::Granite => 0.15,
            RockType::GneissSchist => 0.25,
            RockType::FloodBasalt | RockType::OceanicBasalt => 0.22,
            RockType::Andesite => 0.35,
            RockType::Limestone => 0.55, // Растворяется водой (карст)
            RockType::Sandstone => 0.65,
            RockType::ShaleClay | RockType::PelagicSediment => 0.90,
        }
    }

    /// Коэффициент карстовых/глубоких потерь стока (актуально для известняков и трещиноватых базальтов)
    #[inline]
    pub fn karst_permeability(self) -> f32 {
        match self {
            RockType::Limestone => 0.35,
            RockType::FloodBasalt => 0.15,
            RockType::Sandstone => 0.12,
            _ => 0.02,
        }
    }
}

/// Слой геологических данных: массивы индексируются по `CellId`
#[derive(Resource, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeologyLayer {
    pub plate_ids: Vec<usize>,
    pub elevations: Vec<f32>,
    pub plates: Vec<Plate>,
    pub volcanoes: Vec<Volcano>,
    pub volcano_mask: Vec<bool>,
    pub lithology: Vec<RockType>,
    pub terrain_ruggedness: Vec<f32>,
}

impl GeologyLayer {
    pub fn resize(&mut self, num_cells: usize) {
        self.plate_ids.resize(num_cells, 0);
        self.elevations.resize(num_cells, 0.0);
        self.volcano_mask.resize(num_cells, false);
        self.lithology.resize(num_cells, RockType::default());
        self.terrain_ruggedness.resize(num_cells, 0.0);
    }
}
