// src/domains/tectonics/solver/advection.rs

use super::{TectonicsGeometryCache, TectonicsScratch};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use bevy::math::Quat;
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

    // 1. Вращаем плиты, их кратоны и накопленный кватернион ориентации
    for plate in &mut geology.plates {
        let angle = plate.angular_vel * dt;
        let rot = Quat::from_axis_angle(plate.euler_pole, angle);
        plate.center_dir = (rot * plate.center_dir).normalize_or_zero();
        plate.craton_center = (rot * plate.craton_center).normalize_or_zero();
        plate.accum_rot = (rot * plate.accum_rot).normalize();
    }

    // 1б. Мягкая релаксация центров Вороного, чтобы плиты сохраняли площадь
    if num_plates >= 2 {
        let min_sep = (4.0 * PI / num_plates as f32).sqrt() * 0.52;
        for i in 0..num_plates {
            for j in (i + 1)..num_plates {
                let ci = geology.plates[i].center_dir;
                let cj = geology.plates[j].center_dir;
                let dot = ci.dot(cj).clamp(-1.0, 1.0);
                let dist = dot.acos();
                if dist > 1e-4 && dist < min_sep {
                    let push = (min_sep - dist) * 0.25;
                    let dir_ij = (cj - ci * dot).normalize_or_zero();
                    let dir_ji = (ci - cj * dot).normalize_or_zero();
                    geology.plates[i].center_dir = (ci - dir_ij * push).normalize_or_zero();
                    geology.plates[j].center_dir = (cj - dir_ji * push).normalize_or_zero();
                }
            }
        }
    }

    // 2. Двигаем вулканы вместе с плитой и переводим эрозию ВЫСОТЫ в ШИРИНУ острова!
    for v in &mut geology.volcanoes {
        if let Some(plate) = geology.plates.get(v.plate_id) {
            let angle = plate.angular_vel * dt;
            let rot = Quat::from_axis_angle(plate.euler_pole, angle);
            v.center = (rot * v.center).normalize_or_zero();
        }
        v.age += dt;

        // Потерянная от эрозии и оседания высота (dh) расплывается в стороны, увеличивая радиус острова!
        let old_h = v.height;
        let decay = (1.0 - config.volcano_erosion_rate * dt * 0.85).clamp(0.85, 0.995);
        v.height = old_h * decay;
        let lost_h = (old_h - v.height).max(0.0);

        // Конвертация потерянной высоты в ширину (закон сохранения объема конуса + расползание щита)
        v.radius = (v.radius + lost_h * 0.28 + config.hotspot_radius_growth * dt * 0.20).min(0.16);
    }
    geology.volcanoes.retain(|v| v.height > 0.045);

    // 3. Полулагранжева адвекция накопленной складчатости (orogeny_root) и осадков (sediment)
    {
        let plates = &geology.plates;
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
                let v_i = plates[p_id].omega.cross(r_i);
                let speed = v_i.length();

                if speed < 1e-5 {
                    *orog_out = orogeny[i];
                    *sed_out = sediment[i];
                    return;
                }

                let upwind_dir = -v_i / speed;
                let start = cache.edge_offsets[i];
                let end = cache.edge_offsets[i + 1];

                let mut w_sum = 0.0_f32;
                let mut up_orog = 0.0_f32;
                let mut up_sed = 0.0_f32;

                for e in start..end {
                    let align = cache.edge_tangent_dirs[e].dot(upwind_dir);
                    if align > 0.0 {
                        let w = align * align;
                        let nbr = cache.edge_neighbors[e];
                        up_orog += orogeny[nbr] * w;
                        up_sed += sediment[nbr] * w;
                        w_sum += w;
                    }
                }

                if w_sum > 1e-5 {
                    let courant = ((speed * dt) / cache.avg_cell_rad).clamp(0.0, 0.45);
                    *orog_out = orogeny[i] * (1.0 - courant) + (up_orog / w_sum) * courant;
                    *sed_out = sediment[i] * (1.0 - courant) + (up_sed / w_sum) * courant;
                } else {
                    *orog_out = orogeny[i];
                    *sed_out = sediment[i];
                }
            });
    }

    geology.orogeny_root.copy_from_slice(&scratch.next_orogeny);
    geology
        .sediment_thickness
        .copy_from_slice(&scratch.next_sediment);
}