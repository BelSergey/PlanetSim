// src/domains/tectonics/solver/advection.rs:

use super::{
    TectonicsGeometryCache, TectonicsScratch, boundaries::fast_domain_warp,
    init::find_closest_plates_and_boundary_dists,
};

use super::super::tectonics_data::{CratonLobe, Plate, MAX_PLATE_CRATON_LOBES};

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

    // 1–2. Вычисляем единый кватернион шага `step_rots[p_id]` из `plate.omega * dt`
    //      плюс мягкая поправка от слияния центров Вороного (без перманентной порчи plate.omega)
    let mut omega_corr = vec![Vec3::ZERO; num_plates];
    if num_plates >= 2 && dt > 1e-6 {
        let min_sep = (4.0 * PI * config.plate_min_area_ratio.max(0.004)).sqrt() * 0.85;
        for i in 0..num_plates {
            for j in (i + 1)..num_plates {
                let ci = geology.plates[i].center_dir;
                let cj = geology.plates[j].center_dir;
                let dot = ci.dot(cj).clamp(-1.0, 1.0);
                let dist = dot.acos();
                if dist > 1e-4 && dist < min_sep {
                    let push_vel = ((min_sep - dist) * 0.20) / dt;
                    let axis_ij = ci.cross(cj).normalize_or_zero();
                    omega_corr[i] -= axis_ij * push_vel;
                    omega_corr[j] += axis_ij * push_vel;
                }
            }
        }
    }

    let mut step_rots = vec![Quat::IDENTITY; num_plates];
    let mut inv_step_rots = vec![Quat::IDENTITY; num_plates];
    let should_renorm = geology.sim_step.is_multiple_of(32);

    for (i, plate) in geology.plates.iter_mut().enumerate() {
        let eff_omega = plate.omega + omega_corr[i];
        let eff_speed = eff_omega.length().clamp(0.0, config.angular_vel_max * 1.25);
        let total_rot = if eff_speed > 1e-5 {
            Quat::from_axis_angle(eff_omega / eff_omega.length(), eff_speed * dt)
        } else {
            Quat::IDENTITY
        };

        step_rots[i] = total_rot;
        inv_step_rots[i] = total_rot.inverse();

        plate.center_dir   = total_rot * plate.center_dir;
        plate.craton_center = total_rot * plate.craton_center;
        plate.accum_rot     = total_rot * plate.accum_rot;

        // NEW: лопасти вращаются той же ротацией
        for k in 0..plate.lobe_count as usize {
            plate.lobes[k].center = total_rot * plate.lobes[k].center;
        }

        if should_renorm {
            plate.center_dir    = plate.center_dir.normalize_or_zero();
            plate.craton_center = plate.craton_center.normalize_or_zero();
            plate.accum_rot     = plate.accum_rot.normalize();
            for k in 0..plate.lobe_count as usize {
                plate.lobes[k].center = plate.lobes[k].center.normalize_or_zero();
            }
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
    let dead_plate = geology.plates[dead_id];
    if dead_plate.craton_radius > 0.02 {
        let dead_dir = dead_plate.center_dir;
        if let Some(host_id) = geology
            .plates
            .iter()
            .enumerate()
            .filter(|&(idx, _)| idx != dead_id)
            .max_by(|(_, a), (_, b)| {
                a.center_dir.dot(dead_dir).total_cmp(&b.center_dir.dot(dead_dir))
            })
            .map(|(idx, _)| idx)
        {
            let host = &mut geology.plates[host_id];

            // Переносим активные лопасти, пока есть место
            for k in 0..dead_plate.lobe_count as usize {
                if (host.lobe_count as usize) >= MAX_PLATE_CRATON_LOBES { break; }
                let lobe = dead_plate.lobes[k];
                if lobe.radius > 1e-6 {
                    host.lobes[host.lobe_count as usize] = lobe;
                    host.lobe_count += 1;
                }
            }
            // Старые сейвы: craton_radius есть, а лопастей нет — синтезируем одну
            if dead_plate.lobe_count == 0
                && (host.lobe_count as usize) < MAX_PLATE_CRATON_LOBES
            {
                host.lobes[host.lobe_count as usize] = CratonLobe {
                    center: dead_plate.craton_center,
                    radius: dead_plate.craton_radius,
                    seed_offset: dead_plate.seed_offset,
                };
                host.lobe_count += 1;
            }

            host.recompute_craton_from_lobes();
            host.craton_radius = host.craton_radius.min(0.65); // сохраняем прежний кап
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

    let (parent_lobes, parent_lobe_count, child_lobes, child_lobe_count) =
    split_craton_lobes(&parent, tangent);

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

    {
        let p = &mut geology.plates[largest_plate];
        p.center_dir  = new_parent_center;
        p.euler_pole  = parent_pole;
        p.angular_vel = parent_speed;
        p.omega       = parent_pole * parent_speed;
        p.accum_rot   = rot_parent * parent.accum_rot;

        p.lobes      = parent_lobes;
        p.lobe_count = parent_lobe_count;
        // Лопасти были в старой системе `parent.center_dir` — доворачиваем
        for k in 0..p.lobe_count as usize {
            p.lobes[k].center = rot_parent * p.lobes[k].center;
        }
        p.recompute_craton_from_lobes();

        // Раскол сбрасывает часть накоплений
        p.tension_axis = rot_parent * parent.tension_axis;
        p.internal_tension *= 0.5;
        p.mantle_heat      *= 0.6;
        p.is_alive          = true;
    }

    let child_omega = parent.omega + kick_pole * kick_mag;
    let child_speed = child_omega
        .length()
        .clamp(config.angular_vel_min, config.angular_vel_max);
    let child_pole = if child_omega.length() > 1e-4 {
        child_omega.normalize()
    } else {
        parent.euler_pole
    };

    let old_hue = geology.plates[dead_id].color_hue; // читаем ДО перезаписи слота

    let mut child = Plate {
        center_dir:  new_child_center,
        euler_pole:  child_pole,
        angular_vel: child_speed,
        omega:       child_pole * child_speed,
        accum_rot:   rot_child * parent.accum_rot,
        base_elevation_offset: parent.base_elevation_offset,

        // Эти два поля перезапишет recompute_craton_from_lobes — временные значения
        craton_center: new_child_center,
        craton_radius: 0.0,

        lobes:      child_lobes,
        lobe_count: child_lobe_count,

        seed_offset: parent.seed_offset + 17.3,
        color_hue:   old_hue,

        tension_axis: rot_child * parent.tension_axis,
        internal_tension: 0.0,
        mantle_heat:      0.0,
        is_alive:         true,
    };

    for k in 0..child.lobe_count as usize {
        child.lobes[k].center = rot_child * child.lobes[k].center;
        child.lobes[k].seed_offset += 11.9; // немного разные шумовые паттерны у фрагментов
    }
    child.recompute_craton_from_lobes();

    geology.plates[dead_id] = child;

    let n = geology.plates.len();
    for other in 0..n {
        if other == dead_id { continue; }
        let row = dead_id * n + other;
        let col = other * n + dead_id;
        if row < geology.subduction_polarity.len() { geology.subduction_polarity[row] = 0; }
        if col < geology.subduction_polarity.len() { geology.subduction_polarity[col] = 0; }
    }

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

fn split_craton_lobes(
    parent: &Plate,
    tangent: Vec3,
) -> (
    [CratonLobe; MAX_PLATE_CRATON_LOBES], u8,
    [CratonLobe; MAX_PLATE_CRATON_LOBES], u8,
) {
    let mut p = [CratonLobe::default(); MAX_PLATE_CRATON_LOBES];
    let mut pc = 0u8;
    let mut c = [CratonLobe::default(); MAX_PLATE_CRATON_LOBES];
    let mut cc = 0u8;

    // Старые сейвы: craton_radius задан, а лопастей нет — синтезируем одну «виртуальную»
    let mut src = [CratonLobe::default(); MAX_PLATE_CRATON_LOBES];
    let src_n: usize = if parent.lobe_count > 0 {
        let n = (parent.lobe_count as usize).min(MAX_PLATE_CRATON_LOBES);
        src[..n].copy_from_slice(&parent.lobes[..n]);
        n
    } else if parent.craton_radius > 0.04 {
        src[0] = CratonLobe {
            center: parent.craton_center,
            radius: parent.craton_radius,
            seed_offset: parent.seed_offset,
        };
        1
    } else {
        0
    };

    // Проход 1: каждая лопасть уходит на ту сторону рифта, куда смотрит её центр
    for k in 0..src_n {
        let lobe = src[k];
        if lobe.radius <= 1e-6 { continue; }
        if lobe.center.dot(tangent) >= 0.0 {
            if (cc as usize) < MAX_PLATE_CRATON_LOBES { c[cc as usize] = lobe; cc += 1; }
        } else if (pc as usize) < MAX_PLATE_CRATON_LOBES { p[pc as usize] = lobe; pc += 1; }
    }

    // Проход 2: если одна из сторон пустая — раскалываем крупнейшую лопасть
    // (сохранение площади: 2·(r/√2)² = r²)
    if src_n > 0 && (pc == 0) ^ (cc == 0) {
        let (big, big_idx, from_parent) = if pc > 0 {
            let i = (0..pc as usize)
                .max_by(|&a, &b| p[a].radius.total_cmp(&p[b].radius)).unwrap();
            (p[i], i, true)
        } else {
            let i = (0..cc as usize)
                .max_by(|&a, &b| c[a].radius.total_cmp(&c[b].radius)).unwrap();
            (c[i], i, false)
        };
        let ct = (tangent - big.center * big.center.dot(tangent)).normalize_or_zero();
        if ct != Vec3::ZERO {
            let sep = (big.radius * 0.48).clamp(0.05, 0.28);
            let a = (big.center * sep.cos() - ct * sep.sin()).normalize_or_zero();
            let b = (big.center * sep.cos() + ct * sep.sin()).normalize_or_zero();
            let frag = big.radius * std::f32::consts::FRAC_1_SQRT_2;
            if from_parent {
                p[big_idx] = CratonLobe { center: a, radius: frag, seed_offset: big.seed_offset };
                if (cc as usize) < MAX_PLATE_CRATON_LOBES {
                    c[cc as usize] = CratonLobe { center: b, radius: frag, seed_offset: big.seed_offset + 3.7 };
                    cc += 1;
                }
            } else {
                c[big_idx] = CratonLobe { center: b, radius: frag, seed_offset: big.seed_offset };
                if (pc as usize) < MAX_PLATE_CRATON_LOBES {
                    p[pc as usize] = CratonLobe { center: a, radius: frag, seed_offset: big.seed_offset + 3.7 };
                    pc += 1;
                }
            }
        }
    }

    (p, pc, c, cc)
}