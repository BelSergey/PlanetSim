// src/domains/civilization/civ_data.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Количество институциональных слоев общества
pub const NUM_SOC_LAYERS: usize = 7;

/// Количество 64-битных слов в битовом поле прогресса/идей.
/// 2 слова = 128 бит (чтобы расширить до 256 бит, достаточно поменять 2 на 4).
pub const IDEA_WORDS: usize = 2;
pub const TOTAL_IDEA_BITS: usize = IDEA_WORDS * 64;

/// Смысловая разметка битов в первом слове (биты 0..63):
/// 0..3   (4 бита)  — 4 столпа продовольствия (Foraging, Fishery, Pastoral, Arable)
/// 4..18  (15 бит)  — 15 видов ископаемых (Clay .. RareEarths)
/// 19..27 (9 бит)   — 9 видов биоресурсов (NavalTimber .. FurIvory)
/// 28..35 (8 бит)   — Адаптации к экстремумам среды (Мороз, Жара, Засуха, Болото, Горы, Лес, Река, Море)
/// 36..51 (16 бит)  — 4 вертикальных тира организации (по 4 бита на тир)
/// 52..63 (12 бит)  — Уникальный генетический/культурный код (ДНК сущности при рождении)
/// 64..127 (64 бита)— Резерв расширения (сложные идеологии, особые технологии, магия и т.д.)
pub const FUNCTIONAL_BITS_MASK_W0: u64 = (1u64 << 52) - 1;
pub const DNA_BITS_MASK_W0: u64 = !FUNCTIONAL_BITS_MASK_W0;

/// Унифицированное расширяемое битовое поле (100% совместимо с GPU StorageBuffer)
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct IdeaBits(pub [u64; IDEA_WORDS]);

impl IdeaBits {
    pub const ZERO: Self = Self([0; IDEA_WORDS]);

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    #[inline]
    pub fn has_bit(&self, bit: usize) -> bool {
        let word = bit >> 6;
        let offset = bit & 63;
        word < IDEA_WORDS && ((self.0[word] >> offset) & 1) != 0
    }

    #[inline]
    pub fn set_bit(&mut self, bit: usize) {
        let word = bit >> 6;
        let offset = bit & 63;
        if word < IDEA_WORDS {
            self.0[word] |= 1u64 << offset;
        }
    }

    #[inline]
    pub fn clear_bit(&mut self, bit: usize) {
        let word = bit >> 6;
        let offset = bit & 63;
        if word < IDEA_WORDS {
            self.0[word] &= !(1u64 << offset);
        }
    }

    #[inline]
    pub fn bitor(mut self, other: Self) -> Self {
        for i in 0..IDEA_WORDS {
            self.0[i] |= other.0[i];
        }
        self
    }

    #[inline]
    pub fn bitand(mut self, other: Self) -> Self {
        for i in 0..IDEA_WORDS {
            self.0[i] &= other.0[i];
        }
        self
    }

    #[inline]
    pub fn difference(mut self, other: Self) -> Self {
        for i in 0..IDEA_WORDS {
            self.0[i] &= !other.0[i];
        }
        self
    }

    #[inline]
    pub fn count_ones(&self) -> u32 {
        let mut total = 0;
        for i in 0..IDEA_WORDS {
            total += self.0[i].count_ones();
        }
        total
    }

    /// Количество открытых функциональных битов (без учета случайных битов ДНК 52..63)
    #[inline]
    pub fn count_functional_ones(&self) -> u32 {
        let mut total = (self.0[0] & FUNCTIONAL_BITS_MASK_W0).count_ones();
        for i in 1..IDEA_WORDS {
            total += self.0[i].count_ones();
        }
        total
    }

    /// Коэффициент родства Жаккара [0.0 .. 1.0] между двумя сущностями
    #[inline]
    pub fn jaccard_similarity(&self, other: &Self) -> f32 {
        let mut inter = 0u32;
        let mut union = 0u32;
        for i in 0..IDEA_WORDS {
            inter += (self.0[i] & other.0[i]).count_ones();
            union += (self.0[i] | other.0[i]).count_ones();
        }
        if union == 0 {
            0.0
        } else {
            inter as f32 / union as f32
        }
    }

    /// Генетический/культурный кроссовер двух родителей (гибридизация в духе CK3)
    #[inline]
    pub fn hybrid_crossover(&self, other: &Self, salt: u64) -> Self {
        let mut out = Self::ZERO;
        // В слове 0 объединяем освоенные функциональные навыки и смешиваем ДНК родителей
        let func = (self.0[0] | other.0[0]) & FUNCTIONAL_BITS_MASK_W0;
        let mix_mask = 0xAAAA_AAAA_AAAA_AAAAu64 ^ salt;
        let dna = ((self.0[0] & mix_mask) | (other.0[0] & !mix_mask)) & DNA_BITS_MASK_W0;
        out.0[0] = func | dna;

        for i in 1..IDEA_WORDS {
            out.0[i] = self.0[i] | other.0[i];
        }
        out
    }
}

/// 7 слоев (институтов) социальной симуляции
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum SocLayerKind {
    #[default]
    Species = 0, // 0: Раса / Биологический вид и плотность заселения
    Kinship = 1,   // 1: Род / Клан / Племенной строй
    Culture = 2,   // 2: Культура / Этноязыковая общность
    Religion = 3,  // 3: Миф / Религия / Идеология
    Economy = 4,   // 4: Хозяйство / Торговая сеть (распределяет ресурсы)
    Polity = 5,    // 5: Власть / Государство (хранит тег родной культуры в младших 16 битах ID)
    Knowledge = 6, // 6: Знание / Наука / Технологическая традиция
}

impl SocLayerKind {
    pub const ALL: [SocLayerKind; NUM_SOC_LAYERS] = [
        SocLayerKind::Species,
        SocLayerKind::Kinship,
        SocLayerKind::Culture,
        SocLayerKind::Religion,
        SocLayerKind::Economy,
        SocLayerKind::Polity,
        SocLayerKind::Knowledge,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            SocLayerKind::Species => "0. Раса и Популяция",
            SocLayerKind::Kinship => "1. Род и Кланы",
            SocLayerKind::Culture => "2. Культура и Этнос",
            SocLayerKind::Religion => "3. Миф и Религия",
            SocLayerKind::Economy => "4. Хозяйство и Рынок",
            SocLayerKind::Polity => "5. Власть и Государство",
            SocLayerKind::Knowledge => "6. Знание и Наука",
        }
    }
}

/// Состояние всех институтов в одной клетке гекса (фиксированный размер для GPU)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize)]
pub struct CellSocState {
    pub strength: [f32; NUM_SOC_LAYERS],
    pub prev_strength: [f32; NUM_SOC_LAYERS],
    pub stress: [f32; NUM_SOC_LAYERS],
    pub entity_id: [u32; NUM_SOC_LAYERS],
    pub ideas: [IdeaBits; NUM_SOC_LAYERS],
    pub infrastructure: f32,
}

#[derive(Resource, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CivilizationLayer {
    pub tick: u64,
    pub cells: Vec<CellSocState>,
    #[serde(skip)]
    pub scratch_cells: Vec<CellSocState>,

    /// Предрассчитанная природная битовая маска клетки (ресурсы, типы еды, климат, рельеф)
    pub env_masks: Vec<IdeaBits>,
    /// Базовая демографическая емкость среды для Слоя 0 [0.0 .. 1.0]
    pub base_habitability: Vec<f32>,
    /// Базовое сопротивление движению (из terrain_ruggedness и биомов) [0.0 .. 1.0]
    pub base_friction: Vec<f32>,
    /// Плоский массив соседей [u32; 6] для каждой клетки (u32::MAX = пустой 6-й слот у пентагона)
    #[serde(skip)]
    pub flat_neighbors: Vec<[u32; 6]>,
}

impl CivilizationLayer {
    pub fn resize(&mut self, num_cells: usize) {
        self.cells.resize(num_cells, CellSocState::default());
        self.scratch_cells
            .resize(num_cells, CellSocState::default());
        self.env_masks.resize(num_cells, IdeaBits::ZERO);
        self.base_habitability.resize(num_cells, 0.0);
        self.base_friction.resize(num_cells, 0.0);
        self.flat_neighbors.resize(num_cells, [u32::MAX; 6]);
    }

    pub fn clear_simulation(&mut self) {
        self.tick = 0;
        for c in &mut self.cells {
            *c = CellSocState::default();
        }
        for c in &mut self.scratch_cells {
            *c = CellSocState::default();
        }
    }
}
