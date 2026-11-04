// src/domains/tectonics/tectonics_data.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Максимальное число сросшихся кратонных ядер (террейнов) внутри одной плиты.
/// Хранение фиксированным массивом позволяет оставить `Plate` легковесным `Copy`-типом.
pub const MAX_PLATE_CRATON_LOBES: usize = 4;

/// Отдельное ядро древнего кратона (террейна), входящее в состав континентального щита плиты
#[derive(PartialEq, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CratonLobe {
    /// Центр ядра кратона на единичной сфере (вращается вместе с плитой)
    pub center: Vec3,
    /// Угловой радиус ядра в радианах
    pub radius: f32,
    /// Индивидуальная фаза 3D-шума для некруглой формы террейна
    pub seed_offset: f32,
}

impl Default for CratonLobe {
    fn default() -> Self {
        Self {
            center: Vec3::Y,
            radius: 0.0,
            seed_offset: 0.0,
        }
    }
}

/// Тектоническая плита (составная континентально-океаническая или чисто океаническая)
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
    /// Накопленный поворот плиты от начала симуляции (переводит координаты в локальную систему плиты)
    pub accum_rot: Quat,
    /// Индивидуальная базовая высота (изостатический/мантийный подпор плиты)
    pub base_elevation_offset: f32,
    /// Главный центр континентального щита (центроид всех сросшихся кратонов плиты)
    pub craton_center: Vec3,
    /// Эффективный угловой радиус континентального щита в радианах (`0.0` для чисто океанической плиты)
    pub craton_radius: f32,
    /// Сросшиеся кратонные ядра (террейны), формирующие сложный мультитеррейновый континент
    pub lobes: [CratonLobe; MAX_PLATE_CRATON_LOBES],
    /// Количество активных ядер в массиве `lobes` (`0` для чисто океанической плиты)
    pub lobe_count: u8,
    /// Индивидуальное смещение фазы шума плиты
    pub seed_offset: f32,
    /// Стабильный оттенок цвета плиты `[0.0 .. 360.0]` для карты плит
    pub color_hue: f32,
    /// Главная диаметральная ось внутреннего растяжения плиты (единичный вектор в касательной плоскости)
    pub tension_axis: Vec3,
    /// Накопленное внутреннее растягивающее напряжение плиты от разности граничных сил
    pub internal_tension: f32,
    /// Накопленный перегрев мантии под крупным континентальным щитом (теплоизоляция суперконтинента)
    pub mantle_heat: f32,
    /// Флаг живой плиты (`false`, если плита полностью субдуцировала и поглощена соседями)
    pub is_alive: bool,
}

#[warn(deprecated)]
impl Plate {
    /// Пересчитывает `craton_center` и `craton_radius` как центроид и
    /// эквивалентный радиус `√Σ(rᵢ²)` по активным лопастям.
    pub fn recompute_craton_from_lobes(&mut self) {
        if self.lobe_count == 0 {
            self.craton_center = self.center_dir;
            self.craton_radius = 0.0;
            return;
        }
        let mut sum = Vec3::ZERO;
        let mut sum_r2 = 0.0_f32;
        for k in 0..self.lobe_count as usize {
            let lobe = &self.lobes[k];
            let w = lobe.radius * lobe.radius;
            sum += lobe.center * w;
            sum_r2 += w;
        }
        self.craton_center = if sum.length_squared() > 1e-8 {
            sum.normalize_or_zero()
        } else {
            self.center_dir
        };
        // √Σr² совместим с area-preserving комбинацией √(a²+b²) в аккреции
        self.craton_radius = sum_r2.sqrt();
    }
}

impl Default for Plate {
    fn default() -> Self {
        Self {
            center_dir: Vec3::Y,
            euler_pole: Vec3::Y,
            angular_vel: 0.0,
            omega: Vec3::ZERO,
            accum_rot: Quat::IDENTITY,
            base_elevation_offset: 0.0,
            craton_center: Vec3::Y,
            craton_radius: 0.0,
            lobes: [CratonLobe::default(); MAX_PLATE_CRATON_LOBES],
            lobe_count: 0,
            seed_offset: 0.0,
            color_hue: 0.0,
            tension_axis: Vec3::X,
            internal_tension: 0.0,
            mantle_heat: 0.0,
            is_alive: true,
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
    /// Накопленный угловой дрейф коры над плюмом с момента последнего извержения (рад)
    pub accum_drift_rad: f32,
    /// Дистанция дрейфа плиты до следующего извержения (меняется стохастически после каждого выброса)
    pub next_eruption_dist: f32,
    /// Счётчик извержений плюма (`0` означает, что голова плюма ещё не прожгла трапповую провинцию LIP)
    pub eruption_counter: u32,
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
    OceanicBasalt, // Океанический базальт (MORB, молодая океаническая кора)
    PelagicSediment, // Глубоководные осадки (кремнистые/глинистые илы абиссали и желобов)
    Granite,         // Кислые интрузии (ядра коллизии и батолиты континентальных дуг)
    GneissSchist, // Метаморфиты (гнейсы, кристаллические сланцы складчатых поясов и древних щитов)
    Andesite,     // Средние/кислые вулканиты (островные и континентальные дуги, стратовулканы)
    FloodBasalt,  // Континентальные и островные базальты (рифты, траппы, щитовые вулканы плюмов)
    Sandstone, // Обломочные осадочные (песчаники предгорных прогибов, пассивных окраин и платформ)
    ShaleClay, // Глинистые осадочные (аргиллиты, глинистые сланцы низменностей и древних морей)
    Limestone, // Карбонатные породы (известняки и доломиты теплых шельфов и платформ)
}

impl RockType {
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

    pub crust_thickness: Vec<f32>,
    pub crust_density: Vec<f32>,
    pub crust_age: Vec<f32>,
    pub sediment_thickness: Vec<f32>,
    pub tectonic_stress: Vec<f32>,
    pub orogeny_root: Vec<f32>,
    pub subcell_shift: Vec<[f32; 3]>,

    /// Клеточная степень континентальности коры `[0.0 .. 1.0]`, рождаемая из кратонов и переносимая адвекцией
    pub continental_factor: Vec<f32>,
    /// Ослабленные зоны коры и древние сутуры (швы слияния кратонов и закрывшихся океанов) `[0.0 .. 1.0]`
    pub suture_weakness: Vec<f32>,
    /// Тектонический возраст орогена в клетке (позволяет отличать молодые острые хребты от древних пенепленов)
    pub orogen_age: Vec<f32>,
    /// Доступный обломочный материал от разрушения гор и вулканизма (источник осадков для гидрологии)
    pub sediment_supply: Vec<f32>,

    pub is_ocean: Vec<bool>,
    pub continentality: Vec<f32>,

    pub plates: Vec<Plate>,
    /// Матрица `plates.len() * plates.len()` устойчивой полярности субдукции:
    /// `+1` — плита `row` ныряет под плиту `col`; `-1` — `col` ныряет под `row`; `0` — не задано.
    pub subduction_polarity: Vec<i8>,
    pub plumes: Vec<MantlePlume>,
    pub volcanoes: Vec<Volcano>,
    pub volcano_mask: Vec<bool>,
    pub lithology: Vec<RockType>,
    pub terrain_ruggedness: Vec<f32>,

    pub sim_step: u32,
}

impl GeologyLayer {
    pub fn resize(&mut self, num_cells: usize) {
        self.plate_ids.resize(num_cells, 0);
        self.elevations.resize(num_cells, 0.0);
        self.crust_thickness.resize(num_cells, 7.0);
        self.crust_density.resize(num_cells, 2950.0);
        self.crust_age.resize(num_cells, 0.5);
        self.sediment_thickness.resize(num_cells, 0.1);
        self.tectonic_stress.resize(num_cells, 0.0);
        self.orogeny_root.resize(num_cells, 0.0);
        self.subcell_shift.resize(num_cells, [0.0; 3]);

        self.continental_factor.resize(num_cells, 0.0);
        self.suture_weakness.resize(num_cells, 0.0);
        self.orogen_age.resize(num_cells, 0.0);
        self.sediment_supply.resize(num_cells, 0.0);

        self.is_ocean.resize(num_cells, false);
        self.continentality.resize(num_cells, 0.0);
        self.volcano_mask.resize(num_cells, false);
        self.lithology.resize(num_cells, RockType::default());
        self.terrain_ruggedness.resize(num_cells, 0.0);
    }

    /// Проверяет и при необходимости доинициализирует новые вспомогательные векторы после загрузки старых сохранений
    pub fn ensure_aux_buffers(&mut self, num_cells: usize) {
        if self.continental_factor.len() != num_cells {
            self.continental_factor.resize(num_cells, 0.0);
        }
        if self.suture_weakness.len() != num_cells {
            self.suture_weakness.resize(num_cells, 0.0);
        }
        if self.orogen_age.len() != num_cells {
            self.orogen_age.resize(num_cells, 0.0);
        }
        if self.sediment_supply.len() != num_cells {
            self.sediment_supply.resize(num_cells, 0.0);
        }
        let n_plates = self.plates.len();
        if self.subduction_polarity.len() != n_plates * n_plates {
            self.subduction_polarity.resize(n_plates * n_plates, 0);
        }
    }

    /// Возвращает полярность субдукции для пары плит `(p1, p2)`:
    /// `true`, если `p1` является нижней (погружающейся в жёлоб) плитой относительно `p2`.
    #[inline]
    pub fn is_plate_subducting_under(&self, p1: usize, p2: usize) -> Option<bool> {
        let n = self.plates.len();
        if p1 >= n || p2 >= n || p1 == p2 || self.subduction_polarity.len() != n * n {
            return None;
        }
        match self.subduction_polarity[p1 * n + p2] {
            1 => Some(true),
            -1 => Some(false),
            _ => None,
        }
    }

    /// Фиксирует полярность субдукции между плитами `subducting_plate` (нижняя) и `overriding_plate` (верхняя)
    #[inline]
    pub fn set_subduction_polarity(&mut self, subducting_plate: usize, overriding_plate: usize) {
        let n = self.plates.len();
        if subducting_plate >= n || overriding_plate >= n || subducting_plate == overriding_plate {
            return;
        }
        if self.subduction_polarity.len() != n * n {
            self.subduction_polarity.resize(n * n, 0);
        }
        self.subduction_polarity[subducting_plate * n + overriding_plate] = 1;
        self.subduction_polarity[overriding_plate * n + subducting_plate] = -1;
    }

    /// Проверяет, является ли кора континентальной (по низкой плотности фельзического кратона).
    #[inline]
    pub fn is_continental_crust(&self, cell_id: usize) -> bool {
        self.crust_density[cell_id] < 2835.0
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