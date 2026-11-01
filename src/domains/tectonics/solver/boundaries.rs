// src/domains/tectonics/solver/boundaries.rs

use super::{smoothstep, TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{GeologyLayer, Volcano},
    },
    world::topology::WorldTopology,
};
use bevy::math::Vec3;
use rayon::prelude::*;
use std::collections::VecDeque;

/// Находит пограничные клетки и распространяет вектор скорости схождения/расхождения
/// вглубь плит на физическую дистанцию `config.edge_threshold` (в радианах)
pub fn detect_plate_boundaries(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &GeologyLayer,
    scratch: &mut TectonicsScratch,
) {
    let num_cells = topology.cells.len();
    scratch.boundary_convergence.fill(0.0);
    scratch.boundary_shear.fill(0.0);
    scratch.neighbor_plate.fill(usize::MAX);
    scratch.boundary_normal.fill(Vec3::ZERO);

    let mut dist_rad = vec![f32::MAX; num_cells];
    let mut queue = VecDeque::new();

    // 1. Инициализируем непосредственные клетки контакта двух разных плит
    for i in 0..num_cells {
        let p_i = geology.plate_ids[i];
        let r_i = topology.cells[i].center;
        let v_i = geology.plates[p_i].omega.cross(r_i);

        let start = cache.edge_offsets[i];
        let end = cache.edge_offsets[i + 1];

        let mut best_conv = 0.0_f32;
        let mut best_shear = 0.0_f32;
        let mut best_other = usize::MAX;
        let mut best_normal = Vec3::ZERO;

        for e in start..end {
            let j = cache.edge_neighbors[e];
            let p_j = geology.plate_ids[j];
            if p_j != p_i {
                let n_ij = cache.edge_tangent_dirs[e]; // от i к j
                let v_j = geology.plates[p_j].omega.cross(topology.cells[j].center);
                let rel_v = v_i - v_j;

                let conv = rel_v.dot(n_ij);
                let shear = (rel_v - n_ij * conv).length();

                if best_other == usize::MAX || conv.abs() > best_conv.abs() {
                    best_conv = conv;
                    best_shear = shear;
                    best_other = p_j;
                    best_normal = n_ij;
                }
            }
        }

        if best_other != usize::MAX {
            dist_rad[i] = 0.0;
            scratch.boundary_convergence[i] = best_conv;
            scratch.boundary_shear[i] = best_shear;
            scratch.neighbor_plate[i] = best_other;
            scratch.boundary_normal[i] = best_normal;
            queue.push_back(i);
        }
    }

    // 2. Волновой перенос параметров границы вглубь плиты на `config.edge_threshold` радиан
    let max_rad = 0.24_f32;
    while let Some(curr) = queue.pop_front() {
        let d_curr = dist_rad[curr];
        if d_curr >= max_rad {
            continue;
        }
        let start = cache.edge_offsets[curr];
        let end = cache.edge_offsets[curr + 1];

        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            let next_d = d_curr + cache.edge_dists_rad[e];
            if next_d < dist_rad[nbr] && next_d <= max_rad {
                dist_rad[nbr] = next_d;
                let decay = 1.0 - (next_d / max_rad);
                scratch.boundary_convergence[nbr] = scratch.boundary_convergence[curr] * decay;
                scratch.boundary_shear[nbr] = scratch.boundary_shear[curr] * decay;
                scratch.neighbor_plate[nbr] = scratch.neighbor_plate[curr];
                scratch.boundary_normal[nbr] = scratch.boundary_normal[curr];
                queue.push_back(nbr);
            }
        }
    }
}

pub fn apply_boundary_and_plume_processes(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let ref_speed = config.angular_vel_max.max(0.5);

    // Предварительный снимок плотности и толщины для чтения соседей без гонок данных
    scratch.next_thickness.copy_from_slice(&geology.crust_thickness);
    scratch.next_density.copy_from_slice(&geology.crust_density);

    let old_thick = &scratch.next_thickness;
    let old_dens = &scratch.next_density;
    let conv_slice = &scratch.boundary_convergence;
    let shear_slice = &scratch.boundary_shear;
    let other_slice = &scratch.neighbor_plate;
    let plumes = &geology.plumes;

    let thickness = &mut geology.crust_thickness;
    let density = &mut geology.crust_density;
    let age = &mut geology.crust_age;
    let sediment = &mut geology.sediment_thickness;
    let stress = &mut geology.tectonic_stress;
    let orogeny = &mut geology.orogeny_root;

    thickness
        .par_iter_mut()
        .zip(density.par_iter_mut())
        .zip(age.par_iter_mut())
        .zip(sediment.par_iter_mut())
        .zip(stress.par_iter_mut())
        .zip(orogeny.par_iter_mut())
        .enumerate()
        .for_each(
            |(
                i,
                (((((thick_out, dens_out), age_out), sed_out), stress_out), orog_out),
            )| {
                // 1. Естественное старение коры и охлаждение океанической литосферы
                *age_out += dt;
                *orog_out *= (1.0 - 0.12 * dt).max(0.92);

                let is_cont = *dens_out < 2825.0 || *thick_out >= 19.0;
                if !is_cont {
                    let cooling = (*age_out / config.oceanic_mature_age).clamp(0.0, 1.0).sqrt();
                    let target_dens = config.young_ocean_density
                        + cooling * (config.old_ocean_density - config.young_ocean_density);
                    *dens_out = *dens_out * (1.0 - 0.35 * dt) + target_dens * (0.35 * dt);
                }

                // 2. Граничные процессы (Спрединг, Рифтинг, Субдукция, Коллизия)
                let other_p = other_slice[i];
                let conv = (conv_slice[i] / ref_speed).clamp(-1.5, 1.5);
                let shear = (shear_slice[i] / ref_speed).clamp(0.0, 1.5);
                *stress_out = conv;

                if other_p != usize::MAX {
                    // Оцениваем среднюю плотность и толщину соседа через границу
                    let start = cache.edge_offsets[i];
                    let end = cache.edge_offsets[i + 1];
                    let mut opp_dens = *dens_out;
                    let mut opp_thick = *thick_out;
                    for e in start..end {
                        let nbr = cache.edge_neighbors[e];
                        if geology.plate_ids[nbr] == other_p {
                            opp_dens = old_dens[nbr];
                            opp_thick = old_thick[nbr];
                            break;
                        }
                    }
                    let opp_is_cont = opp_dens < 2825.0 || opp_thick >= 19.0;

                    if conv < -0.04 {
                        // --- ДИВЕРГЕНЦИЯ (РАСХОЖДЕНИЕ) ---
                        let div_mag = (-conv).clamp(0.0, 1.5);
                        if is_cont {
                            // Континентальный рифтогенез: утончаем кору
                            *thick_out = (*thick_out
                                - config.rift_thinning_rate * div_mag * dt)
                                .max(config.ocean_crust_thickness_km);
                            *orog_out = (*orog_out + 0.25 * div_mag * dt).min(0.65);

                            // Если континентальная кора истончилась ниже 11 км — она рвется,
                            // и рождается новый молодой океанический бассейн (как Красное море!)
                            if *thick_out <= 11.0 {
                                *thick_out = config.ocean_crust_thickness_km;
                                *dens_out = config.young_ocean_density;
                                *age_out = 0.0;
                                *sed_out *= 0.25;
                            }
                        } else {
                            // Срединно-океанический хребет (СОХ): рождение свежей коры MORB
                            *thick_out = config.ocean_crust_thickness_km;
                            *dens_out = config.young_ocean_density;
                            *age_out = 0.0;
                            *sed_out = 0.02;
                        }
                    } else if conv > 0.04 {
                        // --- КОНВЕРГЕНЦИЯ (СХОЖДЕНИЕ) ---
                        let conv_mag = conv.clamp(0.0, 1.5);

                        if is_cont && opp_is_cont {
                            // 1. КОНТИНЕНТАЛЬНАЯ КОЛЛИЗИЯ (Гималаи / Тибет):
                            // Оба края легкие — субдукция невозможна, кора мощно сжимается и растет вверх
                            *thick_out = (*thick_out
                                + config.convergence_thickening_rate * 1.35 * conv_mag * dt)
                                .min(config.max_crust_thickness_km);
                            *dens_out = (*dens_out - 45.0 * dt).max(config.cont_crust_density - 25.0);
                            *orog_out = (*orog_out + 0.95 * conv_mag * dt).min(1.0);
                        } else if !is_cont && (opp_is_cont || *dens_out >= opp_dens) {
                            // 2. СУБДУЦИРУЮЩАЯ ОКЕАНИЧЕСКАЯ СТОРОНА (Глубоководный желоб):
                            // Океаническая плита прогибается вниз под континент или более молодую плиту,
                            // а осадки сдираются в аккреционную призму
                            *sed_out *= (1.0 - 0.45 * conv_mag * dt).max(0.2);
                            // Отрицательный stress_out в сочетании с conv кодирует прогиб желоба для изостазии
                            *stress_out = -conv_mag - 0.5;
                        } else {
                            // 3. НАВИСАЮЩАЯ ВЕРХНЯЯ ПЛИТА НАД СУБДУКЦИЕЙ (Анды или Островная дуга):
                            // Магматизм и аккреция утолщают кору и опресняют плотность (рост гранитоидов/андезитов)
                            *thick_out = (*thick_out
                                + config.convergence_thickening_rate * 0.95 * conv_mag * dt)
                                .min(config.max_crust_thickness_km * 0.88);
                            *dens_out = (*dens_out - 95.0 * conv_mag * dt)
                                .max(config.cont_crust_density);
                            *orog_out = (*orog_out + 0.80 * conv_mag * dt).min(1.0);
                        }
                    } else if shear > 0.15 {
                        // Трансформный разлом: умеренная складчатость вдоль сдвига
                        *orog_out = (*orog_out + 0.25 * shear * dt).min(0.55);
                    }
                }

                // 3. Прожигание и подъем коры над мантийными плюмами
                let r_i = topology.cells[i].center;
                for plume in plumes {
                    let d = r_i.dot(plume.position).clamp(-1.0, 1.0).acos();
                    if d < plume.radius * 1.6 {
                        let f = smoothstep(1.0 - d / (plume.radius * 1.6));
                        // Плюм наращивает базальтовую/магматическую подушку коры
                        *thick_out = (*thick_out + 8.5 * f * plume.strength * dt)
                            .min(config.max_crust_thickness_km * 0.75);
                        *orog_out = (*orog_out + 0.35 * f * dt).min(0.85);
                    }
                }
            },
        );

    // 4. Генерация новых вулканов в точках прохождения плит над стационарными плюмами
    spawn_hotspot_volcanoes_on_plates(topology, geology, config);
}

fn spawn_hotspot_volcanoes_on_plates(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    for plume in &geology.plumes {
        // Проверяем, как далеко уехал последний вулкан от точки плюма
        let min_dist_to_existing = geology
            .volcanoes
            .iter()
            .map(|v| v.center.dot(plume.position).clamp(-1.0, 1.0).acos())
            .fold(f32::MAX, f32::min);

        if min_dist_to_existing >= config.hotspot_spacing_rad {
            // Находим клетку и плиту прямо над плюмом
            let mut best_cell = 0;
            let mut best_dot = -1.0_f32;
            for (cid, cell) in topology.cells.iter().enumerate() {
                let d = cell.center.dot(plume.position);
                if d > best_dot {
                    best_dot = d;
                    best_cell = cid;
                }
            }
            let plate_id = geology.plate_ids[best_cell];
            let radius = (config.hotspot_radius_base
                + plume.strength * config.volcano_radius_per_height)
                .max(config.volcano_radius_min);

            geology.volcanoes.push(Volcano {
                center: plume.position,
                height: plume.strength,
                radius,
                plate_id,
                age: 0.0,
            });
        }
    }
}