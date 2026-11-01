// src/domains/tectonics/tectonics_data.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Тектоническая плита (смешанная или чисто океаническая в зависимости от `craton_radius`)
#[derive(PartialEq, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Plate {
    /// Текущий геометрический центр плиты на единичной сфере
    pub center_dir: Vec3,
    /// Текущая ось вращения (полюс Эйлера)
    pub euler_pole: Vec3,
    /// Угловая скорость вращения (рад / ед. времени)
    pub angular_vel: f32,
    /// Полный вектор угловой скорости `omega = euler_pole * angular_vel`
    pub omega: Vec3,
    /// Индивидуальная базовая высота (изостатический/мантийный подпор плиты)
    pub base_elevation_offset: f32,
    /// Центр континентального кратона (смещен относительно `center_dir`)
    pub craton_center: Vec3,
    /// Угловой радиус кратона в радианах (`0.0` для чисто океанической плиты вроде Тихоокеанской)
    pub craton_radius: f32,
    /// Стабильный оттенок цвета плиты `[0.0 .. 360.0]` для карты плит
    pub color_hue: f32,
}

impl Default for Plate {
    fn default() -> Self {
        Self {
            center_dir: Vec3::Y,
            euler_pole: Vec3::Y,
            angular_vel: 0.0,
            omega: Vec3::ZERO,
            base_elevation_offset: 0.0,
            craton_center: Vec3::Y,
            craton_radius: 0.0,
            color_hue: 0.0,
        }
    }
}

/// Стационарный мантийный плюм (горячая точка в системе отсчета мантии)
#[derive(PartialEq, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MantlePlume {
    pub position: Vec3,
    pub strength: f32,
    pub radius: f32,
    pub pulse_timer: f32,
    pub pulse_period: f32,
}

/// Вулкан, привязанный к конкретной плите и дрейфующий вместе с ней
#[derive(PartialEq, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Volcano {
    pub center: Vec3,
    pub height: f32,
    pub radius: f32,
    pub plate_id: usize,
    pub age: f32,
}

/// Литологические классы материнских пород
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum RockType {
    #[default]
    OceanicBasalt,   // Океанический базальт (MORB, молодая океаническая кора)
    PelagicSediment, // Глубоководные осадки (кремнистые/глинистые илы абиссали и желобов)
    Granite,         // Кислые интрузии (ядра коллизии и батолиты континентальных дуг)
    GneissSchist,    // Метаморфиты (гнейсы, кристалланские сланцы складчатых поясов и древних щитов)
    Andesite,        // Средние/кислые вулканиты (островные и континентальные дуги, стратовулканы)
    FloodBasalt,     // Континентальные и островные базальты (рифты, траппы, щитовые вулканы плюмов)
    Sandstone,       // Обломочные осадочные (песчаники предгорных прогибов, пассивных окраин и платформ)
    ShaleClay,       // Глинистые осадочные (аргиллиты, глинистые сланцы низменностей и древних морей)
    Limestone,       // Карбонатные породы (известняки и доломиты теплых шельфов и платформ)
}

impl RockType {
    /// Базовая текстура выветривания породы: `[sand, silt, clay]` (в сумме = 1.0) для модели почв CLORPT
    #[inline]
    pub fn base_texture(self) -> [f32; 3] {
        match self {
            RockType::Granite => [0.65, 0.20, 0.15],
            RockType::Sandstone => [0.75, 0.15, 0.10],
            RockType::GneissSchist => [0.45, 0.35, 0.20],
            RockType::Andesite => [0.35, 0.45, 0.20],
            RockType::Limestone => [0.25, 0.45, 0.30],
            RockType::FloodBasalt | RockType::OceanicBasalt => [0.20, 0.35, 0.45],
            RockType::ShaleClay | RockType::PelagicSediment => [0.15, 0.30, 0.55],
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
            RockType::Limestone => 0.55,
            RockType::Sandstone => 0.65,
            RockType::ShaleClay | RockType::PelagicSediment => 0.90,
        }
    }

    /// Коэффициент карстовых/глубоких потерь стока
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

/// Слой геологических данных (SoA-разметка, готовая к переносу в GPU буферы)
#[derive(Resource, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeologyLayer {
    pub plate_ids: Vec<usize>,
    pub elevations: Vec<f32>,

    // Физические параметры клетки коры
    /// Толщина коры в км (~7 км молодая океаническая, ~35 км континентальный кратон, до ~70 км в коллизии)
    pub crust_thickness: Vec<f32>,
    /// Эффективная плотность коры и литосферы в кг/м³ (~2700 континент, ~2900..3140 океаническая)
    pub crust_density: Vec<f32>,
    /// Возраст коры в условных млн лет (0 на оси спрединга, растет на каждом шаге)
    pub crust_age: Vec<f32>,
    /// Толщина осадочного чехла в км
    pub sediment_thickness: Vec<f32>,
    /// Тектоническое напряжение границы: `< 0` рифт/спрединг, `> 0` субдукция/коллизия
    pub tectonic_stress: Vec<f32>,
    /// Накопленный корень складчатости / орогенеза `[0.0 .. 1.0]`
    pub orogeny_root: Vec<f32>,
    /// Субклеточное накопленное смещение на сфере (для защиты континентов от численного размытия)
    pub subcell_shift: Vec<[f32; 3]>,

    // Океаны и континентальность (вычисляются сразу после рельефа)
    /// Истина только для настоящих океанических бассейнов (прошедших порог площади и абиссальной глубины)
    pub is_ocean: Vec<bool>,
    /// Знаковая континентальность `[-1.0 .. +1.0]` (-1 открытый океан, 0 берег, +1 глубь материка)
    pub continentality: Vec<f32>,

    pub plates: Vec<Plate>,
    pub plumes: Vec<MantlePlume>,
    pub volcanoes: Vec<Volcano>,
    pub volcano_mask: Vec<bool>,
    pub lithology: Vec<RockType>,
    pub terrain_ruggedness: Vec<f32>,

    /// Счетчик выполненных шагов симуляции движения плит
    pub sim_step: u32,
}

impl GeologyLayer {
    pub fn resize(&mut self, num_cells: usize) {
        self.plate_ids.resize(num_cells, 0);
        self.elevations.resize(num_cells, 0.0);
        self.crust_thickness.resize(num_cells, 7.5);
        self.crust_density.resize(num_cells, 2950.0);
        self.crust_age.resize(num_cells, 20.0);
        self.sediment_thickness.resize(num_cells, 0.1);
        self.tectonic_stress.resize(num_cells, 0.0);
        self.orogeny_root.resize(num_cells, 0.0);
        self.subcell_shift.resize(num_cells, [0.0; 3]);
        self.is_ocean.resize(num_cells, false);
        self.continentality.resize(num_cells, 0.0);
        self.volcano_mask.resize(num_cells, false);
        self.lithology.resize(num_cells, RockType::default());
        self.terrain_ruggedness.resize(num_cells, 0.0);
    }

    #[inline]
    pub fn is_continental_crust(&self, cell_id: usize) -> bool {
        self.crust_density[cell_id] < 2825.0 || self.crust_thickness[cell_id] >= 19.0
    }

    /// Истина, если клетка принадлежит настоящему океану (а не сухой впадине на суше)
    #[inline]
    pub fn is_ocean_cell(&self, cell_id: usize) -> bool {
        self.is_ocean
            .get(cell_id)
            .copied()
            .unwrap_or_else(|| self.elevations.get(cell_id).copied().unwrap_or(0.0) <= 0.0)
    }
}