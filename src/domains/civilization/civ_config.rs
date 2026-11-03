// src/domains/civilization/civ_config.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::civ_data::{NUM_SOC_LAYERS, SocLayerKind};

/// Универсальный паспорт слоя института (единый интерфейс для любого слоя)
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct InstitutionSpec {
    pub active: bool,
    pub speed: f32,
    pub spatial_diffusion: f32,
    pub distance_decay: f32,
    pub fracture_threshold: f32,
    pub emergence_threshold: f32,
    /// Веса опоры на 5 факторов среды: [Foraging, Arable, Pastoral, Fishery, Minerals]
    pub env_affinity: [f32; 5],

    // --- Параметры эволюции битового поля (IdeaBits) ---
    pub env_imprint_rate: f32,
    pub cross_layer_learn_rate: f32,
    pub internal_diffusion_rate: f32,
    pub border_osmosis_rate: f32,
    pub conquest_loot_chance: f32,
}

#[derive(Resource, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CivilizationConfig {
    pub is_running: bool,
    pub steps_per_frame: u32,
    pub inspected_layer: SocLayerKind,

    pub specs: [InstitutionSpec; NUM_SOC_LAYERS],
    /// Матрица влияния слоев M[target_layer][source_layer]
    pub cross_matrix: [[f32; NUM_SOC_LAYERS]; NUM_SOC_LAYERS],

    pub infra_growth_rate: f32,
    pub infra_decay_rate: f32,
}

impl Default for CivilizationConfig {
    fn default() -> Self {
        let specs = [
            // 0: Species (Раса / Популяция) — опирается на собирательство, пашню, скот и рыбу
            InstitutionSpec {
                active: true,
                speed: 0.14,
                spatial_diffusion: 0.55,
                distance_decay: 0.04,
                fracture_threshold: 0.92,
                emergence_threshold: 0.58,
                env_affinity: [0.90, 0.85, 0.70, 0.65, 0.10],
                env_imprint_rate: 0.08,
                cross_layer_learn_rate: 0.02,
                internal_diffusion_rate: 0.25,
                border_osmosis_rate: 0.0, // Гены вида не перетекают через границу без смешения
                conquest_loot_chance: 0.0,
            },
            // 1: Kinship (Род / Клан) — быстро рождается, но быстро дробится с расстоянием
            InstitutionSpec {
                active: true,
                speed: 0.18,
                spatial_diffusion: 0.62,
                distance_decay: 0.38,
                fracture_threshold: 0.52,
                emergence_threshold: 0.22,
                env_affinity: [0.80, 0.50, 0.75, 0.60, 0.15],
                env_imprint_rate: 0.12,
                cross_layer_learn_rate: 0.10,
                internal_diffusion_rate: 0.45,
                border_osmosis_rate: 0.08,
                conquest_loot_chance: 0.35,
            },
            // 2: Culture (Культура / Этнос) — медленная, инертная, образует большие семьи и гибриды
            InstitutionSpec {
                active: true,
                speed: 0.06,
                spatial_diffusion: 0.48,
                distance_decay: 0.12,
                fracture_threshold: 0.78,
                emergence_threshold: 0.34,
                env_affinity: [0.40, 0.65, 0.60, 0.50, 0.30],
                env_imprint_rate: 0.10,
                cross_layer_learn_rate: 0.18,
                internal_diffusion_rate: 0.55,
                border_osmosis_rate: 0.14,
                conquest_loot_chance: 0.50,
            },
            // 3: Religion (Миф / Религия) — широкое распространение, схизмы при высоком стрессе
            InstitutionSpec {
                active: true,
                speed: 0.10,
                spatial_diffusion: 0.72,
                distance_decay: 0.14,
                fracture_threshold: 0.74,
                emergence_threshold: 0.42,
                env_affinity: [0.25, 0.45, 0.45, 0.35, 0.25],
                env_imprint_rate: 0.04,
                cross_layer_learn_rate: 0.22,
                internal_diffusion_rate: 0.70,
                border_osmosis_rate: 0.20,
                conquest_loot_chance: 0.40,
            },
            // 4: Economy (Хозяйство / Рынок) — напрямую считывает руды/биоресурсы и разносит их по сети
            InstitutionSpec {
                active: true,
                speed: 0.15,
                spatial_diffusion: 0.68,
                distance_decay: 0.16,
                fracture_threshold: 0.70,
                emergence_threshold: 0.30,
                env_affinity: [0.35, 0.90, 0.75, 0.80, 0.95],
                env_imprint_rate: 0.35, // Быстро подхватывает локальные ресурсы клетки
                cross_layer_learn_rate: 0.25,
                internal_diffusion_rate: 0.85, // Развозит ресурсы по всей торговой плите!
                border_osmosis_rate: 0.30,
                conquest_loot_chance: 0.75,
            },
            // 5: Polity (Власть / Государство) — быстрые завоевания, чувствительность к чужой культуре
            InstitutionSpec {
                active: true,
                speed: 0.20,
                spatial_diffusion: 0.76,
                distance_decay: 0.24,
                fracture_threshold: 0.62,
                emergence_threshold: 0.40,
                env_affinity: [0.15, 0.85, 0.55, 0.50, 0.70],
                env_imprint_rate: 0.05,
                cross_layer_learn_rate: 0.30,
                internal_diffusion_rate: 0.65,
                border_osmosis_rate: 0.12,
                conquest_loot_chance: 0.65,
            },
            // 6: Knowledge (Знание / Наука) — учится у Экономики и Государства, дает технологический рывок
            InstitutionSpec {
                active: true,
                speed: 0.09,
                spatial_diffusion: 0.64,
                distance_decay: 0.10,
                fracture_threshold: 0.85,
                emergence_threshold: 0.46,
                env_affinity: [0.10, 0.40, 0.25, 0.35, 0.65],
                env_imprint_rate: 0.02, // Почти не берет сырье напрямую, требует слой Экономики!
                cross_layer_learn_rate: 0.42,
                internal_diffusion_rate: 0.80,
                border_osmosis_rate: 0.35,
                conquest_loot_chance: 0.80,
            },
        ];

        // Матрица межслойного взаимодействия M[target][source]
        // Слои: 0:Species, 1:Kinship, 2:Culture, 3:Religion, 4:Economy, 5:Polity, 6:Knowledge
        let cross_matrix = [
            // На 0 (Species) положительно влияют Хозяйство (4) и Наука (6) — медицина и агротехника
            [0.20, 0.10, 0.05, 0.05, 0.35, 0.10, 0.25],
            // На 1 (Kinship) влияет Популяция (0), но сильное Государство (5) вытесняет архаичные кланы (-0.30)
            [0.55, 0.25, 0.20, 0.10, 0.10, -0.30, -0.10],
            // На 2 (Culture) влияют Популяция (0), Кланы (1), Религия (3) и Государство (5)
            [0.35, 0.35, 0.30, 0.25, 0.15, 0.25, 0.15],
            // На 3 (Religion) влияют Культура (2), Кланы (1) и Государство (5)
            [0.20, 0.25, 0.40, 0.30, 0.10, 0.25, -0.08],
            // На 4 (Economy) влияют Популяция (0), Государство (5 — порядок) и Наука (6 — технологии)
            [0.40, 0.15, 0.15, 0.05, 0.30, 0.30, 0.40],
            // На 5 (Polity) влияют Культура (2), Религия (3), Хозяйство (4) и Наука (6)
            [0.25, 0.15, 0.30, 0.25, 0.40, 0.25, 0.30],
            // На 6 (Knowledge) сильно влияют Хозяйство (4), Государство (5) и Культура (2)
            [0.15, 0.05, 0.25, 0.10, 0.45, 0.35, 0.35],
        ];

        Self {
            is_running: false,
            steps_per_frame: 1,
            inspected_layer: SocLayerKind::Polity,
            specs,
            cross_matrix,
            infra_growth_rate: 0.015,
            infra_decay_rate: 0.004,
        }
    }
}
