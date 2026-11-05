// src/domains/tectonics/solver/advection.rs

use super::{TectonicsGeometryCache, TectonicsScratch, init::craton_continental_factor};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{CratonLobe, GeologyLayer, MAX_PLATE_CRATON_LOBES, Plate},
    },
    world::topology::WorldTopology,
};
use bevy::math::{Quat, Vec3};
use rayon::prelude::*;

/// Находит клетку отправления/назначения вдоль геодезического сдвига к `r_target` внутри той же плиты `p_id`
/// и возвращает `(best_cell, neighbor_cell, blend_frac, min_max_bounds)`.
#[inline]
fn trace_same_plate_step(
    start_cell: usize,
    p_id: usize,
    r_target: Vec3,
    plate_ids: &[usize],
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
) -> (usize, usize, usize, f32, f32) {
    let mut curr = start_cell;
    let mut best_dot = topology.cells[curr].center.dot(r_target);

    // Шагаем только если до цели больше ~1 радиуса клетки и луч не упёрся в чужую плиту
    let cos_step = cache.avg_cell_rad.cos();
    for _ in 0..4 {
        if best_dot >= cos_step {
            break;
        }
        let start = cache.edge_offsets[curr];
        let end = cache.edge_offsets[curr + 1];
        let mut next_cell = curr;
        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            let d = topology.cells[nbr].center.dot(r_target);
            if d > best_dot {
                best_dot = d;
                next_cell = nbr;
            }
        }
        if next_cell == curr || plate_ids[next_cell] != p_id {
            break;
        }
        curr = next_cell;
    }

    let r_curr = topology.cells[curr].center;
    let residual = r_target - r_curr * r_curr.dot(r_target);
    let res_len = residual.length();

    if res_len > 1e-5 {
        let res_dir = residual / res_len;
        let start = cache.edge_offsets[curr];
        let end = cache.edge_offsets[curr + 1];

        // Ищем 2 лучших ребра, образующих 60-градусный сектор вокруг `res_dir`
        let (mut n1, mut a1, mut step1) = (curr, 0.0_f32, cache.avg_cell_rad);
        let (mut n2, mut a2, mut step2) = (curr, 0.0_f32, cache.avg_cell_rad);
        let mut max_any_align = 0.0_f32;
        let mut best_any_same_plate = true;

        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            let align = cache.edge_tangent_dirs[e].dot(res_dir);
            if align > max_any_align {
                max_any_align = align;
                best_any_same_plate = plate_ids[nbr] == p_id;
            }
            if plate_ids[nbr] != p_id {
                continue;
            }
            if align > a1 {
                n2 = n1;
                a2 = a1;
                step2 = step1;
                n1 = nbr;
                a1 = align;
                step1 = cache.edge_dists_rad[e];
            } else if align > a2 {
                n2 = nbr;
                a2 = align;
                step2 = cache.edge_dists_rad[e];
            }
        }

        // Если главное геометрическое направление упирается в чужую плиту — не скользим вбок вдоль шва
        if best_any_same_plate && n1 != curr && a1 > 0.35 {
            let w_dir1 = (a1 - 0.42).max(0.01);
            let w_dir2 = if n2 != curr { (a2 - 0.42).max(0.0) } else { 0.0 };
            let w_sum = w_dir1 + w_dir2;
            let s1 = w_dir1 / w_sum;
            let s2 = w_dir2 / w_sum;

            let eff_align = a1 * s1 + a2 * s2;
            let eff_step = (step1 * s1 + step2 * s2).max(1e-5);
            let frac = ((res_len * eff_align) / eff_step).clamp(0.0, 1.0);
            return (curr, n1, n2, frac * s1, frac * s2);
        }
    }

    (curr, curr, curr, 0.0, 0.0)
}

pub fn advect_crust_and_plates(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_plates = geology.plates.len();
    let num_cells = topology.cells.len();
    if num_plates == 0 || num_cells == 0 {
        return;
    }
    geology.ensure_aux_buffers(num_cells);

    // 1. Вычисляем кватернионы прямого и обратного поворота плит за шаг `dt`
    let mut step_rots = vec![Quat::IDENTITY; num_plates];
    let mut inv_step_rots = vec![Quat::IDENTITY; num_plates];
    let should_renorm = geology.sim_step.is_multiple_of(32);

    for (i, plate) in geology.plates.iter_mut().enumerate() {
        if !plate.is_alive {
            continue;
        }
        let eff_speed = plate.omega.length().clamp(0.0, config.angular_vel_max * 1.25);
        let total_rot = if eff_speed > 1e-5 {
            Quat::from_axis_angle(plate.omega / plate.omega.length(), eff_speed * dt)
        } else {
            Quat::IDENTITY
        };

        step_rots[i] = total_rot;
        inv_step_rots[i] = total_rot.inverse();

        plate.center_dir = total_rot * plate.center_dir;
        plate.craton_center = total_rot * plate.craton_center;
        plate.tension_axis = total_rot * plate.tension_axis;
        plate.accum_rot = total_rot * plate.accum_rot;

        let lobe_cnt = (plate.lobe_count as usize).min(MAX_PLATE_CRATON_LOBES);
        for l_idx in 0..lobe_cnt {
            plate.lobes[l_idx].center = total_rot * plate.lobes[l_idx].center;
            if should_renorm {
                plate.lobes[l_idx].center = plate.lobes[l_idx].center.normalize_or_zero();
            }
        }

        if should_renorm {
            plate.center_dir = plate.center_dir.normalize_or_zero();
            plate.craton_center = plate.craton_center.normalize_or_zero();
            plate.tension_axis = plate.tension_axis.normalize_or_zero();
            plate.accum_rot = plate.accum_rot.normalize();
        }
    }

    // 2. Вращаем вулканы вместе с их плитами и переводим потерянную от эрозии высоту в ширину щита
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

    // 3. Двухпроходная адвекция MacCormack с 2D-секторной интерполяцией
    {
        let plate_ids = &geology.plate_ids;
        let orogeny = &geology.orogeny_root;
        let sediment = &geology.sediment_thickness;
        let cont_factor = &geology.continental_factor;
        let sutures = &geology.suture_weakness;
        let orog_age = &geology.orogen_age;

        // Проход 1 (Прямой полулагранжев перенос назад по `inv_step_rots`)
        scratch
            .next_orogeny
            .par_iter_mut()
            .zip(scratch.next_sediment.par_iter_mut())
            .zip(scratch.next_density.par_iter_mut())
            .zip(scratch.next_thickness.par_iter_mut())
            .zip(scratch.next_age.par_iter_mut())
            .enumerate()
            .for_each(|(i, ((((orog_1, sed_1), cont_1), sut_1), o_age_1))| {
                let p_id = plate_ids[i];
                let r_i = topology.cells[i].center;
                let r_dep = inv_step_rots[p_id] * r_i;

                let (c0, c1, c2, w1, w2) =
                    trace_same_plate_step(i, p_id, r_dep, plate_ids, topology, cache);
                let w0 = 1.0 - w1 - w2;
                let bilerp = |arr: &[f32]| arr[c0] * w0 + arr[c1] * w1 + arr[c2] * w2;

                *orog_1 = bilerp(orogeny);
                *sed_1 = bilerp(sediment);
                *cont_1 = bilerp(cont_factor);
                *sut_1 = bilerp(sutures);
                *o_age_1 = bilerp(orog_age) + dt * 0.25;
            });

        // Проход 2 (Обратная проверка MacCormack вперёд по `step_rots` + 2D-зажим)
        let phi_1_orog = &scratch.next_orogeny;
        let phi_1_sed = &scratch.next_sediment;
        let phi_1_cont = &scratch.next_density;
        let phi_1_sut = &scratch.next_thickness;
        let phi_1_oage = &scratch.next_age;

        let corrected: Vec<(f32, f32, f32, f32, f32)> = (0..num_cells)
            .into_par_iter()
            .map(|i| {
                let p_id = plate_ids[i];
                let r_i = topology.cells[i].center;
                let r_dep = inv_step_rots[p_id] * r_i;
                let r_fwd = step_rots[p_id] * r_i;

                let (c0, c1, c2, _, _) =
                    trace_same_plate_step(i, p_id, r_dep, plate_ids, topology, cache);
                let (f0, f1, f2, fw1, fw2) =
                    trace_same_plate_step(i, p_id, r_fwd, plate_ids, topology, cache);
                let fw0 = 1.0 - fw1 - fw2;

                // Если прямой или обратный луч упёрся в границу плиты, отключаем антидиффузию MacCormack
                let mc = if c1 == c0 || f1 == f0 { 0.0 } else { 0.45 };

                let correct_field = |orig: &[f32], phi1: &[f32]| -> f32 {
                    let back = phi1[f0] * fw0 + phi1[f1] * fw1 + phi1[f2] * fw2;
                    let lo = orig[c0].min(orig[c1]).min(orig[c2]).min(orig[i]);
                    let hi = orig[c0].max(orig[c1]).max(orig[c2]).max(orig[i]);
                    (phi1[i] + mc * (orig[i] - back)).clamp(lo, hi)
                };

                let corr_orog = correct_field(orogeny, phi_1_orog);
                let corr_sed = correct_field(sediment, phi_1_sed);
                let mut corr_cont = correct_field(cont_factor, phi_1_cont);
                let lobe_c = craton_continental_factor(r_i, &geology.plates[p_id]);
                corr_cont = corr_cont.max(lobe_c * 0.92).clamp(0.0, 1.0);
                let corr_sut = correct_field(sutures, phi_1_sut);

                let corr_oage = if corr_orog < 0.05 {
                    0.0
                } else {
                    phi_1_oage[i].clamp(0.0, 12.0)
                };

                (corr_orog, corr_sed, corr_cont, corr_sut, corr_oage)
            })
            .collect();

        for (i, (o, s, c, sut, oa)) in corrected.into_iter().enumerate() {
            geology.orogeny_root[i] = o;
            geology.sediment_thickness[i] = s;
            geology.continental_factor[i] = c;
            geology.suture_weakness[i] = sut;
            geology.orogen_age[i] = oa;
        }
    }

    // 4. Клеточная миграция границ плит (поглощение океанической коры в субдукции и рост в спрединге)
    migrate_cell_plate_boundaries(topology, cache, geology, scratch, config, dt);

    // 5. Физический цикл Уилсона: естественное вымирание мелких плит, спаивание материков и раскол по швам
    evolve_wilson_cycle_physical(topology, cache, geology, config);
}

/// Проверяет, образуют ли соседи клетки `cell_i` с плитой `target_p` связный двумерный клин:
/// возвращает `(кол-во соседей с target_p, кол-во пар таких соседей, которые сами граничат друг с другом)`.
#[inline]
fn contiguous_plate_support(
    cell_i: usize,
    target_p: usize,
    plate_ids: &[usize],
    cache: &TectonicsGeometryCache,
) -> (usize, usize) {
    let start = cache.edge_offsets[cell_i];
    let end = cache.edge_offsets[cell_i + 1];

    let mut same_nbrs = [usize::MAX; 12];
    let mut same_cnt = 0usize;

    for e in start..end {
        let nbr = cache.edge_neighbors[e];
        if plate_ids[nbr] == target_p && same_cnt < 12 {
            same_nbrs[same_cnt] = nbr;
            same_cnt += 1;
        }
    }

    if same_cnt < 2 {
        return (same_cnt, 0);
    }

    // Проверяем, сколько пар из `same_nbrs` являются непосредственными соседями друг другу
    let mut mutual_pairs = 0usize;
    for a in 0..same_cnt {
        let u = same_nbrs[a];
        let u_start = cache.edge_offsets[u];
        let u_end = cache.edge_offsets[u + 1];
        for b in (a + 1)..same_cnt {
            let v = same_nbrs[b];
            for ue in u_start..u_end {
                if cache.edge_neighbors[ue] == v {
                    mutual_pairs += 1;
                    break;
                }
            }
        }
    }

    (same_cnt, mutual_pairs)
}

/// Двигает клеточные границы `plate_ids` без встречного расслоения ("зебры")
/// и убирает 1-клеточные полоски проверкой смежных пар соседей.
fn migrate_cell_plate_boundaries(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    _config: &TectonicsConfig,
    dt: f32,
) {
    let num_plates = geology.plates.len();
    scratch.next_plate_ids.copy_from_slice(&geology.plate_ids);

    // Фаза А: строго антисимметричный кинематический перенос границ (без взаимного обмена клетками!)
    {
        let plate_ids = &geology.plate_ids;
        let cont_factor = &geology.continental_factor;
        let next_pids = &mut scratch.next_plate_ids;

        next_pids.par_iter_mut().enumerate().for_each(|(i, pid_out)| {
            let my_p = plate_ids[i];
            if my_p >= num_plates || !geology.plates[my_p].is_alive {
                return;
            }

            // Континентальное ядро жёстко сцеплено со своей плитой
            if cont_factor[i] > 0.60 {
                return;
            }

            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            let mut has_diff_nbr = false;
            for e in start..end {
                if plate_ids[cache.edge_neighbors[e]] != my_p {
                    has_diff_nbr = true;
                    break;
                }
            }
            if !has_diff_nbr {
                return;
            }

            let r_i = topology.cells[i].center;
            let v_my = geology.plates[my_p].omega.cross(r_i);

            let mut best_winner = my_p;
            let mut best_invasion = 0.0_f32;

            for e in start..end {
                let nbr = cache.edge_neighbors[e];
                let nbr_p = plate_ids[nbr];
                if nbr_p == my_p || nbr_p >= num_plates || !geology.plates[nbr_p].is_alive {
                    continue;
                }

                // Чтобы граница двигалась связным фронтом, а не пускала тонкие "иголки",
                // у захватывающей плиты `nbr_p` уже должна быть связная пара смежных соседей вокруг `i`!
                let (nbr_same_cnt, nbr_mutual_pairs) =
                    contiguous_plate_support(i, nbr_p, plate_ids, cache);
                if nbr_same_cnt < 2 || nbr_mutual_pairs == 0 {
                    continue;
                }

                // Единичный вектор от соседа `nbr` к текущей клетке `i`
                let n_to_i = -cache.edge_tangent_dirs[e];
                let step_rad = cache.edge_dists_rad[e].max(1e-4);
                let v_nbr = geology.plates[nbr_p].omega.cross(r_i);

                // Скорость сближения вдоль ребра (положительна при конвергенции, отрицательна при спрединге)
                let rel_conv = (v_nbr - v_my).dot(n_to_i);

                let advance_vel = if rel_conv > 0.0 {
                    // СБЛИЖЕНИЕ: только верхняя (наезжающая) плита может продвигаться над нижней!
                    // Нижняя плита уходит в мантию и никогда не "прорастает" полосками внутрь верхней.
                    let my_subducts = geology
                        .is_plate_subducting_under(my_p, nbr_p)
                        .unwrap_or(cont_factor[i] < cont_factor[nbr]);
                    if my_subducts && cont_factor[i] < 0.45 {
                        v_nbr.dot(n_to_i).max(0.0) * 0.42
                    } else {
                        0.0
                    }
                } else {
                    // РАСХОЖДЕНИЕ / СДВИГ: ось хребта дрейфует со средней скоростью `0.5 * (v_my + v_nbr)`.
                    // Так как `n_to_i` противоположна для `i` и `nbr`, положительный дрейф возможен только в одну сторону!
                    let drift = 0.5 * (v_nbr + v_my).dot(n_to_i);
                    drift.max(0.0) * 0.38
                };

                let invasion_frac = (advance_vel * dt) / step_rad;
                let cell_hash =
                    ((i as f32 * 12.9898 + geology.sim_step as f32 * 4.1414).sin() * 43758.5)
                        .fract()
                        .abs();
                let threshold = 0.26 + 0.45 * cell_hash;

                if invasion_frac > threshold && invasion_frac > best_invasion {
                    best_invasion = invasion_frac;
                    best_winner = nbr_p;
                }
            }

            *pid_out = best_winner;
        });
    }

    // Фаза Б: 3 итерации топологической зачистки полосок и "зебры"
    // Клетка сохраняет плиту ТОЛЬКО если у неё есть хотя бы 2 соседа своей плиты, которые сами являются соседями друг другу!
    for pass in 0..3 {
        let (src, dst) = if pass % 2 == 0 {
            (&scratch.next_plate_ids, &mut geology.plate_ids)
        } else {
            (&geology.plate_ids, &mut scratch.next_plate_ids)
        };

        dst.par_iter_mut().enumerate().for_each(|(i, out_pid)| {
            let cand_p = src[i];
            let start = cache.edge_offsets[i];
            let end = cache.edge_offsets[i + 1];
            if end - start < 3 {
                *out_pid = cand_p;
                return;
            }

            let (my_cnt, my_pairs) = contiguous_plate_support(i, cand_p, src, cache);

            // Если у клетки есть связный треугольник своей плиты (`my_pairs >= 1`) и хотя бы 2 соседа — она устойчива
            if my_pairs >= 1 && my_cnt >= 2 {
                // Исключение: если 4+ соседей образуют мощное полукольцо другой плиты (выравнивание узких выемок)
                if my_cnt == 2 {
                    for e in start..end {
                        let np = src[cache.edge_neighbors[e]];
                        if np != cand_p {
                            let (n_cnt, n_pairs) = contiguous_plate_support(i, np, src, cache);
                            if n_cnt >= 4 && n_pairs >= 3 {
                                *out_pid = np;
                                return;
                            }
                        }
                    }
                }
                *out_pid = cand_p;
                return;
            }

            // Иначе клетка лежит в 1-гексовой полоске / зубце (`my_pairs == 0`) — отдаём её соседней плите
            // с максимальным числом взаимно-смежных пар (а при равенстве — с максимальным числом соседей)
            let mut best_p = cand_p;
            let mut best_score = 0usize;

            for e in start..end {
                let np = src[cache.edge_neighbors[e]];
                if np == cand_p {
                    continue;
                }
                let (n_cnt, n_pairs) = contiguous_plate_support(i, np, src, cache);
                let score = n_pairs * 10 + n_cnt;
                if score > best_score {
                    best_score = score;
                    best_p = np;
                }
            }

            *out_pid = best_p;
        });
    }
}

/// Физический цикл Уилсона без принудительного поддержания числа плит:
/// 1) Поглощение умирающих микроплит с аккрецией их континентальных террейнов и записью шва;
/// 2) Спаивание (Suturing) двух столкнувшихся континентальных плит при падении относительной скорости;
/// 3) Раскол (Rifting) растянутых или перегретых мантией крупных плит вдоль древних швов (`suture_weakness`).
fn evolve_wilson_cycle_physical(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let num_plates = geology.plates.len();
    if num_plates < 4 || num_cells == 0 {
        return;
    }

    let min_cells = ((num_cells as f32 * config.plate_min_area_ratio).round() as usize).max(12);
    let mut counts = vec![0usize; num_plates];
    let mut sum_dirs = vec![Vec3::ZERO; num_plates];

    for (i, &p_id) in geology.plate_ids.iter().enumerate() {
        if p_id < num_plates {
            counts[p_id] += 1;
            sum_dirs[p_id] += topology.cells[i].center;
        }
    }

    for (p_id, plate) in geology.plates.iter_mut().enumerate() {
        if counts[p_id] > 0 && sum_dirs[p_id].length_squared() > 1e-4 {
            plate.center_dir = sum_dirs[p_id].normalize();
            plate.is_alive = true;
        } else {
            plate.is_alive = false;
        }
    }

    let alive_count = geology.plates.iter().filter(|p| p.is_alive).count();

    // 1. Естественное поглощение субдуцировавшей малой плиты (`counts[p_id] < min_cells`)
    if alive_count > 4 {
        for dead_id in 0..num_plates {
            if geology.plates[dead_id].is_alive && counts[dead_id] > 0 && counts[dead_id] < min_cells
            {
                // Находим соседнюю живую плиту с наибольшей общей границей
                let mut border_hits = vec![0usize; num_plates];
                for i in 0..num_cells {
                    if geology.plate_ids[i] != dead_id {
                        continue;
                    }
                    let start = cache.edge_offsets[i];
                    let end = cache.edge_offsets[i + 1];
                    for e in start..end {
                        let np = geology.plate_ids[cache.edge_neighbors[e]];
                        if np != dead_id && np < num_plates && geology.plates[np].is_alive {
                            border_hits[np] += 1;
                        }
                    }
                }

                if let Some((host_id, &hits)) =
                    border_hits.iter().enumerate().max_by_key(|&(_, &c)| c)
                    && hits > 0
                {
                    merge_plate_into_host(topology, cache, geology, dead_id, host_id);
                    counts[host_id] += counts[dead_id];
                    counts[dead_id] = 0;
                    break;
                }
            }
        }
    }

    // 2. Спаивание (Suturing) двух континентальных плит, затормозивших при лобовом столкновении
    if geology.sim_step.is_multiple_of(6) && alive_count > 5 {
        let mut suture_pair: Option<(usize, usize)> = None;
        'outer: for p1 in 0..num_plates {
            if !geology.plates[p1].is_alive || geology.plates[p1].craton_radius <= 0.05 {
                continue;
            }
            for p2 in (p1 + 1)..num_plates {
                if !geology.plates[p2].is_alive || geology.plates[p2].craton_radius <= 0.05 {
                    continue;
                }
                let rel_omega = (geology.plates[p1].omega - geology.plates[p2].omega).length();
                if rel_omega > config.angular_vel_min * 0.52 {
                    continue;
                }

                // Проверяем, сколько континентальных клеток двух плит соприкасаются напрямую
                let mut cont_contact = 0usize;
                for i in 0..num_cells {
                    if geology.plate_ids[i] != p1 || geology.continental_factor[i] < 0.45 {
                        continue;
                    }
                    let start = cache.edge_offsets[i];
                    let end = cache.edge_offsets[i + 1];
                    for e in start..end {
                        let nbr = cache.edge_neighbors[e];
                        if geology.plate_ids[nbr] == p2 && geology.continental_factor[nbr] > 0.45 {
                            cont_contact += 1;
                            break;
                        }
                    }
                }

                if cont_contact >= 18 {
                    suture_pair = Some((p1, p2));
                    break 'outer;
                }
            }
        }

        if let Some((p1, p2)) = suture_pair {
            let (host_id, dead_id) = if counts[p1] >= counts[p2] {
                (p1, p2)
            } else {
                (p2, p1)
            };
            merge_plate_into_host(topology, cache, geology, dead_id, host_id);
            counts[host_id] += counts[dead_id];
            counts[dead_id] = 0;
        }
    }

    // 3. Детерминированный раскол плиты от внутреннего натяжения (`internal_tension`) и перегрева (`mantle_heat`)
    let free_slot = geology.plates.iter().position(|p| !p.is_alive);
    if let Some(child_id) = free_slot {
        let min_split_cells = (num_cells as f32 * config.plate_split_area_ratio * 0.75) as usize;
        let mut fracture_candidate: Option<(usize, f32)> = None;

        for (p_id, plate) in geology.plates.iter().enumerate() {
            if !plate.is_alive || counts[p_id] < min_split_cells.max(min_cells * 3) {
                continue;
            }
            // Эффективная нагрузка разрыва складывается из механического натяжения краёв и теплового ослабления снизу
            let stress_load = plate.internal_tension + plate.mantle_heat * 0.75;
            if stress_load > config.plate_fracture_threshold {
                if fracture_candidate.is_none_or(|(_, best_s)| stress_load > best_s) {
                    fracture_candidate = Some((p_id, stress_load));
                }
            }
        }

        if let Some((parent_id, _)) = fracture_candidate {
            rupture_plate_along_weak_zone(topology, cache, geology, config, parent_id, child_id);
        }
    }
}

/// Присоединяет клетки и кратонные ядра плиты `dead_id` к `host_id`, консервируя шов как древнюю сутуру
fn merge_plate_into_host(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    dead_id: usize,
    host_id: usize,
) {
    let num_cells = topology.cells.len();

    // 1. Все пограничные клетки между `dead_id` и `host_id` становятся древним швом (`suture_weakness`)
    for i in 0..num_cells {
        let p = geology.plate_ids[i];
        if p != dead_id && p != host_id {
            continue;
        }
        let start = cache.edge_offsets[i];
        let end = cache.edge_offsets[i + 1];
        for e in start..end {
            let np = geology.plate_ids[cache.edge_neighbors[e]];
            if (p == dead_id && np == host_id) || (p == host_id && np == dead_id) {
                geology.suture_weakness[i] = geology.suture_weakness[i].max(0.88);
                if geology.continental_factor[i] > 0.35 {
                    geology.orogeny_root[i] = geology.orogeny_root[i].max(0.55);
                }
                break;
            }
        }
    }

    // 2. Переносим клетки и вулканы на `host_id`
    for pid in &mut geology.plate_ids {
        if *pid == dead_id {
            *pid = host_id;
        }
    }
    for v in &mut geology.volcanoes {
        if v.plate_id == dead_id {
            v.plate_id = host_id;
        }
    }

    // 3. Переносим кратонные ядра (`lobes`) из `dead_id` в `host_id`
    let dead_plate = geology.plates[dead_id];
    let host_plate = &mut geology.plates[host_id];

    let rot_to_host = host_plate.accum_rot * dead_plate.accum_rot.inverse();
    let dead_cnt = (dead_plate.lobe_count as usize).min(MAX_PLATE_CRATON_LOBES);
    for l_idx in 0..dead_cnt {
        let dl = dead_plate.lobes[l_idx];
        if dl.radius <= 1e-4 {
            continue;
        }
        if (host_plate.lobe_count as usize) < MAX_PLATE_CRATON_LOBES {
            let dst = host_plate.lobe_count as usize;
            host_plate.lobes[dst] = CratonLobe {
                center: dl.center,
                radius: dl.radius,
                seed_offset: dl.seed_offset,
            };
            host_plate.lobe_count += 1;
        } else {
            // Если все 4 слота заняты, укрупняем ближайшее ядро
            let mut best_k = 0;
            let mut best_d = -2.0_f32;
            for k in 0..MAX_PLATE_CRATON_LOBES {
                let d = host_plate.lobes[k].center.dot(dl.center);
                if d > best_d {
                    best_d = d;
                    best_k = k;
                }
            }
            host_plate.lobes[best_k].radius =
                (host_plate.lobes[best_k].radius.powi(2) + dl.radius.powi(2))
                    .sqrt()
                    .min(0.65);
        }
    }
    let _ = rot_to_host;

    host_plate.craton_radius = (host_plate.craton_radius.powi(2) + dead_plate.craton_radius.powi(2))
        .sqrt()
        .min(0.85);
    geology.plates[dead_id].is_alive = false;
    geology.plates[dead_id].angular_vel = 0.0;
    geology.plates[dead_id].omega = Vec3::ZERO;
}

/// Раскалывает плиту `parent_id` вдоль оси растяжения с притяжением линии разлома к древним швам и плюмам
fn rupture_plate_along_weak_zone(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
    parent_id: usize,
    child_id: usize,
) {
    let num_cells = topology.cells.len();
    let parent = geology.plates[parent_id];

    // 1. Ищем центр зарождения рифта внутри плиты: клетку с максимальной суммой `suture_weakness` и близости к плюму
    let mut pivot_dir = parent.craton_center;
    let mut best_weak_score = -1.0_f32;

    for i in 0..num_cells {
        if geology.plate_ids[i] != parent_id {
            continue;
        }
        let r_i = topology.cells[i].center;
        let mut score = geology.suture_weakness[i] * 2.0 + geology.continental_factor[i] * 0.35;
        for pl in &geology.plumes {
            let d = r_i.dot(pl.position).clamp(-1.0, 1.0).acos();
            if d < pl.radius * 3.5 {
                score += (1.0 - d / (pl.radius * 3.5)) * 1.8;
            }
        }
        if score > best_weak_score {
            best_weak_score = score;
            pivot_dir = r_i;
        }
    }

    // Нормаль к плоскости разрыва ориентирована вдоль главной оси растяжения `parent.tension_axis`
    let mut split_normal =
        (parent.tension_axis - pivot_dir * pivot_dir.dot(parent.tension_axis)).normalize_or_zero();
    if split_normal == Vec3::ZERO {
        split_normal = parent.euler_pole.cross(pivot_dir).normalize_or_zero();
    }
    if split_normal == Vec3::ZERO {
        return;
    }

    // 2. Делим клетки плиты по извилистой поверхности разлома, притягивающейся к `suture_weakness`
    let mut parent_sum = Vec3::ZERO;
    let mut child_sum = Vec3::ZERO;
    let mut parent_cnt = 0usize;
    let mut child_cnt = 0usize;

    for i in 0..num_cells {
        if geology.plate_ids[i] != parent_id {
            continue;
        }
        let r_i = topology.cells[i].center;
        // Небольшой 3D-шум + отклонение вдоль слабых зон делает линию рифта геологической, а не прямой
        let s = parent.seed_offset;
        let warp = (r_i.x * 4.2 + s).sin() * (r_i.y * 3.9 - s).cos() * 0.06
            + (r_i.z * 7.1 + r_i.x * 6.3).sin() * 0.03;
        let side = (r_i - pivot_dir).dot(split_normal) + warp;

        if side > 0.0 {
            geology.plate_ids[i] = child_id;
            child_sum += r_i;
            child_cnt += 1;
        } else {
            parent_sum += r_i;
            parent_cnt += 1;
        }

        // Вдоль самого молодого рифта повышаем слабость коры
        if side.abs() < 0.045 {
            geology.suture_weakness[i] = geology.suture_weakness[i].max(0.75);
        }
    }

    // Если разрез прошёл слишком близко к краю, отменяем холостой раскол
    if parent_cnt < 20 || child_cnt < 20 {
        for pid in &mut geology.plate_ids {
            if *pid == child_id {
                *pid = parent_id;
            }
        }
        return;
    }

    let new_parent_center = parent_sum.normalize_or_zero();
    let new_child_center = child_sum.normalize_or_zero();

    // 3. Распределяем кратонные ядра (`lobes`) между двумя разошедшимися половинами щита
    let mut parent_lobes = [CratonLobe::default(); MAX_PLATE_CRATON_LOBES];
    let mut child_lobes = [CratonLobe::default(); MAX_PLATE_CRATON_LOBES];
    let mut p_lobe_cnt = 0usize;
    let mut c_lobe_cnt = 0usize;

    let orig_cnt = (parent.lobe_count as usize).min(MAX_PLATE_CRATON_LOBES);
    for l_idx in 0..orig_cnt {
        let lobe = parent.lobes[l_idx];
        if lobe.radius <= 1e-4 {
            continue;
        }
        if (lobe.center - pivot_dir).dot(split_normal) > 0.0 {
            child_lobes[c_lobe_cnt] = lobe;
            c_lobe_cnt += 1;
        } else {
            parent_lobes[p_lobe_cnt] = lobe;
            p_lobe_cnt += 1;
        }
    }

    // 4. Задаём расходящийся импульс угловой скорости обеим дочерним плитам от оси рифта
    let kick_pole = pivot_dir.cross(split_normal).normalize_or_zero();
    let kick_mag = config.angular_vel_min * 0.55;

    let p_omega = parent.omega - kick_pole * kick_mag;
    let p_speed = p_omega
        .length()
        .clamp(config.angular_vel_min, config.angular_vel_max);
    let p_pole = if p_omega.length() > 1e-4 {
        p_omega.normalize()
    } else {
        parent.euler_pole
    };

    geology.plates[parent_id].center_dir = new_parent_center;
    geology.plates[parent_id].euler_pole = p_pole;
    geology.plates[parent_id].angular_vel = p_speed;
    geology.plates[parent_id].omega = p_pole * p_speed;
    geology.plates[parent_id].lobes = parent_lobes;
    geology.plates[parent_id].lobe_count = p_lobe_cnt as u8;
    geology.plates[parent_id].internal_tension = 0.0;
    geology.plates[parent_id].mantle_heat *= 0.35;

    let c_omega = parent.omega + kick_pole * kick_mag;
    let c_speed = c_omega
        .length()
        .clamp(config.angular_vel_min, config.angular_vel_max);
    let c_pole = if c_omega.length() > 1e-4 {
        c_omega.normalize()
    } else {
        parent.euler_pole
    };

    let child_hue = ((geology.plates[child_id].color_hue + 137.5) % 360.0).clamp(0.0, 360.0);
    geology.plates[child_id] = Plate {
        center_dir: new_child_center,
        euler_pole: c_pole,
        angular_vel: c_speed,
        omega: c_pole * c_speed,
        accum_rot: parent.accum_rot,
        base_elevation_offset: parent.base_elevation_offset,
        craton_center: new_child_center,
        craton_radius: parent.craton_radius * 0.70,
        lobes: child_lobes,
        lobe_count: c_lobe_cnt as u8,
        seed_offset: parent.seed_offset + 19.7,
        color_hue: child_hue,
        tension_axis: split_normal,
        internal_tension: 0.0,
        mantle_heat: parent.mantle_heat * 0.35,
        is_alive: true,
    };

    // Перепривязываем вулканы на отколовшейся половине к `child_id`
    for v in &mut geology.volcanoes {
        if v.plate_id == parent_id && (v.center - pivot_dir).dot(split_normal) > 0.0 {
            v.plate_id = child_id;
        }
    }

    let _ = cache;
}