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
    let num_plates = geology.plates.len();
    if num_plates == 0 {
        return;
    }

    let num_cells = topology.cells.len();
    let cell_area = 4.0 * std::f32::consts::PI / (num_cells.max(1) as f32);
    let edge_thresh = config.edge_threshold.clamp(0.18, 0.55);
    // Нормируем вес клетки граничной полосы через cell_area / edge_thresh,
    // чтобы интеграл вдоль разлома не зависел от разрешения сетки num_cells.
    let edge_weight = cell_area / (edge_thresh * 0.065);

    let mut plate_driving_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_resist_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_areas = vec![0.0_f32; num_plates];
    let mut plate_in_collision = vec![false; num_plates];

    for plume in &mut geology.plumes {
        plume.pulse_timer += dt;
        if plume.pulse_timer >= plume.pulse_period {
            plume.pulse_timer -= plume.pulse_period;
        }
    }

    for i in 0..num_cells {
        let p_id = geology.plate_ids[i];
        if p_id >= num_plates {
            continue;
        }

        let r_i = topology.cells[i].center;
        let thick_i = geology.crust_thickness[i];
        let dens_i = geology.crust_density[i];
        let age_i = geology.crust_age[i];
        let is_cont_i = geology.is_continental_crust(i);

        plate_areas[p_id] += cell_area;
        let mut driving_force = Vec3::ZERO;
        let mut resist_force = Vec3::ZERO;

        // 1. Силы на границах плит (Slab pull, Ridge push, Collisional resistance)
        let other_p = scratch.neighbor_plate[i];
        if other_p < num_plates && other_p != p_id {
            let n_dir = scratch.boundary_normal[i];
            let conv = scratch.boundary_convergence[i];
            let is_cont_other = scratch.neighbor_continental[i] > 0.38;
            let is_subducting = scratch.is_subducting_slab[i];

            if conv > 0.02 {
                if !is_cont_i && dens_i > 2860.0 && is_subducting {
                    // SLAB PULL: тянет только нижнюю (погружающуюся) океаническую плиту
                    let excess_density =
                        ((dens_i - config.young_ocean_density) / 240.0).clamp(0.15, 1.5);
                    let age_factor = (age_i / config.oceanic_mature_age).clamp(0.1, 1.2).sqrt();
                    driving_force += n_dir
                        * (config.slab_pull_strength * excess_density * age_factor * edge_weight);
                } else if is_cont_i {
                    // COLLISIONAL RESISTANCE: полное торможение при столкновении двух континентов,
                    // и лишь малое сопротивление при субдукции океана под континент
                    let thick_factor =
                        (thick_i / config.cont_crust_thickness_km).clamp(0.8, 2.2);
                    let collision_mult = if is_cont_other {
                        plate_in_collision[p_id] = true;
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
                // RIDGE PUSH
                let ridge_buoyancy = (1.0 - (age_i / 0.35).clamp(0.0, 1.0)).max(0.25);
                driving_force -=
                    n_dir * (config.ridge_push_strength * ridge_buoyancy * edge_weight);
            }
        }

        // 2. Периодические толчки от мантийных плюмов (Plume Push)
        for plume in &geology.plumes {
            let pulse_phase = plume.pulse_timer / plume.pulse_period.max(1e-4);
            let pulse_boost = if pulse_phase < 0.25 { 2.4 } else { 0.45 };

            let push_rad = (plume.radius * 3.0).clamp(0.02, 0.8);
            let dot = r_i.dot(plume.position).clamp(-1.0, 1.0);
            if dot > push_rad.cos() {
                let dist = dot.acos();
                if dist > 1e-4 && dist < push_rad {
                    let away_dir = (r_i - plume.position * dot).normalize_or_zero();
                    let falloff = 1.0 - (dist / push_rad);
                    driving_force += away_dir
                        * (config.plume_push_strength
                            * plume.strength
                            * falloff
                            * pulse_boost
                            * cell_area);
                }
            }
        }

        // 3. Трение о мантию (Mantle Drag)
        let v_i = geology.plates[p_id].omega.cross(r_i);
        let drag_mult = if is_cont_i {
            config.continental_drag_mult
                * (thick_i / config.cont_crust_thickness_km).clamp(1.0, 1.8)
        } else {
            1.0
        };
        resist_force -= v_i * (config.mantle_drag_coeff * drag_mult * cell_area);

        plate_driving_torques[p_id] += r_i.cross(driving_force);
        plate_resist_torques[p_id] += r_i.cross(resist_force);
    }

    // Интегрируем угловую скорость для каждой плиты без паразитного отскока от сил трения
    for (p_id, plate) in geology.plates.iter_mut().enumerate() {
        let inertia = plate_areas[p_id].max(0.04);
        let eff_dt = dt * 2.5;

        // Вначале прибавляем активные движущие моменты
        let omega_after_drive = plate.omega + (plate_driving_torques[p_id] / inertia) * eff_dt;
        let drive_speed = omega_after_drive.length();

        // Тормозящий момент (трение + сопротивление коллизии) не может развернуть плиту вспять за 1 шаг
        let resist_delta = (plate_resist_torques[p_id] / inertia) * eff_dt;
        let new_omega = if drive_speed > 1e-5 {
            let dir = omega_after_drive / drive_speed;
            let long_resist = resist_delta.dot(dir);
            let perp_resist = resist_delta - dir * long_resist;
            // Ограничиваем продольное торможение, чтобы скорость вдоль оси не меняла знак (макс. гашение 75% за тик)
            let clamped_long = long_resist.max(-drive_speed * 0.75);
            omega_after_drive + dir * clamped_long + perp_resist * 0.5
        } else {
            omega_after_drive
        };

        let speed = new_omega.length();
        // Если плита находится в лобовой континентальной коллизии, разрешаем ей замедляться ниже angular_vel_min
        let min_allowed_speed = if plate_in_collision[p_id] {
            config.angular_vel_min * 0.15
        } else {
            config.angular_vel_min
        };

        if speed > 1e-5 {
            let clamped_speed = speed.clamp(min_allowed_speed, config.angular_vel_max);
            let pole = new_omega / speed;
            plate.euler_pole = pole;
            plate.angular_vel = clamped_speed;
            plate.omega = pole * clamped_speed;
        } else {
            plate.angular_vel = min_allowed_speed;
            plate.omega = plate.euler_pole * plate.angular_vel;
        }
    }

    let _ = cache;
}