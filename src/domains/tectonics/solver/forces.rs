// src/domains/tectonics/solver/forces.rs:

use super::{TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use bevy::math::Vec3;

pub fn update_plate_velocities(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    use rayon::prelude::*;

    let num_plates = geology.plates.len();
    if num_plates == 0 {
        return;
    }
    let num_cells = topology.cells.len();
    let cell_area = 4.0 * std::f32::consts::PI / (num_cells.max(1) as f32);
    let edge_thresh = config.edge_threshold.clamp(0.18, 0.55);
    let edge_weight = cell_area / (edge_thresh * 0.065);

    // 1. Пульсация плюмов (быстро, ~12 итераций) — оставляем серийной
    for plume in &mut geology.plumes {
        plume.pulse_timer += dt;
        if plume.pulse_timer >= plume.pulse_period {
            plume.pulse_timer -= plume.pulse_period;
        }
    }

    // 2. Параллельный сбор партиальных моментов по клеткам
    let partials: Vec<(usize, Vec3, Vec3, f32, bool)> = (0..num_cells)
        .into_par_iter()
        .map(|i| {
            let p_id = geology.plate_ids[i];
            if p_id >= num_plates {
                return (usize::MAX, Vec3::ZERO, Vec3::ZERO, 0.0, false);
            }
            let r_i = topology.cells[i].center;
            let thick_i = geology.crust_thickness[i];
            let dens_i = geology.crust_density[i];
            let age_i = geology.crust_age[i];
            let is_cont_i = dens_i < 2835.0;

            let mut driving_force = Vec3::ZERO;
            let mut resist_force = Vec3::ZERO;
            let mut in_collision = false;

            // --- Boundary forces (перенос 1-в-1 из старого кода) ---
            let other_p = scratch.neighbor_plate[i];
            if other_p < num_plates && other_p != p_id {
                let n_dir = scratch.boundary_normal[i];
                let conv = scratch.boundary_convergence[i];
                let is_cont_other = scratch.neighbor_continental[i] > 0.38;
                let is_subducting = scratch.is_subducting_slab[i];

                if conv > 0.02 {
                    if !is_cont_i && dens_i > 2860.0 && is_subducting {
                        let excess =
                            ((dens_i - config.young_ocean_density) / 240.0).clamp(0.15, 1.5);
                        let age_factor = (age_i / config.oceanic_mature_age).clamp(0.1, 1.2).sqrt();
                        driving_force +=
                            n_dir * (config.slab_pull_strength * excess * age_factor * edge_weight);
                    } else if is_cont_i {
                        let thick_factor =
                            (thick_i / config.cont_crust_thickness_km).clamp(0.8, 2.2);
                        let collision_mult = if is_cont_other {
                            in_collision = true;
                            1.0
                        } else {
                            0.15
                        };
                        resist_force -= n_dir
                            * (config.collision_resistance_strength
                                * collision_mult
                                * conv
                                * thick_factor
                                * edge_weight);
                    }
                } else if conv < -0.02 {
                    let ridge_buoyancy = (1.0 - (age_i / 0.35).clamp(0.0, 1.0)).max(0.25);
                    driving_force -=
                        n_dir * (config.ridge_push_strength * ridge_buoyancy * edge_weight);
                }
            }

            // --- Plume push ---
            for plume in &geology.plumes {
                let pulse_phase = plume.pulse_timer / plume.pulse_period.max(1e-4);
                let pulse_boost = if pulse_phase < 0.25 { 2.4 } else { 0.45 };
                let push_rad = (plume.radius * 3.0).clamp(0.02, 0.8);
                let dot = r_i.dot(plume.position).clamp(-1.0, 1.0);
                if dot > push_rad.cos() {
                    let dist = dot.acos();
                    if dist > 1e-4 && dist < push_rad {
                        let away = (r_i - plume.position * dot).normalize_or_zero();
                        let falloff = 1.0 - (dist / push_rad);
                        driving_force += away
                            * (config.plume_push_strength
                                * plume.strength
                                * falloff
                                * pulse_boost
                                * cell_area);
                    }
                }
            }

            // --- Mantle drag ---
            let v_i = geology.plates[p_id].omega.cross(r_i);
            let drag_mult = if is_cont_i {
                config.continental_drag_mult
                    * (thick_i / config.cont_crust_thickness_km).clamp(1.0, 1.8)
            } else {
                1.0
            };
            resist_force -= v_i * (config.mantle_drag_coeff * drag_mult * cell_area);

            (
                p_id,
                r_i.cross(driving_force),
                r_i.cross(resist_force),
                cell_area,
                in_collision,
            )
        })
        .collect();

    // 3. Слияние (быстро: num_plates=15, num_cells=163K, ~1-2 мс)
    let mut plate_driving_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_resist_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_areas = vec![0.0f32; num_plates];
    let mut plate_in_collision = vec![false; num_plates];

    for (p_id, drv, res, area, coll) in partials {
        if p_id == usize::MAX {
            continue;
        }
        plate_driving_torques[p_id] += drv;
        plate_resist_torques[p_id] += res;
        plate_areas[p_id] += area;
        if coll {
            plate_in_collision[p_id] = true;
        }
    }

    // 4. Интеграция угловой скорости с разделением движущих и тормозящих моментов
    for (p_id, plate) in geology.plates.iter_mut().enumerate() {
        let inertia = plate_areas[p_id].max(0.04);
        let drive_alpha = plate_driving_torques[p_id] / inertia;
        let resist_alpha = plate_resist_torques[p_id] / inertia;

        // 1. Применяем тягу слэбов, толчок хребтов и импульсы плюмов
        let omega_driven = plate.omega + drive_alpha * (dt * 2.5);
        let driven_speed = omega_driven.length();

        if driven_speed > 1e-5 {
            let dir = omega_driven / driven_speed;
            let resist_step = resist_alpha * (dt * 2.5);

            // Продольная компонента сопротивления только тормозит вращение (не может развернуть плиту вспять),
            // а поперечная — плавно уводит полюс Эйлера в сторону от лобовой коллизии материков
            let parallel_drag = resist_step.dot(dir).min(0.0);
            let perp_deflect = resist_step - dir * resist_step.dot(dir);

            let forward_speed = (driven_speed + parallel_drag).max(driven_speed * 0.25);
            let new_omega = dir * forward_speed + perp_deflect * 0.55;
            let new_speed = new_omega.length();

            // При лобовом столкновении двух континентов разрешаем плите замедляться сильнее
            let min_vel = if plate_in_collision[p_id] {
                config.angular_vel_min * 0.45
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
    }
    let _ = cache;
}
