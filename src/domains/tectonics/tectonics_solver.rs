// src/domains/tectonics/tectonics_solver.rs

use super::{
    tectonics_config::TectonicsConfig,
    tectonics_data::{GeologyLayer, Plate, PlateType, RockType, Volcano},
};
use crate::world::topology::WorldTopology;
use bevy::math::{Quat, Vec3};
use noise::NoiseFn;
use rand::RngExt;
use rayon::prelude::*;
use std::collections::VecDeque;
use std::f32::consts::PI;

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn profile_factor(t: f32, smoothness: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let sm = smoothstep(t);
    t * (1.0 - smoothness) + sm * smoothness
}

/// Генерирует центры тектонических плит и определяет их физические параметры (ось и скорость вращения)
pub fn generate_plates(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<Plate> {
    let mut plates = Vec::with_capacity(config.num_plates);

    for _ in 0..config.num_plates {
        // 1. Случайным образом определяем тип плиты с учетом заданного в конфиге шанса материка
        let is_cont = rng.random_bool(config.continental_ratio);
        let plate_type = if is_cont {
            PlateType::Continental
        } else {
            PlateType::Oceanic
        };

        let mut best_dir = Vec3::Y;
        // Для выбора наиболее удаленного кандидата достаточно минимизировать
        // максимальный dot product с уже размещенными плитами: acos здесь не нужен.
        let mut best_max_dot = f32::INFINITY;

        // 2. Равномерно распределяем центры плит по сфере
        for _ in 0..config.plate_dir_iterations {
            let z: f32 = rng.random_range(-1.0..1.0);
            let lon: f32 = rng.random_range(-PI..PI);
            let r = (1.0 - z * z).sqrt();
            let candidate = Vec3::new(r * lon.cos(), z, r * lon.sin());

            // Максимальная угловая дистанция получается при минимальном
            // максимальном dot product с уже размещенными плитами.
            let max_dot = plates
                .iter()
                .map(|plate: &Plate| candidate.dot(plate.center_dir))
                .fold(-1.0_f32, f32::max);

            if max_dot < best_max_dot {
                best_max_dot = max_dot;
                best_dir = candidate;
            }
        }

        // 3. Генерируем полюс Эйлера (ось, вокруг которой будет вращаться плита).
        let rand_vec = Vec3::new(
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
        )
        .normalize_or_zero();

        let euler_pole = if rand_vec == Vec3::ZERO {
            Vec3::Y
        } else {
            rand_vec
        };

        // 4. Задаем случайную скалярную скорость вращения плиты
        let angular_vel = rng.random_range(config.angular_vel_min..config.angular_vel_max);

        plates.push(Plate {
            center_dir: best_dir,
            plate_type,
            euler_pole,
            angular_vel,
        });
    }
    plates
}

/// Главная функция симуляции: искажает границы плит, находит зоны контакта, рассчитывает рельеф и породы
pub fn simulate_tectonics(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    if geology.lithology.len() != num_cells {
        geology.lithology.resize(num_cells, RockType::default());
    }

    let plates = &geology.plates;
    let volcanoes = &geology.volcanoes;
    let elevations = &mut geology.elevations;
    let plate_ids = &mut geology.plate_ids;
    let volcano_mask = &mut geology.volcano_mask;
    let lithology = &mut geology.lithology;
    // Радиус горячей точки не меняется во время прохода по клеткам.
    // Кэшируем cos(radius), чтобы не вычислять cos для каждой пары «клетка-вулкан».
    let volcano_cos_radii: Vec<f32> = volcanoes.iter().map(|v| v.radius.cos()).collect();
    let mut ridge_cells = vec![false; num_cells];

    ridge_cells
        .par_iter_mut()
        .zip(elevations.par_iter_mut())
        .zip(plate_ids.par_iter_mut())
        .zip(volcano_mask.par_iter_mut())
        .zip(lithology.par_iter_mut())
        .enumerate()
        .for_each(
            |(
                cell_id,
                ((((ridge_out, elevation_out), plate_id_out), volcano_mask_out), lithology_out),
            )| {
                let cell = &topology.cells[cell_id];
                let dir = cell.center;

                // 1. Искажение координат (Domain Warping).
                let p = dir.as_dvec3() * config.warp_scale;
                let offsets = &config.warp_offsets;

                let warp_x = noise.get([
                    p.x + offsets[0][0],
                    p.y + offsets[0][1],
                    p.z + offsets[0][2],
                ]) as f32;
                let warp_y = noise.get([
                    p.x + offsets[1][0],
                    p.y + offsets[1][1],
                    p.z + offsets[1][2],
                ]) as f32;
                let warp_z = noise.get([
                    p.x + offsets[2][0],
                    p.y + offsets[2][1],
                    p.z + offsets[2][2],
                ]) as f32;

                let warp_noise = Vec3::new(warp_x, warp_y, warp_z) * config.warp_strength;
                let warped_dir = (dir + warp_noise).normalize();

                // 2. Находим 3 ближайшие плиты для текущей ячейки (триада).
                let (p1_idx, p2_idx, p3_idx, dist_1, dist_2, dist_3) =
                    find_closest_plates(warped_dir, plates);
                *plate_id_out = p1_idx;

                let p1 = &plates[p1_idx];
                let p2 = &plates[p2_idx];
                let p3 = &plates[p3_idx];

                let bd2 = dist_2 - dist_1;
                let bd3 = dist_3 - dist_1;

                // 3. Вычисляем веса методом обратного расстояния (IDW).
                let eps = config.weight_epsilon;
                let w2 = 1.0 / (bd2.powi(2) + eps);
                let w3 = 1.0 / (bd3.powi(2) + eps);
                let w_sum = w2 + w3;

                let weight2 = w2 / w_sum;
                let weight3 = w3 / w_sum;

                // 4. Базовая высота (пляжи, шельфы) для двух потенциальных границ (1-2 и 1-3)
                let mut h2 = calculate_base_elevation(p1, p2, bd2, config);
                let mut h3 = calculate_base_elevation(p1, p3, bd3, config);

                let v1 = p1.euler_pole.cross(dir) * p1.angular_vel;

                // 5. Генерируем макро-шум, из которого будут лепиться горы и неровности
                let m_scale = config.boundary_noise_scale;
                let m_coords = [
                    warped_dir.x as f64 * m_scale,
                    warped_dir.y as f64 * m_scale,
                    warped_dir.z as f64 * m_scale,
                ];
                let raw_m_noise = noise.get(m_coords) as f32;
                let m_noise = config.boundary_noise_base + raw_m_noise * config.boundary_noise_amp;

                let mut f2_val = 0.0_f32;
                let mut conv2_val = 0.0_f32;
                let mut f3_val = 0.0_f32;
                let mut conv3_val = 0.0_f32;

                // 6. Граница с плитой 2
                if bd2 < config.edge_threshold {
                    let f2 = (1.0 - bd2 / config.edge_threshold).clamp(0.0, 1.0);
                    let normal2 = (p2.center_dir - p1.center_dir).normalize_or_zero();
                    let v2 = p2.euler_pole.cross(dir) * p2.angular_vel;
                    let convergence2 = (v1 - v2).dot(normal2) / config.convergence_ref_speed;

                    f2_val = f2;
                    conv2_val = convergence2;

                    if p1.plate_type == PlateType::Oceanic
                        && p2.plate_type == PlateType::Oceanic
                        && convergence2 < 0.0
                    {
                        *ridge_out = true;
                    }

                    h2 += calculate_boundary_elevation(
                        p1.plate_type,
                        p2.plate_type,
                        f2,
                        convergence2,
                        m_noise,
                        config,
                    );
                }

                // 7. Граница с плитой 3 — аналогично
                if bd3 < config.edge_threshold {
                    let f3 = (1.0 - bd3 / config.edge_threshold).clamp(0.0, 1.0);
                    let normal3 = (p3.center_dir - p1.center_dir).normalize_or_zero();
                    let v3 = p3.euler_pole.cross(dir) * p3.angular_vel;
                    let convergence3 = (v1 - v3).dot(normal3) / config.convergence_ref_speed;

                    f3_val = f3;
                    conv3_val = convergence3;

                    if p1.plate_type == PlateType::Oceanic
                        && p3.plate_type == PlateType::Oceanic
                        && convergence3 < 0.0
                    {
                        *ridge_out = true;
                    }

                    h3 += calculate_boundary_elevation(
                        p1.plate_type,
                        p3.plate_type,
                        f3,
                        convergence3,
                        m_noise,
                        config,
                    );
                }

                // 8. Смешиваем высоты от двух границ для получения гладкого тройного стыка
                let h1 = h2 * weight2 + h3 * weight3;

                // 9. Наложение горячих точек (мантийных плюмов)
                let mut max_volcano_h: f32 = 0.0;
                let mut is_volcano = false;

                let v_scale = config.volcano_noise_scale;
                let v_coords = [
                    warped_dir.x as f64 * v_scale,
                    warped_dir.y as f64 * v_scale,
                    warped_dir.z as f64 * v_scale,
                ];
                let raw_v_noise = noise.get(v_coords) as f32;
                let v_noise_mod = config.volcano_noise_base + config.volcano_noise_mult * raw_v_noise;

                for (v, &cos_radius) in volcanoes.iter().zip(&volcano_cos_radii) {
                    let dot_prod = warped_dir.dot(v.center).clamp(-1.0, 1.0);

                    // Для обычного углового радиуса [0, PI] проверка через заранее
                    // вычисленный cos позволяет пропустить acos для точек вне вулкана.
                    let inside = if (0.0..=PI).contains(&v.radius) {
                        dot_prod > cos_radius
                    } else {
                        dot_prod.acos() < v.radius
                    };

                    if inside {
                        let d = dot_prod.acos();
                        let f = 1.0 - (d / v.radius);
                        let profile = f.powi(config.volcano_profile_pow);
                        let local_h = v.height * profile * v_noise_mod;
                        max_volcano_h = max_volcano_h.max(local_h);

                        if d < v.radius * 0.70 {
                            is_volcano = true;
                        }
                    }
                }

                *volcano_mask_out = is_volcano;

                let h = h1.max(h1 + max_volcano_h);

                // 10. Финальный проход: накладываем мелкую текстуру (детализацию) на всю планету
                let d_scale = config.detail_noise_scale;
                let detail_coords = [
                    warped_dir.x as f64 * d_scale,
                    warped_dir.y as f64 * d_scale,
                    warped_dir.z as f64 * d_scale,
                ];
                let detail_noise = noise.get(detail_coords) as f32;

                *elevation_out =
                    (h + detail_noise * config.detail_noise_amp).max(config.elevation_min_clamp);

                // 11. Первичная коренная литология (по доминирующей границе и расстоянию в радианах)
                let (dom_t2, dom_f, dom_conv) =
                    if f3_val * weight3 * conv3_val.abs() > f2_val * weight2 * conv2_val.abs() {
                        (p3.plate_type, f3_val, conv3_val)
                    } else {
                        (p2.plate_type, f2_val, conv2_val)
                    };

                *lithology_out = classify_base_lithology(
                    p1.plate_type,
                    dom_t2,
                    dom_f,
                    dom_conv,
                    raw_m_noise,
                    detail_noise,
                    config,
                );
            },
        );

    apply_oceanic_crust_cooling(topology, &mut geology.elevations, &ridge_cells, config);

    // Запускаем клеточный автомат для размытия острых ступенек пикселей
    step_elevation_diffusion(
        topology,
        &mut geology.elevations,
        config.diffusion_strength,
        config.slope_erosion_factor,
        config.diffusion_max_strength,
    );

    // 12. Наложение поверхностных слоев: вулканизм, осадочные бассейны, карбонатные шельфы и абиссальные илы
    apply_lithology_overlays(topology, geology, &ridge_cells, noise, config);
    compute_terrain_ruggedness(topology, geology, noise);
}

/// Вычисляет масштабонезависимую рельефность (изрезанность) каждой клетки [0.0 .. 1.0]
pub(crate) fn compute_terrain_ruggedness(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
) {
    let num_cells = topology.cells.len();
    geology.terrain_ruggedness.resize(num_cells, 0.0);

    let elevations = &geology.elevations;
    let lithology = &geology.lithology;
    let volcano_mask = &geology.volcano_mask;
    geology.terrain_ruggedness.par_iter_mut().enumerate().for_each(|(i, ruggedness_out)| {
        let cell = &topology.cells[i];
        let elev = elevations[i];
        let rock = lithology[i];

        // 1. Масштабонезависимый градиент высот (крутизна макро-склона в пересчете на радиан сферы)
        let mut max_angular_grad = 0.0_f32;
        let mut mean_angular_grad = 0.0_f32;
        let n_count = cell.neighbors.len().max(1) as f32;

        for &n_id in &cell.neighbors {
            let step_rad = cell
                .center
                .dot(topology.cells[n_id].center)
                .clamp(-1.0, 1.0)
                .acos()
                .max(1e-4);
            let grad = (elev - elevations[n_id]).abs() / step_rad;
            if grad > max_angular_grad {
                max_angular_grad = grad;
            }
            mean_angular_grad += grad;
        }
        mean_angular_grad /= n_count;

        // Нормируем угловой градиент (на горных хребтах grad ~ 4.0..12.0)
        let slope_score = (max_angular_grad * 0.095 + mean_angular_grad * 0.065).clamp(0.0, 0.78);
        // 2. Внутриклеточная расчлененность (каньоны, ущелья, скальные гребни) из высокочастотного 3D-шума
        let d = cell.center.as_dvec3();
        let crag_a =
            (noise.get([d.x * 26.0 + 13.4, d.y * 26.0 - 27.8, d.z * 26.0 + 41.2]) as f32).abs();
        let crag_b =
            (noise.get([d.x * 52.0 - 31.9, d.y * 52.0 + 18.5, d.z * 52.0 - 22.7]) as f32).abs();
        let micro_crags = (crag_a * 0.65 + crag_b * 0.35).clamp(0.0, 1.0);

        // Твердые породы и карстовые известняки сохраняют острые формы ущелий и скал, а мягкие глины сглаживаются
        let rock_retention = match rock {
            RockType::Granite => 1.00,      // Скальные батолиты и пики
            RockType::Andesite => 0.95,     // Изрезанные стратовулканы и лавовые купола
            RockType::GneissSchist => 0.88, // Складчатые хребты
            RockType::FloodBasalt => 0.80,  // Ступенчатые трапповые плато и каньоны (тип Путорана)
            RockType::Limestone => 0.68,    // Карстовые воронки, каньоны и останцы
            RockType::Sandstone => 0.50,    // Овраги и столовые возвышенности
            RockType::OceanicBasalt => 0.45,
            RockType::ShaleClay | RockType::PelagicSediment => 0.15, // Пологие глинистые равнины
        };

        // В океане учитываем подводные хребты и желоба, на суше — высотную поясность и вулканы
        let elev_prominence = if elev > 0.0 {
            ((elev - 0.08).max(0.0) * 1.45).clamp(0.0, 0.45)
        } else {
            0.0
        };
        let volcano_roughness = if volcano_mask[i] && elev > 0.0 {
            0.18
        } else {
            0.0
        };

        let micro_score =
            micro_crags * rock_retention * (0.14 + slope_score * 0.55 + elev_prominence * 0.60);

        *ruggedness_out =
            (slope_score + elev_prominence * 0.55 + volcano_roughness + micro_score)
                .clamp(0.0, 1.0);
    });
}

/// Определяет базовую породу фундамента по 7 типам границ и удалению вглубь плиты
fn classify_base_lithology(
    t1: PlateType,
    t2: PlateType,
    f: f32,
    convergence: f32,
    raw_m_noise: f32,
    detail_noise: f32,
    config: &TectonicsConfig,
) -> RockType {
    // 1. Глубоко внутри плиты или вдоль спокойного трансформного разлома
    if f <= 0.05 || convergence.abs() < 0.08 {
        return match t1 {
            PlateType::Oceanic => RockType::OceanicBasalt,
            PlateType::Continental => {
                // Древний континентальный кратон: выходы кристаллического щита (гнейсы/граниты)
                // на возвышениях фундамента и осадочный чехол (песчаники) на платформе
                let basement_exposure = raw_m_noise + detail_noise * 0.35;
                if basement_exposure > 0.52 {
                    RockType::Granite
                } else if basement_exposure > 0.18 {
                    RockType::GneissSchist
                } else {
                    RockType::Sandstone
                }
            }
        };
    }

    // Слегка возмущаем координату `f` шумом, чтобы границы поясов пород были естественными, а не линейными
    let f_noisy = (f + raw_m_noise * 0.10 + detail_noise * 0.06).clamp(0.0, 1.0);

    match (t1, t2) {
        // --- 1. СБЛИЖЕНИЕ КОНТИНЕНТАЛЬНЫХ (Коллизия: гранит -> гнейс/сланец -> песчаник) ---
        (PlateType::Continental, PlateType::Continental) if convergence > 0.0 => {
            if f_noisy > 0.62 {
                if detail_noise > -0.20 {
                    RockType::Granite
                } else {
                    RockType::GneissSchist
                }
            } else if f_noisy > 0.25 {
                RockType::GneissSchist
            } else {
                RockType::Sandstone
            }
        }

        // --- 2. СУБДУКЦИЯ, КОНТИНЕНТАЛЬНАЯ СТОРОНА (Магматическая дуга Андского типа) ---
        (PlateType::Continental, PlateType::Oceanic) if convergence > 0.0 => {
            // Используем то же смещение дуги от желоба, что и в calculate_boundary_elevation
            let offset_zone = f - config.co_col_offset;
            let m_zone = (offset_zone * config.co_col_scale).clamp(0.0, 1.0);
            let arc_profile = (m_zone * PI).sin().powi(2);

            if arc_profile > 0.50 {
                // Осевая зона хребта: стратовулканы (андезит) и вскрытые плутоны (гранит)
                if raw_m_noise + detail_noise * 0.4 > 0.05 {
                    RockType::Andesite
                } else {
                    RockType::Granite
                }
            } else if arc_profile > 0.15 {
                // Склоны дуги: метаморфический пояс
                RockType::GneissSchist
            } else {
                // Преддуговой прогиб у берега и предгорный прогиб в тылу дуги
                RockType::Sandstone
            }
        }

        // --- 3. СУБДУКЦИЯ, ОКЕАНСКАЯ СТОРОНА (Глубоководный жёлоб) ---
        (PlateType::Oceanic, PlateType::Continental) if convergence > 0.0 => {
            let offset_dist = (f - config.oc_col_center).abs();
            let trench_profile = (1.0 - offset_dist * config.oc_col_scale).clamp(0.0, 1.0);

            if trench_profile > 0.25 {
                RockType::PelagicSediment // Кремнистые и глинистые осадки жёлоба
            } else {
                RockType::OceanicBasalt
            }
        }

        // --- 4. СБЛИЖЕНИЕ ОКЕАНИЧЕСКИХ (Островная вулканическая дуга) ---
        (PlateType::Oceanic, PlateType::Oceanic) if convergence > 0.0 => {
            if f_noisy > 0.38 {
                RockType::Andesite
            } else {
                RockType::OceanicBasalt
            }
        }

        // --- 5. РАСХОЖДЕНИЕ КОНТИНЕНТАЛЬНЫХ (Континентальный рифт) ---
        (PlateType::Continental, PlateType::Continental) if convergence < 0.0 => {
            if f_noisy > 0.58 {
                // Дно рифтового грабена: базальтовые излияния и озерно-речные пески
                if detail_noise > -0.25 {
                    RockType::FloodBasalt
                } else {
                    RockType::Sandstone
                }
            } else if f_noisy > 0.25 {
                // Поднятые плечи рифта (горсты)
                RockType::GneissSchist
            } else {
                RockType::Sandstone
            }
        }

        // --- 6. РАСХОЖДЕНИЕ ОКЕАН-КОНТИНЕНТ (Пассивная континентальная окраина) ---
        (PlateType::Continental, PlateType::Oceanic)
        | (PlateType::Oceanic, PlateType::Continental)
            if convergence < 0.0 =>
        {
            if f_noisy > 0.45 {
                RockType::ShaleClay
            } else {
                RockType::Sandstone
            }
        }

        // --- 7. РАСХОЖДЕНИЕ ОКЕАНИЧЕСКИХ (Срединно-океанический хребет) ---
        (PlateType::Oceanic, PlateType::Oceanic) if convergence < 0.0 => {
            RockType::OceanicBasalt // Свежий базальт MORB
        }

        _ => match t1 {
            PlateType::Oceanic => RockType::OceanicBasalt,
            PlateType::Continental => RockType::GneissSchist,
        },
    }
}

/// Накладывает вулканизм, осадочные бассейны и карбонатные платформы поверх готового рельефа
fn apply_lithology_overlays(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    ridge_cells: &[bool],
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let elevations = &geology.elevations;
    let plate_ids = &geology.plate_ids;
    let volcano_mask = &geology.volcano_mask;
    let plates = &geology.plates;
    let lithology = &mut geology.lithology;

    lithology.par_iter_mut().enumerate().for_each(|(i, rock_out)| {
        let r = ridge_cells[i];
        let elev = elevations[i];
        let plate_type = plates[plate_ids[i]].plate_type;
        let lat_abs = topology.cells[i].lat_lon.x.abs();

        // 1. Вулканизм горячих точек (мантийных плюмов) перекрывает коренные породы
        if volcano_mask[i] {
            *rock_out = if plate_type == PlateType::Oceanic {
                RockType::FloodBasalt // Океанические щитовые вулканы (Гавайский тип)
            } else if elev > config.cont_base_height + 0.14 {
                RockType::Andesite // Высокие континентальные стратовулканы / кальдеры
            } else {
                RockType::FloodBasalt // Континентальные базальтовые плато (траппы)
            };
            return;
        }

        // Вычисляем локальный перепад высот (уклон) по соседям
        let neighbors = &topology.cells[i].neighbors;
        let mut max_slope = 0.0_f32;
        for &n_id in neighbors {
            let diff = (elev - elevations[n_id]).abs();
            if diff > max_slope {
                max_slope = diff;
            }
        }

        let dir = topology.cells[i].center.as_dvec3();
        let litho_noise =
            noise.get([dir.x * 6.5 + 31.7, dir.y * 6.5 - 19.3, dir.z * 6.5 + 47.1]) as f32;

        // 2. Морское дно (Ниже уровня океана)
        if elev <= 0.0 {
            // Тёплые мелководные шельфы (тропики и субтропики до ~35° широты) -> Известняки (рифовые/карбонатные банки)
            if elev > -0.09 + litho_noise * 0.015
                && lat_abs < 0.62 + litho_noise * 0.06
                && !r
                && *rock_out != RockType::Andesite
            {
                *rock_out = RockType::Limestone;
            }
            // Глубокие старые абиссальные равнины вдали от СОХ -> Пелагические осадки
            else if elev < config.ocean_base_height - 0.05 + litho_noise * 0.02
                && !r
                && *rock_out == RockType::OceanicBasalt
            {
                *rock_out = RockType::PelagicSediment;
            }
            return;
        }

        // 3. Суша (Выше уровня океана)
        // Если участок бывшего морского дна поднялся выше уровня моря, превращаем илы/MORB в осадочные породы суши
        if *rock_out == RockType::PelagicSediment
            || *rock_out == RockType::OceanicBasalt
        {
            *rock_out =
                if lat_abs < 0.60 + litho_noise * 0.06 && elev < config.cont_base_height * 0.8 {
                    RockType::Limestone
                } else {
                    RockType::ShaleClay
                };
        }

        // Карбонатные платформы на тёплых прибрежных равнинах с малым уклоном
        if elev < config.cont_base_height * 0.78
            && max_slope < 0.018
            && lat_abs < 0.58 + litho_noise * 0.05
            && matches!(
                *rock_out,
                RockType::Sandstone | RockType::ShaleClay
            )
            && litho_noise > 0.12
        {
            *rock_out = RockType::Limestone;
            return;
        }

        // Осадконакопление в плоских низинах и межгорных котловинах
        if elev < config.cont_base_height * (0.90 + litho_noise * 0.08) && max_slope < 0.014 {
            if max_slope < 0.006 && elev < config.cont_base_height * (0.55 + litho_noise * 0.08) {
                // Плоские низменности — глинистые отложения
                *rock_out = RockType::ShaleClay;
            } else if matches!(
                *rock_out,
                RockType::Granite | RockType::GneissSchist
            ) {
                // Пологие равнины поверх древнего фундамента перекрываются песчаным осадочным чехлом
                *rock_out = RockType::Sandstone;
            }
        }
    });
}

/// Находит ровно 3 ближайшие плиты к данной точке.
fn find_closest_plates(dir: Vec3, plates: &[Plate]) -> (usize, usize, usize, f32, f32, f32) {
    // acos монотонно убывает: сортируем по dot product, не вычисляя угол
    // для каждой плиты. Углы нужны только для трёх выбранных результатов.
    let mut dot_1 = f32::NEG_INFINITY;
    let mut dot_2 = f32::NEG_INFINITY;
    let mut dot_3 = f32::NEG_INFINITY;

    let mut p1_idx = 0;
    let mut p2_idx = 0;
    let mut p3_idx = 0;

    for (i, plate) in plates.iter().enumerate() {
        let dot = dir.dot(plate.center_dir).clamp(-1.0, 1.0);

        if dot > dot_1 {
            dot_3 = dot_2;
            p3_idx = p2_idx;
            dot_2 = dot_1;
            p2_idx = p1_idx;
            dot_1 = dot;
            p1_idx = i;
        } else if dot > dot_2 {
            dot_3 = dot_2;
            p3_idx = p2_idx;
            dot_2 = dot;
            p2_idx = i;
        } else if dot > dot_3 {
            dot_3 = dot;
            p3_idx = i;
        }
    }

    (
        p1_idx,
        p2_idx,
        p3_idx,
        if dot_1.is_finite() {
            dot_1.clamp(-1.0, 1.0).acos()
        } else {
            f32::MAX
        },
        if dot_2.is_finite() {
            dot_2.clamp(-1.0, 1.0).acos()
        } else {
            f32::MAX
        },
        if dot_3.is_finite() {
            dot_3.clamp(-1.0, 1.0).acos()
        } else {
            f32::MAX
        },
    )
}

/// Вычисляет плавный переход высот между равнинами и океанами (шельфы, побережья)
fn calculate_base_elevation(
    p1: &Plate,
    p2: &Plate,
    boundary_dist: f32,
    config: &TectonicsConfig,
) -> f32 {
    let h_base = match p1.plate_type {
        PlateType::Continental => config.cont_base_height,
        PlateType::Oceanic => config.ocean_base_height,
    };
    let sm = config.base_profile_smoothness;

    match (p1.plate_type, p2.plate_type) {
        (PlateType::Continental, PlateType::Oceanic) => {
            let factor = profile_factor(boundary_dist / config.base_co_dist, sm);
            let offset = (config.base_co_offset + config.base_oc_offset) * 0.5;
            h_base * factor - (1.0 - factor) * offset
        }
        (PlateType::Oceanic, PlateType::Continental) => {
            let factor = profile_factor(boundary_dist / config.base_oc_dist, sm);
            let offset = (config.base_co_offset + config.base_oc_offset) * 0.5;
            h_base * factor - (1.0 - factor) * offset
        }
        (PlateType::Continental, PlateType::Continental) => {
            let factor = profile_factor(boundary_dist / config.base_cc_dist, sm);
            h_base * factor + (1.0 - factor) * config.base_cc_offset
        }
        (PlateType::Oceanic, PlateType::Oceanic) => {
            let factor = profile_factor(boundary_dist / config.base_oo_dist, sm);
            h_base * factor + (1.0 - factor) * config.base_oo_offset
        }
    }
}

/// Выполняет расчет рельефа, возникающего непосредственно от тектонического столкновения или растяжения
fn calculate_boundary_elevation(
    t1: PlateType,
    t2: PlateType,
    f: f32,
    convergence: f32,
    mountain_noise: f32,
    config: &TectonicsConfig,
) -> f32 {
    let conv_factor = convergence.clamp(0.0, 1.0).exp();

    match (t1, t2) {
        (PlateType::Continental, PlateType::Continental) if convergence > 0.0 => {
            let angle = f * PI * config.cc_col_freq;
            let shape = angle.sin().powi(2);
            let blend = config.cc_col_base + config.cc_col_f_mult * f;

            shape * blend * config.cc_col_amp * mountain_noise * conv_factor
        }

        (PlateType::Continental, PlateType::Oceanic) if convergence > 0.0 => {
            let offset_zone = f - config.co_col_offset;
            let m_zone = (offset_zone * config.co_col_scale).clamp(0.0, 1.0);

            (m_zone * PI).sin().powi(2) * config.co_col_amp * mountain_noise * conv_factor
        }

        (PlateType::Oceanic, PlateType::Continental) if convergence > 0.0 => {
            let offset_dist = (f - config.oc_col_center).abs();
            let profile = (1.0 - offset_dist * config.oc_col_scale).clamp(0.0, 1.0);

            -profile * config.oc_col_amp * conv_factor
        }

        (PlateType::Oceanic, PlateType::Oceanic) if convergence > 0.0 => {
            (f * PI).sin() * config.oo_col_amp * mountain_noise * conv_factor
        }

        (PlateType::Continental, PlateType::Continental) if convergence < 0.0 => {
            let graben = -f.powi(config.cc_rift_graben_pow) * config.cc_rift_graben_amp;
            let horsts = (f * PI).sin() * config.cc_rift_horst_amp;

            (horsts + graben) * config.cc_rift_total_amp * mountain_noise * conv_factor
        }

        (PlateType::Continental, PlateType::Oceanic) if convergence < 0.0 => {
            let offset_zone = f - config.co_rift_offset;
            let m_zone = (offset_zone * config.co_rift_scale).clamp(0.0, 1.0);

            -(m_zone * PI).sin().powi(2) * config.co_rift_amp * mountain_noise * conv_factor
        }

        (PlateType::Oceanic, PlateType::Oceanic) if convergence < 0.0 => {
            let axial_rift = -f.powi(config.oo_rift_axial_pow) * config.oo_rift_axial_amp;
            let ridges = (f * PI).sin() * config.oo_rift_ridge_amp;
            let noise_mod = config.oo_rift_noise_base + config.oo_rift_noise_mult * mountain_noise;

            (ridges + axial_rift) * config.oo_rift_total_amp * noise_mod * conv_factor
        }

        _ => 0.0,
    }
}

/// Клеточный автомат для диффузии (размытия) высот, избавляет от пиксельных ступенек на сетке
pub fn step_elevation_diffusion(
    topology: &WorldTopology,
    elevations: &mut [f32],
    strength: f32,
    slope_erosion_factor: f32,
    max_strength: f32,
) {
    let old_elevations = elevations.to_vec();

    elevations.par_iter_mut().enumerate().for_each(|(i, elevation_out)| {
        let cell = &topology.cells[i];
        if cell.neighbors.is_empty() {
            return;
        }
        // Сумму высот и максимальный уклон можно получить за один обход соседей.
        let mut neighbor_sum = 0.0_f32;
        let mut max_slope = 0.0_f32;
        let current_elevation = old_elevations[i];

        for &n_id in &cell.neighbors {
            let neighbor_elevation = old_elevations[n_id];
            neighbor_sum += neighbor_elevation;
            max_slope = max_slope.max((current_elevation - neighbor_elevation).abs());
        }

        let avg_neighbor = neighbor_sum / cell.neighbors.len() as f32;

        let local_strength = (strength + max_slope * slope_erosion_factor).min(max_strength);

        *elevation_out = old_elevations[i] * (1.0 - local_strength) + avg_neighbor * local_strength;
    });
}

fn apply_oceanic_crust_cooling(
    topology: &WorldTopology,
    elevations: &mut [f32],
    ridge_cells: &[bool],
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let mut dist_from_ridge = vec![u32::MAX; num_cells];
    let mut queue = VecDeque::new();

    for i in 0..num_cells {
        if ridge_cells[i] {
            dist_from_ridge[i] = 0;
            queue.push_back(i);
        }
    }

    while let Some(cell_id) = queue.pop_front() {
        let current_dist = dist_from_ridge[cell_id];
        for &neighbor_id in &topology.cells[cell_id].neighbors {
            if elevations[neighbor_id] <= 0.0 && dist_from_ridge[neighbor_id] > current_dist + 1 {
                dist_from_ridge[neighbor_id] = current_dist + 1;
                queue.push_back(neighbor_id);
            }
        }
    }

    let max_dist = config.oceanic_age_max_distance.max(1.0);
    elevations.par_iter_mut().enumerate().for_each(|(i, elevation)| {
        if *elevation <= 0.0 && dist_from_ridge[i] != u32::MAX {
            let age_factor = smoothstep((dist_from_ridge[i] as f32) / max_dist);
            *elevation -= age_factor * config.oceanic_age_depth_gain;
        }
    });
}

/// Создает цепочки вулканов, вращая статичные мантийные плюмы (горячие точки) по дуге
pub fn generate_hotspots(
    rng: &mut impl RngExt,
    plates: &[Plate],
    config: &TectonicsConfig,
) -> Vec<Volcano> {
    let mut volcanoes = Vec::new();

    for _ in 0..config.hotspot_count {
        let plume_pos = Vec3::new(
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
        )
        .normalize_or_zero();

        if plume_pos == Vec3::ZERO {
            continue;
        }

        // acos монотонно убывает: ближайшая плита имеет максимальный dot product.
        let mut best_dot = f32::NEG_INFINITY;
        let mut plate_idx = 0;

        for (i, plate) in plates.iter().enumerate() {
            let dot_prod = plume_pos.dot(plate.center_dir);
            if dot_prod > best_dot {
                best_dot = dot_prod;
                plate_idx = i;
            }
        }

        let plate = &plates[plate_idx];
        let chain_length = rng.random_range(config.hotspot_chain_min..config.hotspot_chain_max);
        let time_step = config.hotspot_time_step;

        for step in 0..chain_length {
            let angle = plate.angular_vel * (step as f32) * time_step;
            let rotation = Quat::from_axis_angle(plate.euler_pole, angle);
            let volcano_pos = rotation * plume_pos;

            let age_factor = 1.0 - (step as f32 / chain_length as f32);
            let height_base =
                rng.random_range(config.hotspot_height_min..config.hotspot_height_max);

            let height = height_base * age_factor.powf(config.hotspot_erosion_pow);
            let radius = (config.hotspot_radius_base
                + height * config.volcano_radius_per_height
                + config.hotspot_radius_growth * (1.0 - age_factor))
                .max(config.volcano_radius_min);

            volcanoes.push(Volcano {
                center: volcano_pos,
                height,
                radius,
            });
        }
    }
    volcanoes
}
