// src/domains/tectonics/solver/isostasy.rs

use super::{TectonicsGeometryCache, TectonicsScratch, smoothstep};
use crate::{
    domains::tectonics::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer},
    world::topology::WorldTopology,
};
use rayon::prelude::*;

/// Гипсометрический профиль окраины от центра кратона (`c = 1.0`) через шельф к океану (`c = 0.0`).
/// Различает широкие осадочные пассивные окраины и узкие крутые активные окраины субдукции.
#[inline]
fn margin_hypsometric_curve(
    c: f32,
    cont_base: f32,
    ocean_base: f32,
    is_active_margin: f32,
    sediment_wedge: f32,
) -> f32 {
    // На пассивной окраине с мощным осадочным клином шельф шире и мельче, на активной — узкий и крутой
    let shelf_c_start = 0.24 - sediment_wedge * 0.05 + is_active_margin * 0.08;
    let land_c_start = 0.54 - sediment_wedge * 0.04 + is_active_margin * 0.05;
    let shelf_edge_h = ((-0.055_f32) - is_active_margin * 0.035).max(ocean_base * 0.35);

    if c >= land_c_start {
        let t = smoothstep((c - land_c_start) / (1.0 - land_c_start).max(0.1));
        t * cont_base
    } else if c >= shelf_c_start {
        let t = smoothstep((c - shelf_c_start) / (land_c_start - shelf_c_start).max(0.08));
        shelf_edge_h * (1.0 - t)
    } else {
        let t = smoothstep(c / shelf_c_start.max(0.08));
        ocean_base * (1.0 - t) + shelf_edge_h * t
    }
}

pub fn update_isostasy_and_erosion(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_cells = topology.cells.len();
    geology.ensure_aux_buffers(num_cells);

    // Калибруем влияние плотностей коры и мантии из конфига (закон Архимеда / Эйри)
    let rho_m = config.mantle_density.max(3000.0);
    let cont_buoyancy_ratio =
        ((rho_m - config.cont_crust_density) / (3300.0 - 2700.0)).clamp(0.4, 2.0);
    let ocean_sink_ratio = ((config.old_ocean_density - config.young_ocean_density)
        / (3130.0 - 2890.0))
        .clamp(0.3, 2.2);

    let eff_cont_base = config.cont_base_height * cont_buoyancy_ratio;
    let eff_ocean_base = config.ocean_base_height;

    {
        let plates = &geology.plates;
        let plate_ids = &geology.plate_ids;
        let age = &geology.crust_age;
        let stress = &geology.tectonic_stress;
        let orogeny = &geology.orogeny_root;
        let orog_age = &geology.orogen_age;
        let sediment = &geology.sediment_thickness;
        let shifts = &geology.subcell_shift;
        let elevations = &mut geology.elevations;

        elevations
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, elev_out)| {
                let p_id = plate_ids[i];
                let plate_offset = plates
                    .get(p_id)
                    .map(|p| p.base_elevation_offset)
                    .unwrap_or(0.0);

                let boundary_elev_delta = shifts[i][0];
                let c1 = shifts[i][1];
                let str_abs = stress[i].abs().clamp(0.0, 1.0);
                let sed_wedge = (sediment[i] / 3.5).clamp(0.0, 1.0);

                let boundary_fade = 1.0 - str_abs;
                let eff_plate_offset = plate_offset * boundary_fade * 0.5;

                // 1. Базовая высота из профиля окраины (с учётом пассивного шельфа vs активного жёлоба)
                let mut h = margin_hypsometric_curve(
                    c1,
                    eff_cont_base,
                    eff_ocean_base,
                    str_abs,
                    sed_wedge,
                ) + eff_plate_offset;

                // 2. Остывание и проседание старой океанической коры (закон Парсонса — Склейтера)
                if c1 < 0.28 {
                    let ocean_weight = 1.0 - (c1 / 0.28);
                    let age_norm = (age[i] / config.oceanic_mature_age.max(0.1)).clamp(0.0, 1.0);
                    let cooling_subsidence =
                        (age_norm.sqrt() - 0.5) * 0.10 * ocean_sink_ratio * ocean_weight;
                    h -= cooling_subsidence;
                }

                // 3. Остаточный изостатический корень древних гор (orogen memory):
                // Молодые горы поддерживаются активным boundary_elev_delta, а старые (Урал/Аппалачи) —
                // плавно пенепленизируются, сохраняя пологий кряж даже после исчезновения границы плит.
                let o_age = orog_age[i];
                let peneplain_factor = (-0.28 * o_age).exp().clamp(0.22, 1.0);
                let relic_mountain_root =
                    orogeny[i] * 0.22 * peneplain_factor * (1.0 - str_abs * 0.65);

                // 4. Суммарный тектонический рельеф
                h += boundary_elev_delta + relic_mountain_root;

                *elev_out = h.max(config.elevation_min_clamp);
            });
    }

    // 5. Нелинейная склоновая эрозия (rate ∝ |∇h|^k) вместо равномерного пластилинового размытия:
    // Срезает только крутые пики и молодые хребты, не размывая берега и плоские равнины,
    // и переводит срезанную породу в `sediment_supply` для гидрологии.
    let base_rate = (config.step_erosion_rate * dt * 6.0).clamp(0.0, 0.20);
    if base_rate > 1e-5 {
        scratch.temp_elevations.copy_from_slice(&geology.elevations);
        let old_elev = &scratch.temp_elevations;
        let orog_age = &geology.orogen_age;
        let sed_supply = &mut geology.sediment_supply;
        let ref_rad = cache.avg_cell_rad.max(1e-4);
        let slope_pow = config.nonlinear_erosion_pow.clamp(1.0, 3.0);

        geology
            .elevations
            .par_iter_mut()
            .zip(sed_supply.par_iter_mut())
            .enumerate()
            .for_each(|(i, (elev_out, supply_out))| {
                let start = cache.edge_offsets[i];
                let end = cache.edge_offsets[i + 1];
                if start == end {
                    return;
                }

                let my_h = old_elev[i];
                let mut weighted_nbr_sum = 0.0_f32;
                let mut weight_sum = 0.0_f32;
                let mut max_norm_slope = 0.0_f32;

                for e in start..end {
                    let nbr = cache.edge_neighbors[e];
                    let dist_rad = cache.edge_dists_rad[e].max(1e-5);
                    let dh = (my_h - old_elev[nbr]).abs();
                    let norm_slope = (dh / dist_rad) * ref_rad;
                    if norm_slope > max_norm_slope {
                        max_norm_slope = norm_slope;
                    }
                    // Нелинейный вес: крутые уступы обмениваются массой гораздо быстрее пологих
                    let w = (norm_slope / 0.08).max(0.05).powf(slope_pow - 1.0);
                    weighted_nbr_sum += old_elev[nbr] * w;
                    weight_sum += w;
                }

                let target_h = if weight_sum > 1e-6 {
                    weighted_nbr_sum / weight_sum
                } else {
                    my_h
                };

                // Молодые высокие хребты эродируются активно, а старые сглаженные пенеплены — медленно
                let age_resistance = 1.0 / (1.0 + orog_age[i] * 0.45);
                let nonlinear_factor = (max_norm_slope / 0.06).clamp(0.0, 2.5).powf(slope_pow);
                let eff_blend = (base_rate * nonlinear_factor * age_resistance).clamp(0.0, 0.22);

                let new_h = my_h * (1.0 - eff_blend) + target_h * eff_blend;
                let eroded_mass = (my_h - new_h).max(0.0);
                *elev_out = new_h;
                *supply_out = (*supply_out + eroded_mass * 12.0).clamp(0.0, 3.0);
            });
    }
}

/// Нелинейная диффузия рельефа (используется при одиночном шаге эрозии из пайплайна)
pub fn step_elevation_diffusion(
    topology: &WorldTopology,
    elevations: &mut [f32],
    strength: f32,
    slope_erosion_factor: f32,
    max_strength: f32,
) {
    let old_elevations = elevations.to_vec();

    elevations.par_iter_mut().enumerate().for_each(|(i, elev)| {
        let cell = &topology.cells[i];
        if cell.neighbors.is_empty() {
            return;
        }
        let my_h = old_elevations[i];
        let mut weighted_sum = 0.0_f32;
        let mut w_total = 0.0_f32;
        let mut max_slope = 0.0_f32;

        for &n_id in &cell.neighbors {
            let nh = old_elevations[n_id];
            let diff = (my_h - nh).abs();
            if diff > max_slope {
                max_slope = diff;
            }
            let w = 0.05 + diff * 12.0;
            weighted_sum += nh * w;
            w_total += w;
        }

        let avg_neighbor = weighted_sum / w_total.max(1e-5);
        // Нелинейное подавление диффузии на плоских участках (max_slope < 0.015 почти не размывается)
        let slope_gate = (max_slope / 0.035).clamp(0.05, 1.5).powf(1.6);
        let local_strength =
            ((strength * 0.35 + max_slope * slope_erosion_factor) * slope_gate).min(max_strength);
        *elev = my_h * (1.0 - local_strength) + avg_neighbor * local_strength;
    });
}