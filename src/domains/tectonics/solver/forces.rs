// src/domains/tectonics/solver/forces.rs

use super::{TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use bevy::math::Vec3;
use rayon::prelude::*;

#[derive(Clone, Copy, Default)]
struct CellForceContrib {
    plate_id: usize,
    driving_torque: Vec3,
    resist_torque: Vec3,
    /// Проекция радиального растягивающего усилия (от центра плиты к краю)
    radial_tension: f32,
    /// Вектор направления максимального растяжения в касательной плоскости плиты
    tension_vec: Vec3,
    cell_area: f32,
    cont_area: f32,
    in_collision: bool,
}

pub fn update_plate_velocities(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_plates = geology.plates.len();
    if num_plates == 0 {
        return;
    }
    let num_cells = topology.cells.len();
    geology.ensure_aux_buffers(num_cells);

    let cell_area = 4.0 * std::f32::consts::PI / (num_cells.max(1) as f32);
    let edge_thresh = config.edge_threshold.clamp(0.18, 0.55);
    let edge_weight = cell_area / (edge_thresh * 0.065);

    // 1. Пульсация мантийных плюмов
    for plume in &mut geology.plumes {
        plume.pulse_timer += dt;
        if plume.pulse_timer >= plume.pulse_period {
            plume.pulse_timer -= plume.pulse_period;
        }
    }

    // 2. Параллельный сбор крутящих моментов и внутренних растягивающих напряжений по клеткам
    let partials: Vec<CellForceContrib> = (0..num_cells)
        .into_par_iter()
        .map(|i| {
            let p_id = geology.plate_ids[i];
            if p_id >= num_plates || !geology.plates[p_id].is_alive {
                return CellForceContrib {
                    plate_id: usize::MAX,
                    ..Default::default()
                };
            }

            let plate = &geology.plates[p_id];
            let r_i = topology.cells[i].center;
            let thick_i = geology.crust_thickness[i];
            let dens_i = geology.crust_density[i];
            let age_i = geology.crust_age[i];
            let c_i = geology.continental_factor[i];
            let is_cont_i = c_i > 0.36 || dens_i < 2835.0;

            let mut driving_force = Vec3::ZERO;
            let mut resist_force = Vec3::ZERO;
            let mut in_collision = false;

            // --- Граничные силы с учётом полярности субдукции и трения трансформных сдвигов ---
            let other_p = scratch.neighbor_plate[i];
            if other_p < num_plates && other_p != p_id && geology.plates[other_p].is_alive {
                let n_dir = scratch.boundary_normal[i];
                let conv = scratch.boundary_convergence[i];
                let shear = scratch.boundary_shear[i];
                let is_cont_other = scratch.neighbor_continental[i] > 0.36;
                let is_subducting = scratch.is_subducting_slab[i];

                if conv > 0.015 {
                    if !is_cont_i && is_subducting {
                        // Тяга холодного тонущего слэба (Slab Pull) действует строго на ныряющую плиту
                        let excess =
                            ((dens_i - config.young_ocean_density) / 240.0).clamp(0.20, 1.6);
                        let age_factor =
                            (age_i / config.oceanic_mature_age.max(0.1)).clamp(0.15, 1.3).sqrt();
                        driving_force +=
                            n_dir * (config.slab_pull_strength * excess * age_factor * edge_weight);
                    } else if is_cont_i {
                        // Сопротивление коллизии материков и сжатия наезжающей плиты
                        let thick_factor =
                            (thick_i / config.cont_crust_thickness_km).clamp(0.8, 2.2);
                        let collision_mult = if is_cont_other {
                            in_collision = true;
                            1.15
                        } else {
                            0.22
                        };
                        resist_force -= n_dir
                            * (config.collision_resistance_strength
                                * collision_mult
                                * conv
                                * thick_factor
                                * edge_weight);
                    }
                } else if conv < -0.015 {
                    // Толчок срединно-океанического хребта / континентального рифта (Ridge Push)
                    let ridge_buoyancy = (1.0 - (age_i / 0.40).clamp(0.0, 1.0)).max(0.28);
                    driving_force -=
                        n_dir * (config.ridge_push_strength * ridge_buoyancy * edge_weight);
                }

                // Трение вдоль трансформных (сдвиговых) разломов
                if shear > 0.015 {
                    let v_i = plate.omega.cross(r_i);
                    let v_other = geology.plates[other_p].omega.cross(r_i);
                    let rel_v = v_i - v_other;
                    let tang_v = rel_v - n_dir * rel_v.dot(n_dir);
                    let tang_dir = tang_v.normalize_or_zero();
                    let fault_weakening = 1.0 - geology.suture_weakness[i] * 0.45;
                    resist_force -= tang_dir
                        * (config.mantle_drag_coeff
                            * 0.55
                            * shear
                            * fault_weakening
                            * edge_weight);
                }
            }

            // --- Радиальный напор мантийных плюмов (Plume Push) ---
            let mut plume_radial_pull = 0.0_f32;
            for plume in &geology.plumes {
                let pulse_phase = plume.pulse_timer / plume.pulse_period.max(1e-4);
                let pulse_boost = if pulse_phase < 0.25 { 2.4 } else { 0.48 };
                let push_rad = (plume.radius * 3.0).clamp(0.02, 0.8);
                let dot = r_i.dot(plume.position).clamp(-1.0, 1.0);
                if dot > push_rad.cos() {
                    let dist = dot.acos();
                    if dist > 1e-4 && dist < push_rad {
                        let away = (r_i - plume.position * dot).normalize_or_zero();
                        let falloff = 1.0 - (dist / push_rad);
                        let p_force = config.plume_push_strength
                            * plume.strength
                            * falloff
                            * pulse_boost
                            * cell_area;
                        driving_force += away * p_force;
                        plume_radial_pull += p_force * 0.65;
                    }
                }
            }

            // --- Вязкое трение подошвы литосферы о мантию (Mantle Drag) ---
            let v_i = plate.omega.cross(r_i);
            let drag_mult = if is_cont_i {
                config.continental_drag_mult
                    * (thick_i / config.cont_crust_thickness_km).clamp(1.0, 1.8)
            } else {
                1.0
            };
            resist_force -= v_i * (config.mantle_drag_coeff * drag_mult * cell_area);

            // --- Вклад клетки во внутреннее растягивающее напряжение плиты (Tension) ---
            // Проецируем положение клетки в касательную плоскость центра плиты `c_p`
            let c_p = plate.center_dir;
            let offset_from_center = (r_i - c_p * c_p.dot(r_i)).normalize_or_zero();
            // Если сила тянет край плиты наружу от её центра (Slab Pull на окраине или плюм изнутри),
            // это создаёт растягивающее напряжение вдоль диаметральной оси `offset_from_center`.
            let outward_pull = driving_force.dot(offset_from_center).max(0.0) + plume_radial_pull;
            // Учитываем локальное ослабление прочности коры древними швами (`suture_weakness`)
            let weakness_boost = 1.0 + geology.suture_weakness[i] * (config.suture_weakness_mult - 1.0).max(0.0);
            let eff_tension = outward_pull * weakness_boost;

            CellForceContrib {
                plate_id: p_id,
                driving_torque: r_i.cross(driving_force),
                resist_torque: r_i.cross(resist_force),
                radial_tension: eff_tension,
                tension_vec: offset_from_center * eff_tension,
                cell_area,
                cont_area: if is_cont_i { cell_area } else { 0.0 },
                in_collision,
            }
        })
        .collect();

    // 3. Агрегация моментов, площади и осей растяжения по плитам
    let mut plate_driving_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_resist_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_areas = vec![0.0_f32; num_plates];
    let mut plate_cont_areas = vec![0.0_f32; num_plates];
    let mut plate_tension_sum = vec![0.0_f32; num_plates];
    let mut plate_tension_vec = vec![Vec3::ZERO; num_plates];
    let mut plate_in_collision = vec![false; num_plates];

    for item in partials {
        let p_id = item.plate_id;
        if p_id >= num_plates {
            continue;
        }
        plate_driving_torques[p_id] += item.driving_torque;
        plate_resist_torques[p_id] += item.resist_torque;
        plate_areas[p_id] += item.cell_area;
        plate_cont_areas[p_id] += item.cont_area;
        plate_tension_sum[p_id] += item.radial_tension;
        plate_tension_vec[p_id] += item.tension_vec;
        if item.in_collision {
            plate_in_collision[p_id] = true;
        }
    }

    // 4. Интеграция угловой скорости, прогрева суперконтинента и накопленного растяжения
    let split_ref_area = (4.0 * std::f32::consts::PI * config.plate_split_area_ratio.max(0.05)).max(0.25);
    let step_f = geology.sim_step as f32;

    for (p_id, plate) in geology.plates.iter_mut().enumerate() {
        if !plate.is_alive || plate_areas[p_id] <= 1e-5 {
            plate.angular_vel = 0.0;
            plate.omega = Vec3::ZERO;
            continue;
        }

        // Момент инерции сферической шапки растет нелинейно с площадью плиты
        let area = plate_areas[p_id].max(0.04);
        let inertia = (area * (0.65 + 0.35 * (area / split_ref_area).min(2.0))).max(0.04);
        let drive_alpha = plate_driving_torques[p_id] / inertia;
        let resist_alpha = plate_resist_torques[p_id] / inertia;

        // Плавное блуждание полюса Эйлера (создаёт изгибы цепей горячих точек вроде Гавайско-Императорской)
        let wander_phase = step_f * 0.11 + plate.seed_offset;
        let wander_vec = Vec3::new(
            wander_phase.sin(),
            (wander_phase * 1.37 + 2.1).cos(),
            (wander_phase * 0.83 - 1.4).sin(),
        )
        .normalize_or_zero();
        let wander_torque = wander_vec * (config.euler_pole_wander * 0.12);

        let omega_driven = plate.omega + (drive_alpha + wander_torque) * (dt * 2.5);
        let driven_speed = omega_driven.length();

        if driven_speed > 1e-5 {
            let dir = omega_driven / driven_speed;
            let resist_step = resist_alpha * (dt * 2.5);

            // Продольное сопротивление только тормозит плиту, а поперечное уводит ось вращения от коллизии
            let parallel_drag = resist_step.dot(dir).min(0.0);
            let perp_deflect = resist_step - dir * resist_step.dot(dir);

            let forward_speed = (driven_speed + parallel_drag).max(driven_speed * 0.22);
            let new_omega = dir * forward_speed + perp_deflect * 0.58;
            let new_speed = new_omega.length();

            let min_vel = if plate_in_collision[p_id] {
                config.angular_vel_min * 0.30
            } else {
                config.angular_vel_min
            };

            if new_speed > 1e-5 {
                let clamped_speed = new_speed.clamp(min_vel, config.angular_vel_max);
                let pole = new_omega / new_speed;
                plate.euler_pole = pole;
                plate.angular_vel = clamped_speed;
                plate.omega = pole * clamped_speed;
            }
        } else {
            plate.angular_vel = config.angular_vel_min;
            plate.omega = plate.euler_pole * plate.angular_vel;
        }

        // Обновляем накопленный тепловой подпор мантии под крупными континентами (теплоизоляция суперконтинента)
        let cont_ratio = plate_cont_areas[p_id] / split_ref_area;
        if cont_ratio > 0.55 {
            let heat_gain = (cont_ratio - 0.55) * config.supercontinent_heat_rate * dt;
            plate.mantle_heat = (plate.mantle_heat + heat_gain).clamp(0.0, 2.0);
        } else {
            plate.mantle_heat = (plate.mantle_heat * (-0.25 * dt).exp()).max(0.0);
        }

        // Обновляем внутреннее растягивающее напряжение и главную ось разрыва плиты
        let inst_tension = (plate_tension_sum[p_id] / area.sqrt().max(0.20)) * 0.38;
        let tension_decay = (-0.45 * dt).exp();
        plate.internal_tension =
            (plate.internal_tension * tension_decay + inst_tension * (1.0 - tension_decay)).clamp(0.0, 4.0);

        let raw_t_axis = plate_tension_vec[p_id]
            - plate.center_dir * plate.center_dir.dot(plate_tension_vec[p_id]);
        if raw_t_axis.length_squared() > 1e-6 {
            let new_axis = raw_t_axis.normalize();
            // Выбираем знак оси согласованно с предыдущим шагом
            let signed_axis = if new_axis.dot(plate.tension_axis) < 0.0 {
                -new_axis
            } else {
                new_axis
            };
            plate.tension_axis = (plate.tension_axis * 0.65 + signed_axis * 0.35).normalize_or_zero();
        }
        if plate.tension_axis == Vec3::ZERO {
            plate.tension_axis = plate.euler_pole.cross(plate.center_dir).normalize_or_zero();
        }
    }

    let _ = cache;
}