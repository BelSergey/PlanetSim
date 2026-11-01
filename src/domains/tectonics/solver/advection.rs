// src/domains/tectonics/solver/advection.rs

use super::{TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use bevy::math::{Quat, Vec3};
use rayon::prelude::*;

pub fn advect_crust_and_plates(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    // 1. Вращаем центры плит, кратонов и существующие вулканы вместе с их плитами
    for plate in &mut geology.plates {
        let angle = plate.angular_vel * dt;
        let rot = Quat::from_axis_angle(plate.euler_pole, angle);
        plate.center_dir = (rot * plate.center_dir).normalize_or_zero();
        plate.craton_center = (rot * plate.craton_center).normalize_or_zero();
    }

    for v in &mut geology.volcanoes {
        if let Some(plate) = geology.plates.get(v.plate_id) {
            let angle = plate.angular_vel * dt;
            let rot = Quat::from_axis_angle(plate.euler_pole, angle);
            v.center = (rot * v.center).normalize_or_zero();
        }
        v.age += dt;
        // Постепенная эрозия высоты и расползание старых вулканов цепи
        v.height *= (1.0 - config.volcano_erosion_rate * dt).max(0.85);
        v.radius = (v.radius + config.hotspot_radius_growth * dt * 0.25).min(0.14);
    }
    // Удаляем совсем стершиеся древние подводные гайоты
    geology.volcanoes.retain(|v| v.height > 0.04);

    // 2. Полулагранжева адвекция коры с субклеточным аккумулятором (без размытия берегов)
    {
        let plates = &geology.plates;
        let plate_ids = &geology.plate_ids;
        let thickness = &geology.crust_thickness;
        let density = &geology.crust_density;
        let age = &geology.crust_age;
        let sediment = &geology.sediment_thickness;
        let orogeny = &geology.orogeny_root;
        let shifts = &geology.subcell_shift;

        let next_plate_ids = &mut scratch.next_plate_ids;
        let next_thickness = &mut scratch.next_thickness;
        let next_density = &mut scratch.next_density;
        let next_age = &mut scratch.next_age;
        let next_sediment = &mut scratch.next_sediment;
        let next_orogeny = &mut scratch.next_orogeny;
        let next_shift = &mut scratch.next_shift;

        next_plate_ids
            .par_iter_mut()
            .zip(next_thickness.par_iter_mut())
            .zip(next_density.par_iter_mut())
            .zip(next_age.par_iter_mut())
            .zip(next_sediment.par_iter_mut())
            .zip(next_orogeny.par_iter_mut())
            .zip(next_shift.par_iter_mut())
            .enumerate()
            .for_each(
                |(
                    i,
                    (
                        (
                            ((((p_out, th_out), den_out), age_out), sed_out),
                            orog_out,
                        ),
                        sh_out,
                    ),
                )| {
                    let p_id = plate_ids[i];
                    let r_i = topology.cells[i].center;
                    let v_i = plates[p_id].omega.cross(r_i);

                    // Накапливаем угловой сдвиг в касательной плоскости клетки
                    let prev_shift = Vec3::from_array(shifts[i]);
                    let raw_shift = prev_shift + v_i * dt;
                    // Ортогонализуем к r_i
                    let accum_shift = raw_shift - r_i * raw_shift.dot(r_i);
                    let shift_len = accum_shift.length();

                    // Ищем соседа в направлении ОТКУДА движется кора (-accum_shift)
                    let upwind_dir = if shift_len > 1e-6 {
                        -accum_shift / shift_len
                    } else {
                        Vec3::ZERO
                    };

                    let start = cache.edge_offsets[i];
                    let end = cache.edge_offsets[i + 1];

                    let mut best_nbr = i;
                    let mut best_align = 0.25_f32; // Порог сонаправленности
                    let mut best_dist = cache.avg_cell_rad;

                    for e in start..end {
                        let align = cache.edge_tangent_dirs[e].dot(upwind_dir);
                        if align > best_align {
                            best_align = align;
                            best_nbr = cache.edge_neighbors[e];
                            best_dist = cache.edge_dists_rad[e];
                        }
                    }

                    // Если накопленный сдвиг превысил шаг до соседа, делаем перенос (Donor-cell)
                    if best_nbr != i && shift_len >= best_dist * 0.52 {
                        let nbr_p = plate_ids[best_nbr];
                        let nbr_v = plates[nbr_p].omega.cross(topology.cells[best_nbr].center);
                        let toward_i = -cache.edge_tangent_dirs[(start..end)
                            .find(|&e| cache.edge_neighbors[e] == best_nbr)
                            .unwrap_or(start)];

                        // Проверяем, действительно ли соседняя клетка наплывает на клетку i
                        if nbr_v.dot(toward_i) > 0.0 {
                            let my_cont = density[i] < 2825.0 && thickness[i] > 20.0;
                            let nbr_cont =
                                density[best_nbr] < 2825.0 && thickness[best_nbr] > 20.0;

                            // Легкий континент не может быть «затоптан» наплывающей тонкой океанической корой
                            // (океаническая кора уходит под него в субдукцию!)
                            if my_cont && !nbr_cont && nbr_p != p_id {
                                *p_out = p_id;
                                *th_out = thickness[i];
                                *den_out = density[i];
                                *age_out = age[i];
                                *sed_out = sediment[i];
                                *orog_out = orogeny[i];
                                *sh_out = (accum_shift * 0.35).to_array();
                                return;
                            }

                            // Если внутри одной плиты и одного типа коры — плавно смешиваем,
                            // а если идет граница континента или плиты — переносим дискретно без размытия!
                            let blend = if nbr_p == p_id && my_cont == nbr_cont {
                                0.65
                            } else {
                                1.0
                            };

                            *p_out = nbr_p;
                            *th_out = thickness[i] * (1.0 - blend) + thickness[best_nbr] * blend;
                            *den_out = density[i] * (1.0 - blend) + density[best_nbr] * blend;
                            *age_out = age[i] * (1.0 - blend) + age[best_nbr] * blend;
                            *sed_out = sediment[i] * (1.0 - blend) + sediment[best_nbr] * blend;
                            *orog_out = orogeny[i] * (1.0 - blend) + orogeny[best_nbr] * blend;

                            let rem_shift =
                                accum_shift - accum_shift.normalize_or_zero() * best_dist;
                            *sh_out = rem_shift.to_array();
                            return;
                        }
                    }

                    // Иначе кора еще копит субклеточный сдвиг внутри текущего гекса
                    *p_out = p_id;
                    *th_out = thickness[i];
                    *den_out = density[i];
                    *age_out = age[i];
                    *sed_out = sediment[i];
                    *orog_out = orogeny[i];
                    *sh_out = accum_shift.to_array();
                },
            );
    }

    // Копируем буферы обратно
    geology.plate_ids.copy_from_slice(&scratch.next_plate_ids);
    geology
        .crust_thickness
        .copy_from_slice(&scratch.next_thickness);
    geology.crust_density.copy_from_slice(&scratch.next_density);
    geology.crust_age.copy_from_slice(&scratch.next_age);
    geology
        .sediment_thickness
        .copy_from_slice(&scratch.next_sediment);
    geology.orogeny_root.copy_from_slice(&scratch.next_orogeny);
    geology.subcell_shift.copy_from_slice(&scratch.next_shift);

    // 3. Баланс поглощения и раскола плит (равный шанс поглощения и рифтового раскола)
    balance_plate_absorption_and_splitting(topology, geology, config);
}

fn balance_plate_absorption_and_splitting(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let num_plates = geology.plates.len();
    if num_plates < 3 {
        return;
    }

    let mut counts = vec![0usize; num_plates];
    let mut centroids = vec![Vec3::ZERO; num_plates];

    for (i, &p_id) in geology.plate_ids.iter().enumerate() {
        if p_id < num_plates {
            counts[p_id] += 1;
            centroids[p_id] += topology.cells[i].center;
        }
    }

    // Актуализируем геометрические центры плит по их реальным клеткам
    for p_id in 0..num_plates {
        if counts[p_id] > 0 {
            let norm = centroids[p_id].normalize_or_zero();
            if norm != Vec3::ZERO {
                geology.plates[p_id].center_dir = norm;
            }
        }
    }

    let min_cells = (num_cells as f32 * config.plate_min_area_ratio) as usize;
    let split_cells = (num_cells as f32 * config.plate_split_area_ratio) as usize;

    // Ищем умирающую (субдуцировавшую) плиту и самую огромную плиту для раскола
    let mut dying_plate = None;
    let mut largest_plate = 0;
    let mut max_count = 0;

    for (p_id, &c) in counts.iter().enumerate() {
        if c > max_count {
            max_count = c;
            largest_plate = p_id;
        }
        if c < min_cells {
            dying_plate = Some(p_id);
        }
    }

    let Some(dead_id) = dying_plate else {
        return;
    };
    if max_count < split_cells || largest_plate == dead_id {
        return;
    }

    // 1. Поглощаем остатки умирающей плиты соседними плитами
    for i in 0..num_cells {
        if geology.plate_ids[i] == dead_id {
            let replacement = topology.cells[i]
                .neighbors
                .iter()
                .map(|&n| geology.plate_ids[n])
                .find(|&np| np != dead_id)
                .unwrap_or(largest_plate);
            geology.plate_ids[i] = replacement;
        }
    }

    // 2. Раскалываем самую крупную плиту `largest_plate` на две по линии разлома (предпочтительно через плюм)
    let parent = geology.plates[largest_plate];
    let split_origin = geology
        .plumes
        .iter()
        .map(|pl| pl.position)
        .min_by(|a, b| {
            let da = a.dot(parent.center_dir).clamp(-1.0, 1.0).acos();
            let db = b.dot(parent.center_dir).clamp(-1.0, 1.0).acos();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(parent.center_dir);

    let split_normal = parent
        .euler_pole
        .cross(split_origin)
        .normalize_or_zero();
    let split_normal = if split_normal == Vec3::ZERO {
        Vec3::X
    } else {
        split_normal
    };

    for i in 0..num_cells {
        if geology.plate_ids[i] == largest_plate {
            let r = topology.cells[i].center;
            // Волнистая линия рифтового раскола
            let ripple = (r.x * 9.0 + r.y * 7.0).sin() * 0.04;
            if (r - split_origin).dot(split_normal) + ripple > 0.0 {
                geology.plate_ids[i] = dead_id;
            }
        }
    }

    // Задаем новой отколовшейся плите `dead_id` и материнской `largest_plate` расходящиеся оси вращения
    let kick = split_origin.cross(split_normal).normalize_or_zero() * (config.angular_vel_min * 0.65);
    let new_omega_a = parent.omega + kick;
    let new_omega_b = parent.omega - kick;

    if new_omega_a.length() > 1e-4 {
        geology.plates[largest_plate].euler_pole = new_omega_a.normalize();
        geology.plates[largest_plate].angular_vel = new_omega_a
            .length()
            .clamp(config.angular_vel_min, config.angular_vel_max);
        geology.plates[largest_plate].omega =
            geology.plates[largest_plate].euler_pole * geology.plates[largest_plate].angular_vel;
    }

    if new_omega_b.length() > 1e-4 {
        geology.plates[dead_id].euler_pole = new_omega_b.normalize();
        geology.plates[dead_id].angular_vel = new_omega_b
            .length()
            .clamp(config.angular_vel_min, config.angular_vel_max);
        geology.plates[dead_id].omega =
            geology.plates[dead_id].euler_pole * geology.plates[dead_id].angular_vel;
    }
}