// src/domains/tectonics/solver/forces.rs

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
    let edge_weight = cache.avg_cell_rad;

    let mut plate_torques = vec![Vec3::ZERO; num_plates];
    let mut plate_areas = vec![0.0_f32; num_plates];

    // Обновляем таймеры пульсаций плюмов
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
        let mut local_force = Vec3::ZERO;

        // 1. Силы на границах плит (Slab pull, Ridge push, Collisional resistance)
        let other_p = scratch.neighbor_plate[i];
        if other_p < num_plates && other_p != p_id {
            let n_dir = scratch.boundary_normal[i]; // В сторону соседней плиты
            let conv = scratch.boundary_convergence[i];

            if conv > 0.02 {
                // Схождение плит (Конвергенция)
                if !is_cont_i && dens_i > 2860.0 {
                    // SLAB PULL: плотная старая океаническая кора заныривает и тянет свою плиту к желобу
                    let excess_density =
                        ((dens_i - config.young_ocean_density) / 240.0).clamp(0.15, 1.5);
                    let age_factor = (age_i / config.oceanic_mature_age).clamp(0.1, 1.2).sqrt();
                    local_force +=
                        n_dir * (config.slab_pull_strength * excess_density * age_factor * edge_weight);
                } else if is_cont_i {
                    // COLLISIONAL RESISTANCE: легкий континент упирается в границу и тормозит сближение
                    let thick_factor =
                        (thick_i / config.cont_crust_thickness_km).clamp(0.8, 2.2);
                    local_force -= n_dir
                        * (config.collision_resistance_strength
                            * conv
                            * thick_factor
                            * edge_weight);
                }
            } else if conv < -0.02 {
                // Расхождение плит (Дивергенция / Спрединг)
                // RIDGE PUSH: гравитационное сползание от поднятого хребта толкает плиту от разлома
                let ridge_buoyancy = (1.0 - (age_i / 0.35).clamp(0.0, 1.0)).max(0.25);
                local_force -=
                    n_dir * (config.ridge_push_strength * ridge_buoyancy * edge_weight);
            }
        }

        // 2. Периодические толчки от мантийных плюмов (Plume Push)
        for plume in &geology.plumes {
            // Импульс максимален в начале цикла пульсации плюма
            let pulse_phase = plume.pulse_timer / plume.pulse_period.max(1e-4);
            let pulse_boost = if pulse_phase < 0.25 { 2.4 } else { 0.45 };

            let dot = r_i.dot(plume.position).clamp(-1.0, 1.0);
            let dist = dot.acos();
            let push_rad = plume.radius * 3.0;
            if dist > 1e-4 && dist < push_rad {
                let away_dir = (r_i - plume.position * dot).normalize_or_zero();
                let falloff = 1.0 - (dist / push_rad);
                local_force += away_dir
                    * (config.plume_push_strength
                        * plume.strength
                        * falloff
                        * pulse_boost
                        * cell_area);
            }
        }

        // 3. Трение о мантию (Mantle Drag) — пропорционально линейной скорости клетки и корням коры
        let v_i = geology.plates[p_id].omega.cross(r_i);
        let drag_mult = if is_cont_i {
            config.continental_drag_mult
                * (thick_i / config.cont_crust_thickness_km).clamp(1.0, 1.8)
        } else {
            1.0
        };
        local_force -= v_i * (config.mantle_drag_coeff * drag_mult * cell_area);

        // Вклад силы в угловой момент плиты: tau = r x F
        plate_torques[p_id] += r_i.cross(local_force);
    }

    // Интегрируем угловую скорость (ось Эйлера + скалярная скорость) для каждой плиты
    for (p_id, plate) in geology.plates.iter_mut().enumerate() {
        let inertia = plate_areas[p_id].max(0.04);
        let alpha = plate_torques[p_id] / inertia;

        let new_omega = plate.omega + alpha * (dt * 2.5);
        let speed = new_omega.length();

        if speed > 1e-5 {
            let clamped_speed = speed.clamp(config.angular_vel_min, config.angular_vel_max);
            let pole = new_omega / speed;
            plate.euler_pole = pole;
            plate.angular_vel = clamped_speed;
            plate.omega = pole * clamped_speed;
        }
    }
}