// src/domains/tectonics/solver/advection.rs:

use super::{
    TectonicsGeometryCache, TectonicsScratch, boundaries::fast_domain_warp,
    init::find_closest_plates_and_boundary_dists,
};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use bevy::math::{Quat, Vec3};
use rayon::prelude::*;
use std::f32::consts::PI;

pub fn advect_crust_and_plates(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_plates = geology.plates.len();
    if num_plates == 0 {
        return;
    }

    // 1. Мягкая защита от слияния двух центров Вороного в одну точку:
    //    применяется только на малом радиусе `min_sep` (соответствующем `plate_min_area_ratio`),
    //    чтобы не блокировать поглощение малых плит, и напрямую учитывается в `omega`.
    if num_plates >= 2 && dt > 1e-6 {
        let min_sep = (4.0 * PI * config.plate_min_area_ratio.max(0.004)).sqrt() * 0.85;
        let mut omega_corr = vec![Vec3::ZERO; num_plates];

        for i in 0..num_plates {
            for j in (i + 1)..num_plates {
                let ci = geology.plates[i].center_dir;
                let cj = geology.plates[j].center_dir;
                let dot = ci.dot(cj).clamp(-1.0, 1.0);
                let dist = dot.acos();
                if dist > 1e-4 && dist < min_sep {
                    let push_vel = ((min_sep - dist) * 0.35) / dt;
                    let axis_ij = ci.cross(cj).normalize_or_zero();
                    omega_corr[i] -= axis_ij * push_vel;
                    omega_corr[j] += axis_ij * push_vel;
                }
            }
        }

        for (i, plate) in geology.plates.iter_mut().enumerate() {
            if omega_corr[i] != Vec3::ZERO {
                let adjusted = plate.omega + omega_corr[i];
                let spd = adjusted.length().clamp(0.05, config.angular_vel_max);
                if adjusted.length() > 1e-5 {
                    plate.euler_pole = adjusted.normalize();
                    plate.angular_vel = spd;
                    plate.omega = plate.euler_pole * spd;
                }
            }
        }
    }

    // 2. Единый кватернион шага `step_rots[p_id]` строго из физического вращения `plate.omega * dt`
    let mut step_rots = vec![Quat::IDENTITY; num_plates];
    let mut inv_step_rots = vec![Quat::IDENTITY; num_plates];
    let should_renorm = geology.sim_step.is_multiple_of(32);

    for (i, plate) in geology.plates.iter_mut().enumerate() {
        let total_rot = Quat::from_axis_angle(plate.euler_pole, plate.angular_vel * dt);
        step_rots[i] = total_rot;
        inv_step_rots[i] = total_rot.inverse();

        plate.center_dir = total_rot * plate.center_dir;
        plate.craton_center = total_rot * plate.craton_center;
        plate.accum_rot = total_rot * plate.accum_rot;

        if should_renorm {
            plate.center_dir = plate.center_dir.normalize_or_zero();
            plate.craton_center = plate.craton_center.normalize_or_zero();
            plate.accum_rot = plate.accum_rot.normalize();
        }
    }

    // 3. Вращаем вулканы тем же `step_rots[v.plate_id]` и применяем экспоненциальное затухание
    let decay = (-config.volcano_erosion_rate * dt * 0.85).exp();
    let max_volc_radius = (config.hotspot_radius_base * 2.8).max(0.14);

    for v in &mut geology.volcanoes {
        if let Some(&rot) = step_rots.get(v.plate_id) {
            v.center = rot * v.center;
            if should_renorm {
                v.center = v.center.normalize_or_zero();
            }
        }
        v.age += dt;

        let old_h = v.height;
        v.height = old_h * decay;
        let lost_h = (old_h - v.height).max(0.0);
        v.radius = (v.radius + lost_h * 0.28 + config.hotspot_radius_growth * dt * 0.20)
            .min(max_volc_radius);
    }
    let min_volcano_h = config.hotspot_height_min * 0.14;
    geology.volcanoes.retain(|v| v.height > min_volcano_h);

    // 4. Полулагранжева адвекция `orogeny_root` и `sediment_thickness` строго внутри своей плиты `p_id`
    {
        let plate_ids = &geology.plate_ids;
        let orogeny = &geology.orogeny_root;
        let sediment = &geology.sediment_thickness;

        let next_orogeny = &mut scratch.next_orogeny;
        let next_sediment = &mut scratch.next_sediment;

        next_orogeny
            .par_iter_mut()
            .zip(next_sediment.par_iter_mut())
            .enumerate()
            .for_each(|(i, (orog_out, sed_out))| {
                let p_id = plate_ids[i];
                let r_i = topology.cells[i].center;
                let r_dep = inv_step_rots[p_id] * r_i;

                let mut curr = i;
                let mut best_dot = r_i.dot(r_dep);
                for _ in 0..4 {
                    let start = cache.edge_offsets[curr];
                    let end = cache.edge_offsets[curr + 1];
                    let mut next_cell = curr;
                    for e in start..end {
                        let nbr = cache.edge_neighbors[e];
                        // Не перешагиваем через границы на чужую плиту
                        if plate_ids[nbr] != p_id {
                            continue;
                        }
                        let d = topology.cells[nbr].center.dot(r_dep);
                        if d > best_dot {
                            best_dot = d;
                            next_cell = nbr;
                        }
                    }
                    if next_cell == curr {
                        break;
                    }
                    curr = next_cell;
                }

                let r_curr = topology.cells[curr].center;
                let residual = r_dep - r_curr * r_curr.dot(r_dep);
                let res_len = residual.length();

                if res_len > 1e-5 {
                    let res_dir = residual / res_len;
                    let start = cache.edge_offsets[curr];
                    let end = cache.edge_offsets[curr + 1];

                    let mut w_sum = 0.0_f32;
                    let mut nbr_orog = 0.0_f32;
                    let mut nbr_sed = 0.0_f32;
                    let mut best_align = 0.0_f32;
                    let mut best_step_rad = cache.avg_cell_rad;

                    for e in start..end {
                        let nbr = cache.edge_neighbors[e];
                        if plate_ids[nbr] != p_id {
                            continue;
                        }
                        let align = cache.edge_tangent_dirs[e].dot(res_dir);
                        if align > 0.0 {
                            let w = align * align;
                            nbr_orog += orogeny[nbr] * w;
                            nbr_sed += sediment[nbr] * w;
                            w_sum += w;

                            if align > best_align {
                                best_align = align;
                                best_step_rad = cache.edge_dists_rad[e];
                            }
                        }
                    }

                    if w_sum > 1e-5 {
                        let frac = (res_len / best_step_rad.max(1e-5)).clamp(0.0, 1.0);
                        *orog_out = orogeny[curr] * (1.0 - frac) + (nbr_orog / w_sum) * frac;
                        *sed_out = sediment[curr] * (1.0 - frac) + (nbr_sed / w_sum) * frac;
                        return;
                    }
                }

                *orog_out = orogeny[curr];
                *sed_out = sediment[curr];
            });
    }

    geology.orogeny_root.copy_from_slice(&scratch.next_orogeny);
    geology
        .sediment_thickness
        .copy_from_slice(&scratch.next_sediment);

    // 5. Цикл Уилсона: поглощение сжатых малых плит и раскол сверхкрупных плит
    recycle_and_split_voronoi_plates(topology, geology, config);
}

fn recycle_and_split_voronoi_plates(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let num_plates = geology.plates.len();
    if num_plates < 4 || num_cells == 0 {
        return;
    }

    let min_cells = (num_cells as f32 * config.plate_min_area_ratio) as usize;
    let split_cells = (num_cells as f32 * config.plate_split_area_ratio) as usize;
    if min_cells == 0 {
        return;
    }

    let mut counts = vec![0usize; num_plates];
    for &p_id in &geology.plate_ids {
        if p_id < num_plates {
            counts[p_id] += 1;
        }
    }

    let mut dying_plate = None;
    let mut min_found = usize::MAX;
    let mut largest_plate = 0;
    let mut max_count = 0;

    for (p_id, &c) in counts.iter().enumerate() {
        if c > max_count {
            max_count = c;
            largest_plate = p_id;
        }
        if c < min_cells && c < min_found {
            min_found = c;
            dying_plate = Some(p_id);
        }
    }

    let Some(dead_id) = dying_plate else {
        return;
    };
    if max_count < split_cells || largest_plate == dead_id {
        return;
    }

    // 1. Аккреция террейна: если у поглощаемой плиты `dead_id` был остаток континентального кратона,
    //    припаиваем его площадь к ближайшей соседней плите, чтобы континент не исчезал бесследно.
    let dead_craton_r = geology.plates[dead_id].craton_radius;
    if dead_craton_r > 0.02 {
        let dead_dir = geology.plates[dead_id].center_dir;
        if let Some((host_id, _)) = geology
            .plates
            .iter()
            .enumerate()
            .filter(|&(idx, _)| idx != dead_id)
            .max_by(|(_, a), (_, b)| {
                a.center_dir
                    .dot(dead_dir)
                    .total_cmp(&b.center_dir.dot(dead_dir))
            })
        {
            let host = &mut geology.plates[host_id];
            let combined_r = (host.craton_radius.powi(2) + dead_craton_r.powi(2))
                .sqrt()
                .min(0.65);
            host.craton_radius = combined_r;
        }
    }

    // 2. Определяем направление рифта по ближайшему к `largest_plate` мантийному плюму
    let parent = geology.plates[largest_plate];
    let split_dir = geology
        .plumes
        .iter()
        .map(|pl| pl.position)
        .filter(|&pos| pos.dot(parent.center_dir) > 0.25)
        .min_by(|a, b| {
            let da = a.dot(parent.center_dir).clamp(-1.0, 1.0).acos();
            let db = b.dot(parent.center_dir).clamp(-1.0, 1.0).acos();
            da.total_cmp(&db)
        })
        .unwrap_or_else(|| {
            parent
                .euler_pole
                .cross(parent.center_dir)
                .normalize_or_zero()
        });

    let tangent =
        (split_dir - parent.center_dir * parent.center_dir.dot(split_dir)).normalize_or_zero();
    if tangent == Vec3::ZERO {
        return;
    }

    // 3. Разводим центры Вороного и раскалываем кратон на два отдельных фрагмента по разные стороны рифта
    let sep_angle = 0.14_f32;
    let new_parent_center =
        (parent.center_dir * sep_angle.cos() - tangent * sep_angle.sin()).normalize_or_zero();
    let new_child_center =
        (parent.center_dir * sep_angle.cos() + tangent * sep_angle.sin()).normalize_or_zero();

    let (parent_craton_center, parent_craton_rad, child_craton_center, child_craton_rad) = if parent
        .craton_radius
        > 0.04
    {
        let craton_tang = (tangent - parent.craton_center * parent.craton_center.dot(tangent))
            .normalize_or_zero();
        let craton_sep = (parent.craton_radius * 0.48).clamp(0.05, 0.28);
        let c_parent = (parent.craton_center * craton_sep.cos() - craton_tang * craton_sep.sin())
            .normalize_or_zero();
        let c_child = (parent.craton_center * craton_sep.cos() + craton_tang * craton_sep.sin())
            .normalize_or_zero();
        // Сумма площадей фрагментов (0.74^2 + 0.67^2 ≈ 1.0) сохраняет исходную площадь континента
        (
            c_parent,
            parent.craton_radius * 0.74,
            c_child,
            parent.craton_radius * 0.67,
        )
    } else {
        (new_parent_center, 0.0, new_child_center, 0.0)
    };

    let rot_parent = Quat::from_rotation_arc(parent.center_dir, new_parent_center);
    let rot_child = Quat::from_rotation_arc(parent.center_dir, new_child_center);

    // Задаём расходящиеся импульсы скорости обеим половинам расколовшейся плиты
    let kick_pole = parent.center_dir.cross(tangent).normalize_or_zero();
    let kick_mag = config.angular_vel_min * 0.45;

    let parent_omega = parent.omega - kick_pole * kick_mag;
    let parent_speed = parent_omega
        .length()
        .clamp(config.angular_vel_min, config.angular_vel_max);
    let parent_pole = if parent_omega.length() > 1e-4 {
        parent_omega.normalize()
    } else {
        parent.euler_pole
    };

    geology.plates[largest_plate].center_dir = new_parent_center;
    geology.plates[largest_plate].euler_pole = parent_pole;
    geology.plates[largest_plate].angular_vel = parent_speed;
    geology.plates[largest_plate].omega = parent_pole * parent_speed;
    geology.plates[largest_plate].accum_rot = rot_parent * parent.accum_rot;
    geology.plates[largest_plate].craton_center = parent_craton_center;
    geology.plates[largest_plate].craton_radius = parent_craton_rad;

    let child_omega = parent.omega + kick_pole * kick_mag;
    let child_speed = child_omega
        .length()
        .clamp(config.angular_vel_min, config.angular_vel_max);
    let child_pole = if child_omega.length() > 1e-4 {
        child_omega.normalize()
    } else {
        parent.euler_pole
    };

    let old_hue = geology.plates[dead_id].color_hue;
    geology.plates[dead_id] = super::super::tectonics_data::Plate {
        center_dir: new_child_center,
        euler_pole: child_pole,
        angular_vel: child_speed,
        omega: child_pole * child_speed,
        accum_rot: rot_child * parent.accum_rot,
        base_elevation_offset: parent.base_elevation_offset,
        craton_center: child_craton_center,
        craton_radius: child_craton_rad,
        seed_offset: parent.seed_offset + 17.3,
        color_hue: old_hue,
    };

    // 4. Перепривязываем вулканы к актуальным плитам, чтобы вулканы старой `dead_id`
    //    и расколовшейся `largest_plate` вращались со своей физической плитой
    let warp_scale = config.warp_scale as f32;
    let warp_strength = config.warp_strength;
    for v in &mut geology.volcanoes {
        if v.plate_id == dead_id || v.plate_id == largest_plate {
            let warped_v = fast_domain_warp(v.center, warp_scale, warp_strength);
            let (new_p_id, _, _, _, _) =
                find_closest_plates_and_boundary_dists(warped_v, &geology.plates);
            v.plate_id = new_p_id;
        }
    }
}
