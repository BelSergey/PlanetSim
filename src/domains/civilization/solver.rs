// src/domains/civilization/solver.rs

use rayon::prelude::*;

use super::{
    civ_config::CivilizationConfig,
    civ_data::{
        CellSocState, CivilizationLayer, DNA_BITS_MASK_W0, IdeaBits, NUM_SOC_LAYERS, SocLayerKind,
    },
};
use crate::{
    domains::{
        climate::climate_data::ClimateLayer,
        hydrology::hydrology_data::{BioResource, Biome, HydrologyLayer, MineralResource},
        tectonics::tectonics_data::GeologyLayer,
    },
    world::topology::WorldTopology,
};

#[inline]
fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
fn hash_to_unit_f32(seed: u32) -> f32 {
    (hash_u32(seed) & 0x00FF_FFFF) as f32 / 16_777_215.0
}

/// Генерирует 12-битную сигнатуру ДНК (в битах 52..63 слова 0) для новой сущности
#[inline]
fn generate_entity_dna(entity_id: u32, layer: usize) -> IdeaBits {
    let h1 = hash_u32(entity_id ^ ((layer as u32).wrapping_mul(0x9E37_79B9))) as u64;
    let dna_12bit = ((h1 & 0x0FFF) | 0x0101) << 52;
    let mut bits = IdeaBits::ZERO;
    bits.0[0] = dna_12bit & DNA_BITS_MASK_W0;
    bits
}

/// Подготавливает природные маски, проходимость и стартовые очаги Слоя 0 и Слоя 1
pub fn prepare_civilization_substrate(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &HydrologyLayer,
    civ: &mut CivilizationLayer,
    seed: u32,
) {
    let num_cells = topology.cells.len();
    if geology.elevations.len() != num_cells || hydrology.biomes.len() != num_cells {
        return;
    }

    civ.resize(num_cells);
    civ.clear_simulation();

    let river_thresh = 0.00035_f32;
    let has_climate = climate.spring.temperatures.len() == num_cells
        && climate.summer.temperatures.len() == num_cells
        && climate.winter.temperatures.len() == num_cells;

    let mut candidates: Vec<(usize, f32)> = Vec::new();

    for i in 0..num_cells {
        // 1. Плоский массив 6 соседей для GPU-совместимости
        let mut n_arr = [u32::MAX; 6];
        for (slot, &n_id) in topology.cells[i].neighbors.iter().take(6).enumerate() {
            n_arr[slot] = n_id as u32;
        }
        civ.flat_neighbors[i] = n_arr;

        let elev = geology.elevations[i];
        if elev <= 0.0 {
            civ.env_masks[i] = IdeaBits::ZERO;
            civ.base_habitability[i] = 0.0;
            civ.base_friction[i] = 0.85;
            continue;
        }

        // 2. Собираем природную битовую маску клетки (биты 0..35)
        let mut mask = IdeaBits::ZERO;

        let forage = hydrology.foraging_potential.get(i).copied().unwrap_or(0.0);
        let fish = hydrology.fishery_potential.get(i).copied().unwrap_or(0.0);
        let pastoral = hydrology.pastoral_potential.get(i).copied().unwrap_or(0.0);
        let arable = hydrology.arable_potential.get(i).copied().unwrap_or(0.0);

        if forage > 0.30 {
            mask.set_bit(0);
        }
        if fish > 0.25 {
            mask.set_bit(1);
        }
        if pastoral > 0.30 {
            mask.set_bit(2);
        }
        if arable > 0.30 {
            mask.set_bit(3);
        }

        let mineral = hydrology
            .minerals
            .get(i)
            .copied()
            .unwrap_or(MineralResource::None);
        if mineral != MineralResource::None {
            mask.set_bit(3 + mineral as usize);
        }

        let bio = hydrology
            .bio_resources
            .get(i)
            .copied()
            .unwrap_or(BioResource::None);
        if bio != BioResource::None {
            mask.set_bit(18 + bio as usize);
        }

        let mean_temp = if has_climate {
            (climate.spring.temperatures[i]
                + climate.summer.temperatures[i]
                + climate.winter.temperatures[i])
                / 3.0
        } else {
            15.0
        };
        let sm = hydrology.soil_moisture.get(i).copied().unwrap_or(0.5);
        let rugged = geology.terrain_ruggedness.get(i).copied().unwrap_or(0.0);
        let biome = hydrology.biomes[i];
        let flow = hydrology.flow_accumulation.get(i).copied().unwrap_or(0.0);

        if mean_temp < 0.0 {
            mask.set_bit(28);
        }
        if mean_temp > 24.0 {
            mask.set_bit(29);
        }
        if sm < 0.22 {
            mask.set_bit(30);
        }
        if biome == Biome::Wetland {
            mask.set_bit(31);
        }
        if rugged > 0.42 {
            mask.set_bit(32);
        }
        if matches!(
            biome,
            Biome::BorealTaiga
                | Biome::TemperateDeciduousForest
                | Biome::TemperateRainforest
                | Biome::TropicalSeasonalForest
                | Biome::TropicalRainforest
        ) {
            mask.set_bit(33);
        }
        if flow >= river_thresh {
            mask.set_bit(34);
        }
        if topology.cells[i]
            .neighbors
            .iter()
            .any(|&n| geology.elevations[n] <= 0.0)
        {
            mask.set_bit(35);
        }

        civ.env_masks[i] = mask;

        // 3. Базовое сопротивление движению
        let biome_fric = match biome {
            Biome::IceSheet => 0.90,
            Biome::Wetland => 0.55,
            Biome::TropicalRainforest => 0.50,
            Biome::BorealTaiga | Biome::TemperateRainforest => 0.35,
            Biome::HotDesert | Biome::ColdDesert => 0.40,
            Biome::AlpineBare => 0.65,
            Biome::TemperateDeciduousForest | Biome::TropicalSeasonalForest => 0.22,
            Biome::TemperateGrassland | Biome::Savanna | Biome::MediterraneanShrubland => 0.08,
            Biome::RiparianOasis => 0.05,
            _ => 0.25,
        };
        let river_bonus = if flow >= river_thresh { 0.15 } else { 0.0 };
        civ.base_friction[i] = (rugged * 0.65 + biome_fric * 0.55 - river_bonus).clamp(0.04, 0.96);

        // 4. Пригодность клетки для жизни: берем лучший уклад + бонус от суммы остальных
        let best_food = forage.max(arable).max(pastoral).max(fish);
        let sum_food = forage * 0.35 + arable * 0.40 + pastoral * 0.25 + fish * 0.25;
        let food_base = (best_food * 0.70 + sum_food * 0.45).clamp(0.0, 1.0);
        let hab = (food_base * (1.0 - rugged * 0.40)).clamp(0.0, 1.0);
        civ.base_habitability[i] = hab;

        // Отбираем кандидатов для стартовых колыбелей (теплые богатые долины с собирательством и пашней)
        if hab > 0.45 && forage > 0.35 {
            let jitter = hash_to_unit_f32(seed ^ (i as u32).wrapping_mul(0x45d9_f3b));
            if jitter > 0.96 {
                candidates.push((i, hab + forage * 0.5 + jitter * 0.2));
            }
        }
    }

    // 5. Гарантированно засеваем от 8 до 24 стартовых колыбелей жизни (Слой 0 и первичные кланы Слоя 1)
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let spawn_count = candidates.len().clamp(1, 18);

    // Палитра доступных рас. Если в симуляции только люди — оставьте одну запись.
    // В норме должна приходить из civ_palettes / WorldConfig.
    let species_palette: &[u32] = &[
        (hash_u32(seed ^ 0xA1B2_C3D4) | 1) & 0xFFFF_FFFE, // люди
                                                          // (hash_u32(seed ^ 0x1234_5678) | 1) & 0xFFFF_FFFE, // эльфы
                                                          // (hash_u32(seed ^ 0x8765_4321) | 1) & 0xFFFF_FFFE, // орки
    ];

    for (idx, &(cell_idx, _)) in candidates.iter().take(spawn_count).enumerate() {
        let mask = civ.env_masks[cell_idx];
        let species_id = species_palette[idx % species_palette.len()];
        let clan_id = (hash_u32(seed ^ (cell_idx as u32).wrapping_mul(53)) | 1) & 0xFFFF_FFFE;

        // Слой 0: Раса / Популяция
        civ.cells[cell_idx].entity_id[0] = species_id;
        civ.cells[cell_idx].strength[0] = 0.75;
        civ.cells[cell_idx].prev_strength[0] = 0.75;
        civ.cells[cell_idx].ideas[0] = generate_entity_dna(species_id, 0).bitor(mask);

        // Слой 1: Первичное родовое племя в колыбели
        civ.cells[cell_idx].entity_id[1] = clan_id;
        civ.cells[cell_idx].strength[1] = 0.55;
        civ.cells[cell_idx].prev_strength[1] = 0.55;
        civ.cells[cell_idx].ideas[1] = generate_entity_dna(clan_id, 1).bitor(mask);
    }
}

/// Выполняет 1 шаг многопоточной симуляции всех 7 институциональных слоев общества
pub fn step_civilization_simulation(
    geology: &GeologyLayer,
    hydrology: &HydrologyLayer,
    civ: &mut CivilizationLayer,
    config: &CivilizationConfig,
    world_seed: u32,
) {
    let num_cells = civ.cells.len();
    if num_cells == 0 || geology.elevations.len() != num_cells {
        return;
    }

    civ.tick += 1;
    let tick_u32 = civ.tick as u32;

    let current = &civ.cells;
    let next = &mut civ.scratch_cells;
    let flat_neighbors = &civ.flat_neighbors;
    let env_masks = &civ.env_masks;
    let base_friction = &civ.base_friction;
    let specs = &config.specs;
    let cross_matrix = &config.cross_matrix;

    next.par_iter_mut().enumerate().for_each(|(i, next_cell)| {
        let cur = current[i];

        if geology.elevations[i] <= 0.0 {
            *next_cell = CellSocState::default();
            return;
        }

        let neighbors = &flat_neighbors[i];
        let env_mask = env_masks[i];

        // 1. Обновление инфраструктуры клетки
        let econ_s = cur.strength[SocLayerKind::Economy.index()];
        let polity_s = cur.strength[SocLayerKind::Polity.index()];
        let pop_s = cur.strength[SocLayerKind::Species.index()];
        let target_infra = ((pop_s * 0.30 + econ_s * 0.45 + polity_s * 0.35)
            * (1.0 - cur.stress[SocLayerKind::Polity.index()] * 0.5))
            .clamp(0.0, 1.0);

        let infra_rate = if target_infra > cur.infrastructure {
            config.infra_growth_rate
        } else {
            config.infra_decay_rate
        };
        let new_infra =
            (cur.infrastructure + (target_infra - cur.infrastructure) * infra_rate).clamp(0.0, 1.0);

        let tech_bits = cur.ideas[SocLayerKind::Knowledge.index()]
            .bitor(cur.ideas[SocLayerKind::Economy.index()]);
        let eng_mastery = (if tech_bits.has_bit(32) { 0.25 } else { 0.0 })
            + (if tech_bits.has_bit(33) { 0.20 } else { 0.0 })
            + (if tech_bits.has_bit(34) { 0.20 } else { 0.0 });

        let my_effective_friction =
            (base_friction[i] * (1.0 - new_infra * (0.45 + eng_mastery))).clamp(0.02, 0.95);
        let my_conductivity = 1.0 - my_effective_friction;

        let env_factors = [
            hydrology.foraging_potential[i],
            hydrology.arable_potential[i],
            hydrology.pastoral_potential[i],
            hydrology.fishery_potential[i],
            hydrology.mineral_richness[i],
        ];

        let mut out_cell = cur;
        out_cell.infrastructure = new_infra;

        // 2. Послойный расчет (k = 0..7)
        for k in 0..NUM_SOC_LAYERS {
            let spec = &specs[k];
            if !spec.active {
                continue;
            }

            let my_s = cur.strength[k];
            let my_id = cur.entity_id[k];
            let my_ideas = cur.ideas[k];

            // --- А. Расчет локальной несущей емкости (по доминирующему укладу + синергия) ---
            let mut max_weighted = 0.0_f32;
            let mut sum_weighted = 0.0_f32;
            let mut aff_sum = 0.0_f32;
            for (f_idx, &env_val) in env_factors.iter().enumerate() {
    let w = spec.env_affinity[f_idx];
    let mastery_mult = if my_ideas.has_bit(f_idx) { 1.30 } else { 0.95 };
    let val = env_val * w * mastery_mult;
                if val > max_weighted {
                    max_weighted = val;
                }
                sum_weighted += val;
                aff_sum += w;
            }
            let avg_weighted = if aff_sum > 0.0 {
                sum_weighted / aff_sum
            } else {
                0.0
            };
            // Клетка кормится от своего лучшего ресурса + бонус от комплексности
            let env_cap = (max_weighted * 0.75 + avg_weighted * 0.45).clamp(0.0, 1.0);

            let unlocked_func_bits = my_ideas.count_functional_ones() as f32;
            let tech_cap_bonus = (unlocked_func_bits * 0.022).clamp(0.0, 0.65);

            let mut cross_push = 0.0_f32;
            for (m, &w_km) in cross_matrix[k].iter().enumerate() {
    if m != k {
        cross_push += w_km * cur.strength[m];
    }
}

            let local_capacity = if k == 0 {
                (env_cap * 1.15 + cross_push * 0.35 + tech_cap_bonus).clamp(0.0, 1.0)
            } else {
                let demo_ceiling = (cur.strength[0] * 1.30).clamp(0.0, 1.0);
                ((env_cap * 0.65 + cross_push * 0.65 + tech_cap_bonus) * demo_ceiling)
                    .clamp(0.0, 1.0)
            };

            // --- Б. Сбор влияния от 6 соседей ---
            let mut neighbor_avg_s = 0.0_f32;
            let mut neighbor_max_s = 0.0_f32;
            let mut weight_sum = 0.0_f32;

            let mut same_id_support = 0.0_f32;
            let mut foreign_friction = 0.0_f32;

            let mut best_rival_id = 0u32;
            let mut best_rival_push = 0.0_f32;
            let mut best_rival_ideas = IdeaBits::ZERO;

            for &n_u32 in neighbors {
                if n_u32 == u32::MAX {
                    break;
                }
                let n_id = n_u32 as usize;
                if geology.elevations[n_id] <= 0.0 {
                    continue;
                }

                let n_cell = current[n_id];
                let n_cond = (1.0 - base_friction[n_id] * (1.0 - n_cell.infrastructure * 0.5))
                    .clamp(0.02, 0.98);
                let link_cond = (my_conductivity * n_cond).sqrt();

                let n_s = n_cell.strength[k];
                let n_ent = n_cell.entity_id[k];

                neighbor_avg_s += n_s * link_cond;
                if n_s * link_cond > neighbor_max_s {
                    neighbor_max_s = n_s * link_cond;
                }
                weight_sum += link_cond;

                if n_ent != 0 {
                    if n_ent == my_id {
                        same_id_support += n_s * link_cond;
                    } else {
                        let n_tech_power =
                            1.0 + (n_cell.ideas[k].count_functional_ones() as f32) * 0.025;
                        let mut push = n_s * link_cond * n_tech_power;

                        if k == SocLayerKind::Polity.index() {
                            let attacker_culture_tag = n_ent & 0xFFFF;
                            let local_culture_tag =
                                cur.entity_id[SocLayerKind::Culture.index()] & 0xFFFF;
                            if attacker_culture_tag != 0
                                && attacker_culture_tag == local_culture_tag
                            {
                                push *= 1.55;
                            } else {
                                let cultural_affinity = n_cell.ideas[k]
                                    .jaccard_similarity(&cur.ideas[SocLayerKind::Culture.index()]);
                                push *= 0.65 + 0.35 * cultural_affinity;
                            }
                        }

                        if push > best_rival_push {
                            best_rival_push = push;
                            best_rival_id = n_ent;
                            best_rival_ideas = n_cell.ideas[k];
                        }

                        if my_id != 0 {
                            let affinity = my_ideas.jaccard_similarity(&n_cell.ideas[k]);
                            foreign_friction += my_s * n_s * link_cond * (1.25 - affinity * 0.65);
                        }
                    }
                }
            }

            if weight_sum > 0.0 {
                neighbor_avg_s /= weight_sum;
            }

            // --- В. Обновление силы слоя S_t ---
            let mut new_id = my_id;
            let mut new_ideas = my_ideas;

            // Мирная экспансия соседней плиты в пустую клетку
            if new_id == 0 && best_rival_id != 0 && best_rival_push > 0.08 && local_capacity > 0.06
            {
                new_id = best_rival_id;
                new_ideas = best_rival_ideas;
            }

            // Авторождение новой сущности (для слоев 1..6 на базе популяции или новых очагов Слоя 0)
            let cell_rng = hash_to_unit_f32(
                world_seed
                    ^ (i as u32).wrapping_mul(0x9E37_79B9)
                    ^ tick_u32.wrapping_mul(0x85EB_CA6B)
                    ^ (k as u32 * 97),
            );

            if new_id == 0
                && k != SocLayerKind::Species.index()
                && local_capacity >= spec.emergence_threshold
                && cur.entity_id[0] != 0
                && cell_rng < 0.12
            {
                let raw_uid = (hash_u32(world_seed ^ (i as u32) ^ (tick_u32 << 7) ^ (k as u32))
                    | 1)
                    & 0xFFFF_FFFE;

                new_id = if k == SocLayerKind::Polity.index() {
                    let local_cult_tag = cur.entity_id[SocLayerKind::Culture.index()] & 0xFFFF;
                    (raw_uid & 0xFFFF_0000) | local_cult_tag.max(1)
                } else {
                    raw_uid
                };

                let mut born_ideas = generate_entity_dna(new_id, k);
                if spec.env_imprint_rate > 0.02 {
                    born_ideas = born_ideas.bitor(env_mask);
                }
                if k == SocLayerKind::Polity.index() {
                    born_ideas = born_ideas.bitor(cur.ideas[SocLayerKind::Culture.index()]);
                }
                new_ideas = born_ideas;
            }

            // Сила слоя растет только если в клетке уже присутствует сущность (new_id != 0)
            let mut new_s = if new_id != 0 {
                let inertia = (my_s - cur.prev_strength[k]) * 0.20;
                let spatial_pull =
                    neighbor_max_s * (spec.spatial_diffusion * 0.45) + neighbor_avg_s * 0.25;
                let target_s =
                    (local_capacity.max(spatial_pull) * 0.85 + spatial_pull * 0.25 + inertia)
                        .clamp(0.0, local_capacity.max(0.15));
                (my_s + spec.speed * (target_s - my_s)).clamp(0.05, 1.0)
            } else {
                0.0
            };

            // --- Г. Расчет напряженности (Stress) и Нелинейные события ---
            let mut new_stress = cur.stress[k] * 0.88;

            if new_id != 0 {
                let isolation = (1.0 - same_id_support * 0.55).clamp(0.0, 1.0);
                let decay_stress = spec.distance_decay * isolation * my_effective_friction * 0.28;

                let mut cultural_unrest = 0.0_f32;
                if k == SocLayerKind::Polity.index() {
                    let state_cult_tag = new_id & 0xFFFF;
                    let cell_cult_tag = cur.entity_id[SocLayerKind::Culture.index()] & 0xFFFF;
                    if state_cult_tag != 0 && cell_cult_tag != 0 && state_cult_tag != cell_cult_tag
                    {
                        let syncretism =
                            new_ideas.jaccard_similarity(&cur.ideas[SocLayerKind::Culture.index()]);
                        cultural_unrest = (1.0 - syncretism) * 0.22;
                    }
                }

                let stability_damp =
                    1.0 / (1.0 + (new_ideas.0[0] >> 36 & 0xFFFF).count_ones() as f32 * 0.08);

                new_stress = (new_stress
                    + (foreign_friction * 0.35 + decay_stress + cultural_unrest) * stability_damp)
                    .clamp(0.0, 1.5);

                if new_stress >= spec.fracture_threshold {
                    let my_defense = new_s
                        * (0.75 + same_id_support * 0.35 + geology.terrain_ruggedness[i] * 0.40)
                        * (1.0 + (new_ideas.count_functional_ones() as f32) * 0.025);

                    if best_rival_id != 0 && best_rival_push > my_defense * 0.82 {
                        if k == SocLayerKind::Culture.index()
                            && best_rival_push < my_defense * 1.35
                            && cell_rng < 0.35
                        {
                            let hybrid_id =
                                (hash_u32(new_id ^ best_rival_id ^ tick_u32) | 1) & 0xFFFF_FFFE;
                            new_id = hybrid_id;
                            new_ideas = new_ideas
                                .hybrid_crossover(&best_rival_ideas, tick_u32 as u64)
                                .bitor(env_mask);
                            new_stress = 0.15;
                        } else {
                            let looted_bits = if cell_rng < spec.conquest_loot_chance {
                                new_ideas
                            } else {
                                IdeaBits::ZERO
                            };
                            new_id = best_rival_id;
                            new_ideas = best_rival_ideas.bitor(looted_bits);
                            new_s *= 0.72;
                            new_stress = 0.20;
                        }
                    // отключим для рас но вообще надо бы прокинуть через конфиг
                    } else if isolation > 0.55
                        && cell_rng < 0.25
                        && k != SocLayerKind::Species.index()
                    {
                        let rebel_uid =
                            (hash_u32(new_id ^ (i as u32) ^ tick_u32) | 1) & 0xFFFF_FFFE;
                        new_id = if k == SocLayerKind::Polity.index() {
                            let local_cult = cur.entity_id[SocLayerKind::Culture.index()] & 0xFFFF;
                            (rebel_uid & 0xFFFF_0000) | local_cult.max(1)
                        } else {
                            rebel_uid
                        };
                        let mutated_dna = generate_entity_dna(new_id, k);
                        new_ideas.0[0] = (new_ideas.0[0] & !DNA_BITS_MASK_W0)
                            | ((new_ideas.0[0] ^ mutated_dna.0[0]) & DNA_BITS_MASK_W0);
                        new_stress = 0.18;
                    }
                }
            }

            // --- Д. Эволюция и диффузия битового поля прогресса (IdeaBits) ---
            if new_id != 0 && new_s > 0.10 {
                if spec.env_imprint_rate > 0.0 && cell_rng < spec.env_imprint_rate * new_s {
                    let missing_env = env_mask.difference(new_ideas);
                    if !missing_env.is_empty() {
                        let bit = missing_env.0[0].trailing_zeros() as usize;
                        if bit < 36 {
                            new_ideas.set_bit(bit);
                        }
                    }
                }

                if spec.cross_layer_learn_rate > 0.0 {
                    for m in 0..NUM_SOC_LAYERS {
                        if cross_matrix[k][m] > 0.15
                            && cur.strength[m] > 0.20
                            && hash_to_unit_f32(tick_u32 ^ (i as u32) ^ (m as u32 * 31))
                                < spec.cross_layer_learn_rate * cross_matrix[k][m]
                        {
                            let learnable = cur.ideas[m].0[0] & ((1u64 << 52) - 1);
                            new_ideas.0[0] |= learnable;
                        }
                    }
                }

                for &n_u32 in neighbors {
                    if n_u32 == u32::MAX {
                        break;
                    }
                    let n_cell = current[n_u32 as usize];
                    let n_ent = n_cell.entity_id[k];
                    if n_ent == 0 {
                        continue;
                    }

                    let rate = if n_ent == new_id {
                        spec.internal_diffusion_rate
                    } else {
                        spec.border_osmosis_rate
                    };

                    if rate > 0.0
                        && hash_to_unit_f32(tick_u32.wrapping_add(n_u32) ^ (k as u32 * 13))
                            < rate * my_conductivity
                    {
                        let n_func = n_cell.ideas[k].0[0] & ((1u64 << 52) - 1);
                        new_ideas.0[0] |= n_func;
                        for w_idx in 1..super::civ_data::IDEA_WORDS {
                            new_ideas.0[w_idx] |= n_cell.ideas[k].0[w_idx];
                        }
                    }
                }

                let func_count = (new_ideas.0[0] & ((1u64 << 36) - 1)).count_ones();
                for tier in 0..4usize {
                    let req_func = ((tier + 1) * 4) as u32;
                    let req_strength = 0.30 + tier as f32 * 0.14;
                    if func_count >= req_func && new_s >= req_strength && cell_rng < 0.05 {
                        let bit_slot = 36 + tier * 4 + ((tick_u32 as usize + i) & 3);
                        new_ideas.set_bit(bit_slot);
                    }
                }
            } else if new_s < 0.03 {
                new_id = 0;
                new_s = 0.0;
                new_stress = 0.0;
                new_ideas = IdeaBits::ZERO;
            }

            out_cell.prev_strength[k] = my_s;
            out_cell.strength[k] = new_s;
            out_cell.stress[k] = new_stress;
            out_cell.entity_id[k] = new_id;
            out_cell.ideas[k] = new_ideas;
        }

        *next_cell = out_cell;
    });

    std::mem::swap(&mut civ.cells, &mut civ.scratch_cells);
}
