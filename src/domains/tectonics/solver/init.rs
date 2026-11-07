// src/domains/tectonics/solver/init.rs

use super::{TectonicsGeometryCache, TectonicsScratch, smoothstep};
use crate::{
    domains::tectonics::{
        tectonics_config::TectonicsConfig,
        tectonics_data::{
            CratonLobe, GeologyLayer, MAX_PLATE_CRATON_LOBES, MantlePlume, Plate, Volcano,
        },
    },
    world::topology::WorldTopology,
};
use bevy::math::{Quat, Vec3};
use noise::NoiseFn;
use rand::{RngExt, SeedableRng, rngs::StdRng};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::f32::consts::PI;

#[derive(Copy, Clone, PartialEq)]
struct WaveNode {
    cost: f32,
    cell: usize,
}

impl Eq for WaveNode {}

impl Ord for WaveNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.total_cmp(&self.cost)
    }
}

impl PartialOrd for WaveNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn random_unit_vec3(rng: &mut impl RngExt) -> Vec3 {
    let z: f32 = rng.random_range(-1.0..1.0);
    let lon: f32 = rng.random_range(-PI..PI);
    let r = (1.0 - z * z).sqrt();
    Vec3::new(r * lon.cos(), z, r * lon.sin())
}

fn random_tangent(normal: Vec3, angle: f32) -> Vec3 {
    let helper = if normal.y.abs() < 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t1 = normal.cross(helper).normalize_or_zero();
    let t2 = normal.cross(t1).normalize_or_zero();
    (t1 * angle.cos() + t2 * angle.sin()).normalize_or_zero()
}

#[inline]
fn move_on_sphere(dir: Vec3, tangent: Vec3, arc_rad: f32) -> Vec3 {
    if tangent == Vec3::ZERO || arc_rad.abs() < 1e-6 {
        return dir;
    }
    (dir * arc_rad.cos() + tangent * arc_rad.sin()).normalize_or_zero()
}

/// Быстрый 3D-шум для модуляции скорости роста кратонов и волновых границ плит
#[inline]
fn craton_growth_noise(dir: Vec3, seed: f32) -> f32 {
    let n1 = (dir.x * 3.1 + seed).sin() * (dir.y * 2.9 - seed * 0.7).cos()
        + (dir.z * 3.4 + seed * 1.3).sin() * 0.6;
    let n2 = (dir.y * 6.7 - dir.z * 5.3 + seed * 2.1).sin()
        * (dir.x * 6.1 + dir.z * 5.9 - seed * 1.4).cos();
    let n3 = (dir.z * 11.5 + dir.x * 10.7 - seed * 0.9).sin();
    (n1 * 0.50 + n2 * 0.32 + n3 * 0.18).clamp(-1.0, 1.0)
}

/// Вычисляет континентальность для одного ядра `CratonLobe` в локальных координатах плиты
#[inline]
fn eval_single_lobe(world_dir: Vec3, local_p: Vec3, lobe: &CratonLobe) -> f32 {
    if lobe.radius <= 1e-4 {
        return 0.0;
    }
    let d_craton = world_dir.dot(lobe.center).clamp(-1.0, 1.0).acos();
    let s = lobe.seed_offset;

    let lobe_1 = (local_p.x * 2.4 + s).sin() * (local_p.z * 2.4 - s * 0.7).cos();
    let lobe_2 = (local_p.y * 4.5 - s * 1.3).sin() * (local_p.x * 4.1 + local_p.z * 3.9).cos();
    let lobe_3 = (local_p.z * 7.8 + local_p.y * 7.1 + s * 2.1).sin();

    let shape_mod = 1.0 + lobe_1 * 0.32 + lobe_2 * 0.18 + lobe_3 * 0.09;
    let land_radius = (lobe.radius * shape_mod).max(0.06);

    let shelf_and_slope_width = 0.14;
    let inner_plateau = (land_radius * 0.42).max(0.02);
    let outer_foot = land_radius + shelf_and_slope_width;

    if d_craton <= inner_plateau {
        1.0
    } else if d_craton <= land_radius {
        let t = smoothstep((d_craton - inner_plateau) / (land_radius - inner_plateau).max(1e-4));
        1.0 - t * 0.43
    } else if d_craton < outer_foot {
        let t = smoothstep((d_craton - land_radius) / shelf_and_slope_width);
        0.57 * (1.0 - t)
    } else {
        0.0
    }
}

/// Вычисляет степень континентальности коры `[0.0 .. 1.0]` в точке `dir` для составной плиты `plate`.
/// Поддерживает мультитеррейновые щиты из нескольких сросшихся кратонных ядер (`plate.lobes`).
#[inline]
pub(crate) fn craton_continental_factor(dir: Vec3, plate: &Plate) -> f32 {
    if plate.craton_radius <= 1e-4 && plate.lobe_count == 0 {
        return 0.0;
    }

    let local_p = (plate.accum_rot.inverse() * dir).normalize_or_zero();

    if plate.lobe_count > 0 {
        let count = (plate.lobe_count as usize).min(MAX_PLATE_CRATON_LOBES);
        let mut best = 0.0_f32;
        let mut second = 0.0_f32;
        for idx in 0..count {
            let val = eval_single_lobe(dir, local_p, &plate.lobes[idx]);
            if val > best {
                second = best;
                best = val;
            } else if val > second {
                second = val;
            }
        }
        // Плавное сращивание (перешеек) между двумя близкими ядрами одного континентального щита
        let neck_bridge = if best > 0.18 && second > 0.18 {
            (best * second).sqrt() * 0.35
        } else {
            0.0
        };
        return (best + neck_bridge).clamp(0.0, 1.0);
    }

    let fallback_lobe = CratonLobe {
        center: plate.craton_center,
        radius: plate.craton_radius,
        seed_offset: plate.seed_offset,
    };
    eval_single_lobe(dir, local_p, &fallback_lobe)
}

/// Находит 3 ближайшие живые плиты и возвращает геодезические расстояния до границ Вороного (в радианах)
#[allow(unused)]
#[inline]
pub(crate) fn find_closest_plates_and_boundary_dists(
    dir: Vec3,
    plates: &[Plate],
) -> (usize, usize, usize, f32, f32) {
    let mut dot_1 = -2.0_f32;
    let mut dot_2 = -2.0_f32;
    let mut dot_3 = -2.0_f32;

    let mut p1_idx = 0;
    let mut p2_idx = 0;
    let mut p3_idx = 0;

    for (i, plate) in plates.iter().enumerate() {
        if !plate.is_alive {
            continue;
        }
        let dot = dir.dot(plate.center_dir).clamp(-1.0, 1.0);
        if dot > dot_1 {
            dot_3 = dot_2;
            p3_idx = p2_idx;
            dot_2 = dot_1;
            p2_idx = p1_idx;
            dot_1 = dot;
            p1_idx = i;
        } else if dot > dot_2 {
            dot_3 = dot_2;
            p3_idx = p2_idx;
            dot_2 = dot;
            p2_idx = i;
        } else if dot > dot_3 {
            dot_3 = dot;
            p3_idx = i;
        }
    }

    let c1 = plates[p1_idx].center_dir;
    let c2 = plates[p2_idx].center_dir;
    let c3 = plates[p3_idx].center_dir;

    let chord12 = (c1 - c2).length().max(0.18);
    let chord13 = (c1 - c3).length().max(0.18);

    let bd2 = ((dot_1 - dot_2) / chord12).clamp(0.0, 1.0).asin();
    let bd3 = ((dot_1 - dot_3) / chord13).clamp(0.0, 1.0).asin();

    (p1_idx, p2_idx, p3_idx, bd2, bd3)
}

/// Генерирует начальный вектор плит с мультитеррейновыми континентальными щитами и океаническими плитами.
/// Полная сеточная разметка кратонов, древних швов и волнового роста плит выполняется в `init_tectonics`.
pub fn generate_plates(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<Plate> {
    let num_plates = config.num_plates.max(4);
    let mut plates = Vec::with_capacity(num_plates);

    let plate_angular_radius = (4.0 / num_plates as f32).sqrt();
    let pure_frac = config.pure_oceanic_plate_fraction.clamp(0.12, 0.48);
    let num_pure_oceanic = ((num_plates as f32 * pure_frac).round() as usize)
        .clamp(1, num_plates.saturating_sub(2).max(1));
    let num_continental = num_plates - num_pure_oceanic;

    // 1. Генерируем центры макро-кластеров континентальных щитов
    let num_clusters = config
        .craton_cluster_count
        .clamp(2, 6)
        .min(num_continental.max(2));
    let mut cluster_centers = Vec::with_capacity(num_clusters);
    for _ in 0..num_clusters {
        let mut best_dir = Vec3::Y;
        let mut best_dist = -1.0_f32;
        for _ in 0..16 {
            let cand = random_unit_vec3(rng);
            let min_d = cluster_centers
                .iter()
                .map(|&c: &Vec3| cand.dot(c).clamp(-1.0, 1.0).acos())
                .fold(f32::MAX, f32::min);
            if min_d > best_dist {
                best_dist = min_d;
                best_dir = cand;
            }
        }
        cluster_centers.push(best_dir);
    }

    let cont_ratio = (config.continental_ratio as f32).clamp(0.15, 0.80);

    // 2. Создаём континентальные плиты (75% в кластерах со сложными составными ядрами, 25% — одиночные террейны)
    for i in 0..num_continental {
        let is_micro_terrane = i + 1 == num_continental && num_continental >= 4;
        let base_dir = if !is_micro_terrane {
            let cl_center = cluster_centers[i % cluster_centers.len()];
            let mut best_cand = cl_center;
            let mut best_score = -1e9_f32;
            for _ in 0..config.plate_dir_iterations.max(12) {
                let az = rng.random_range(0.0..(2.0 * PI));
                let dist = rng.random_range(0.18..0.68);
                let cand = move_on_sphere(cl_center, random_tangent(cl_center, az), dist);
                let min_p_dist = plates
                    .iter()
                    .map(|p: &Plate| cand.dot(p.center_dir).clamp(-1.0, 1.0).acos())
                    .fold(f32::MAX, f32::min);
                if min_p_dist > best_score {
                    best_score = min_p_dist;
                    best_cand = cand;
                }
            }
            best_cand
        } else {
            // Одиночный микроконтинент / террейн вдали от гигантов
            let mut best_cand = Vec3::Y;
            let mut best_dist = -1.0_f32;
            for _ in 0..config.plate_dir_iterations.max(16) {
                let cand = random_unit_vec3(rng);
                let min_d = plates
                    .iter()
                    .map(|p: &Plate| cand.dot(p.center_dir).clamp(-1.0, 1.0).acos())
                    .fold(f32::MAX, f32::min);
                if min_d > best_dist {
                    best_dist = min_d;
                    best_cand = cand;
                }
            }
            best_cand
        };

        // Power-law разброс размеров: пара крупных щитов и несколько средних/малых
        let rank_t = i as f32 / (num_continental.max(1) as f32);
        let size_weight = if is_micro_terrane {
            0.48
        } else {
            (1.28 - rank_t * 0.65) * rng.random_range(0.85..1.18)
        };
        let shield_rad = plate_angular_radius * (0.44 + 0.78 * cont_ratio) * size_weight;

        let shift_az = rng.random_range(0.0..(2.0 * PI));
        let max_shift = plate_angular_radius * config.craton_eccentricity.clamp(0.1, 1.1) * 0.28;
        let shift_dist = rng.random_range((max_shift * 0.10)..max_shift.max(0.02));
        let craton_center =
            move_on_sphere(base_dir, random_tangent(base_dir, shift_az), shift_dist);

        // Формируем от 2 до 4 сросшихся кратонных ядер (террейнов) внутри каждого крупного щита
        let lobe_count = if is_micro_terrane {
            1
        } else {
            rng.random_range(2..=MAX_PLATE_CRATON_LOBES)
        };
        let mut lobes = [CratonLobe::default(); MAX_PLATE_CRATON_LOBES];
        for (l_idx, lobe) in lobes.iter_mut().enumerate().take(lobe_count) {
            let l_az =
                (l_idx as f32 * 2.0 * PI / lobe_count as f32) + rng.random_range(-0.45..0.45);
            let l_offset = if lobe_count == 1 {
                0.0
            } else {
                shield_rad * rng.random_range(0.22..0.52)
            };
            let l_center =
                move_on_sphere(craton_center, random_tangent(craton_center, l_az), l_offset);
            let l_rad = shield_rad * rng.random_range(0.58..0.88);
            *lobe = CratonLobe {
                center: l_center,
                radius: l_rad,
                seed_offset: rng.random_range(0.0..100.0),
            };
        }
        cluster_centers.push(best_dir);
    }

        let pole_cand = random_unit_vec3(rng);
        let euler_pole = if pole_cand == Vec3::ZERO {
            Vec3::Y
        } else {
            pole_cand
        };
        let angular_vel = rng.random_range(config.angular_vel_min..config.angular_vel_max);
        let var = config.plate_base_height_variance.max(0.0);
        let base_elevation_offset = if var > 0.0 {
            rng.random_range(-var..var)
        } else {
            (1.28 - rank_t * 0.65) * rng.random_range(0.85..1.18)
        };
        let seed_offset = rng.random_range(0.0..100.0);
        let color_hue = ((i as f32 * 137.50776) % 360.0).clamp(0.0, 360.0);

        plates.push(Plate {
            center_dir: base_dir,
            euler_pole,
            angular_vel,
            omega: euler_pole * angular_vel,
            accum_rot: Quat::IDENTITY,
            base_elevation_offset,
            craton_center,
            craton_radius: shield_rad,
            lobes,
            lobe_count: lobe_count as u8,
            seed_offset,
            color_hue,
            tension_axis: Vec3::X,
            internal_tension: 0.0,
            mantle_heat: 0.0,
            is_alive: true,
        });
    }

    // 3. Размещаем чисто океанические плиты как можно дальше от всех континентальных щитов
    for k in 0..num_pure_oceanic {
        let mut best_dir = Vec3::NEG_Y;
        let mut best_score = -1.0_f32;

        for _ in 0..(config.plate_dir_iterations * 2).max(24) {
            let cand = random_unit_vec3(rng);
            let min_cont_dist = plates
                .iter()
                .filter(|p| p.craton_radius > 0.0)
                .map(|p| cand.dot(p.craton_center).clamp(-1.0, 1.0).acos() - p.craton_radius * 0.6)
                .fold(f32::MAX, f32::min);
            let min_any_dist = plates
                .iter()
                .map(|p| cand.dot(p.center_dir).clamp(-1.0, 1.0).acos())
                .fold(f32::MAX, f32::min);

            let score = min_cont_dist * 0.65 + min_any_dist * 0.35;
            if score > best_score {
                best_score = score;
                best_dir = cand;
            }
        }

        let idx = num_continental + k;
        let pole_cand = random_unit_vec3(rng);
        let euler_pole = if pole_cand == Vec3::ZERO {
            Vec3::Y
        } else {
            pole_cand
        };
        let angular_vel = rng.random_range(config.angular_vel_min..config.angular_vel_max);
        let var = config.plate_base_height_variance.max(0.0);
        let base_elevation_offset = if var > 0.0 {
            rng.random_range(-var..var)
        } else {
            0.0
        };
        let seed_offset = rng.random_range(0.0..100.0);
        let color_hue = ((idx as f32 * 137.50776) % 360.0).clamp(0.0, 360.0);

        plates.push(Plate {
            center_dir: base_dir,
            euler_pole,
            angular_vel,
            omega: euler_pole * angular_vel,
            accum_rot: Quat::IDENTITY,
            base_elevation_offset,
            craton_center: best_dir,
            craton_radius: 0.0,
            lobes: [CratonLobe::default(); MAX_PLATE_CRATON_LOBES],
            lobe_count: 0,
            seed_offset,
            color_hue,
            tension_axis: Vec3::X,
            internal_tension: 0.0,
            mantle_heat: 0.0,
            is_alive: true,
        });
    }

    plates
}

pub fn generate_plumes(rng: &mut impl RngExt, config: &TectonicsConfig) -> Vec<MantlePlume> {
    let mut plumes = Vec::with_capacity(config.hotspot_count);
    for _ in 0..config.hotspot_count {
        let pos = random_unit_vec3(rng);
        if pos == Vec3::ZERO {
            continue;
        }

        let strength = rng.random_range(config.hotspot_height_min..config.hotspot_height_max);
        let radius = config.hotspot_radius_base * rng.random_range(0.85..1.45);
        let pulse_period = rng.random_range(0.12..0.32);
        let pulse_timer = rng.random_range(0.0..pulse_period);
        let next_eruption_dist = config.hotspot_spacing_rad * rng.random_range(0.45..1.45);
        let accum_drift_rad = next_eruption_dist * rng.random_range(0.55..0.98);

        plumes.push(MantlePlume {
            position: pos,
            strength,
            radius,
            pulse_timer,
            pulse_period,
            accum_drift_rad,
            next_eruption_dist,
            eruption_counter: 0, // 0 сигнализирует о первичном излиянии базальтовых траппов (LIP)
        });
    }
    plumes
}

pub fn generate_hotspots(
    rng: &mut impl RngExt,
    plates: &[Plate],
    config: &TectonicsConfig,
) -> Vec<Volcano> {
    let plumes = generate_plumes(rng, config);
    plumes
        .into_iter()
        .map(|plume| {
            let mut best_dist = f32::MAX;
            let mut plate_id = 0;
            for (i, plate) in plates.iter().enumerate() {
                let d = plume.position.dot(plate.center_dir).clamp(-1.0, 1.0).acos();
                if d < best_dist {
                    best_dist = d;
                    plate_id = i;
                }
            }
            Volcano {
                center: plume.position,
                height: plume.strength,
                radius: plume.radius,
                plate_id,
                age: 0.0,
            }
        })
        .collect()
}

/// Находит ближайшую к направлению `dir` клетку сферы за O(N) при инициализации
fn find_nearest_cell(topology: &WorldTopology, dir: Vec3) -> usize {
    let mut best_cell = 0;
    let mut best_dot = -2.0_f32;
    for (i, cell) in topology.cells.iter().enumerate() {
        let d = cell.center.dot(dir);
        if d > best_dot {
            best_dot = d;
            best_cell = i;
        }
    }
    best_cell
}

/// Выполняет полную генерацию «Сверху вниз» на сферическом графе:
/// 1) Рост кратонных ядер с шумом до выбора бюджета суши `continental_ratio`;
/// 2) Слияние близких кратонов в щиты и запись древних швов (`suture_weakness`);
/// 3) Волновой рост континентальных и океанических плит сквозь кратоны в океан;
/// 4) Инициализация устойчивой матрицы полярности субдукции `subduction_polarity`.
fn build_top_down_cratons_sutures_and_plates(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    let num_plates = geology.plates.len();
    if num_cells == 0 || num_plates < 3 {
        return;
    }

    // Собираем все кратонные ядра (террейны) со всех континентальных плит
    struct KernelSeed {
        plate_id: usize,
        _lobe_idx: usize,
        seed_cell: usize,
        speed_inv: f32,
        noise_seed: f32,
        max_cost: f32,
    }

    let mut kernels = Vec::new();
    for (p_id, plate) in geology.plates.iter().enumerate() {
        let count = (plate.lobe_count as usize).min(MAX_PLATE_CRATON_LOBES);
        if count > 0 {
            for l_idx in 0..count {
                let lobe = &plate.lobes[l_idx];
                if lobe.radius <= 1e-4 {
                    continue;
                }
                let seed_cell = find_nearest_cell(topology, lobe.center);
                let speed = (0.75 + ((lobe.seed_offset * 1.37).sin().abs()) * 0.65).max(0.4);
                kernels.push(KernelSeed {
                    plate_id: p_id,
                    _lobe_idx: l_idx,
                    seed_cell,
                    speed_inv: 1.0 / speed,
                    noise_seed: lobe.seed_offset,
                    max_cost: (lobe.radius * 1.35).max(0.12),
                });
            }
        } else if plate.craton_radius > 1e-4 {
            let seed_cell = find_nearest_cell(topology, plate.craton_center);
            kernels.push(KernelSeed {
                plate_id: p_id,
                _lobe_idx: 0,
                seed_cell,
                speed_inv: 1.0,
                noise_seed: plate.seed_offset,
                max_cost: (plate.craton_radius * 1.35).max(0.12),
            });
        }
    }

    // 1. Многоисточниковый волновой рост кратонов до достижения бюджета суши `continental_ratio`
    let target_land_cells =
        ((num_cells as f64 * config.continental_ratio.clamp(0.12, 0.82)) as usize).max(1);
    let mut cell_kernel = vec![usize::MAX; num_cells];
    let mut kernel_cost = vec![f32::MAX; num_cells];
    let mut heap = BinaryHeap::with_capacity(num_cells / 2);

    for (k_id, k) in kernels.iter().enumerate() {
        cell_kernel[k.seed_cell] = k_id;
        kernel_cost[k.seed_cell] = 0.0;
        heap.push(WaveNode {
            cost: 0.0,
            cell: k.seed_cell,
        });
    }

    let mut claimed_land = 0usize;
    while let Some(WaveNode { cost, cell }) = heap.pop() {
        if cost > kernel_cost[cell] + 1e-6 {
            continue;
        }
        let k_id = cell_kernel[cell];
        if k_id == usize::MAX {
            continue;
        }
        claimed_land += 1;
        if claimed_land >= target_land_cells {
            break;
        }

        let k = &kernels[k_id];
        if cost >= k.max_cost {
            continue;
        }

        let start = cache.edge_offsets[cell];
        let end = cache.edge_offsets[cell + 1];
        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            let step_rad = cache.edge_dists_rad[e];
            let n_dir = topology.cells[nbr].center;
            // 3D-шум делает края каждого растущего кратона изрезанными (полуострова и заливы)
            let noise_val = craton_growth_noise(n_dir, k.noise_seed);
            let step_mult = (1.0 - 0.42 * noise_val).clamp(0.45, 1.75) * k.speed_inv;
            let next_cost = cost + step_rad * step_mult;

            if next_cost < k.max_cost && next_cost < kernel_cost[nbr] {
                kernel_cost[nbr] = next_cost;
                cell_kernel[nbr] = k_id;
                heap.push(WaveNode {
                    cost: next_cost,
                    cell: nbr,
                });
            }
        }
    }

    // 2. Вычисляем древние швы (`suture_weakness`) на стыках разных кратонных ядер и профиль шельфа
    geology.continental_factor.fill(0.0);
    geology.suture_weakness.fill(0.0);
    geology.orogen_age.fill(0.0);

    let mut dist_from_land = vec![u32::MAX; num_cells];
    let mut dist_to_margin = vec![u32::MAX; num_cells];
    let mut suture_dist = vec![u32::MAX; num_cells];
    let mut bfs_land = VecDeque::with_capacity(num_cells / 3);
    let mut bfs_margin = VecDeque::with_capacity(num_cells / 3);
    let mut bfs_suture = VecDeque::with_capacity(num_cells / 8);

    for u in 0..num_cells {
        let ku = cell_kernel[u];
        if ku != usize::MAX {
            dist_from_land[u] = 0;
            bfs_land.push_back(u);

            let start = cache.edge_offsets[u];
            let end = cache.edge_offsets[u + 1];
            let mut is_margin = false;
            let mut is_suture = false;

            for e in start..end {
                let v = cache.edge_neighbors[e];
                let kv = cell_kernel[v];
                if kv == usize::MAX {
                    is_margin = true;
                } else if kv != ku {
                    // Два разных кратонных ядра столкнулись и срослись — фиксируем древнюю сутуру!
                    is_suture = true;
                }
            }

            if is_margin {
                dist_to_margin[u] = 0;
                bfs_margin.push_back(u);
            }
            if is_suture {
                suture_dist[u] = 0;
                bfs_suture.push_back(u);
            }
        }
    }

    while let Some(u) = bfs_margin.pop_front() {
        let next_d = dist_to_margin[u].saturating_add(1);
        if next_d > 6 {
            continue;
        }
        let start = cache.edge_offsets[u];
        let end = cache.edge_offsets[u + 1];
        for e in start..end {
            let v = cache.edge_neighbors[e];
            if cell_kernel[v] != usize::MAX && dist_to_margin[v] > next_d {
                dist_to_margin[v] = next_d;
                bfs_margin.push_back(v);
            }
        }
    }

    let shelf_steps = ((0.13 / cache.avg_cell_rad.max(1e-4)).round() as u32).clamp(2, 7);
    while let Some(u) = bfs_land.pop_front() {
        let next_d = dist_from_land[u].saturating_add(1);
        if next_d > shelf_steps {
            continue;
        }
        let start = cache.edge_offsets[u];
        let end = cache.edge_offsets[u + 1];
        for e in start..end {
            let v = cache.edge_neighbors[e];
            if dist_from_land[v] > next_d {
                dist_from_land[v] = next_d;
                bfs_land.push_back(v);
            }
        }
    }

    while let Some(u) = bfs_suture.pop_front() {
        let next_d = suture_dist[u].saturating_add(1);
        if next_d > 4 {
            continue;
        }
        let start = cache.edge_offsets[u];
        let end = cache.edge_offsets[u + 1];
        for e in start..end {
            let v = cache.edge_neighbors[e];
            if suture_dist[v] > next_d {
                suture_dist[v] = next_d;
                bfs_suture.push_back(v);
            }
        }
    }

    // Заполняем клеточную континентальность и древние швы (с реликтовыми кряжами вроде Урала)
    for i in 0..num_cells {
        if cell_kernel[i] != usize::MAX {
            let m_steps = dist_to_margin[i].min(6) as f32;
            let inland_t = smoothstep(m_steps / 4.0);
            geology.continental_factor[i] = 0.58 + 0.42 * inland_t;
        } else if dist_from_land[i] <= shelf_steps {
            let s_t = smoothstep(dist_from_land[i] as f32 / (shelf_steps as f32));
            geology.continental_factor[i] = 0.55 * (1.0 - s_t);
        }

        if suture_dist[i] <= 4 {
            let w = 1.0 - smoothstep(suture_dist[i] as f32 / 4.5);
            geology.suture_weakness[i] = (w * 0.90).clamp(0.0, 1.0);
            // Вдоль древнего шва остаётся невысокий состаренный кряж (прото-Урал / Аппалачи)
            geology.orogeny_root[i] = geology.orogeny_root[i].max(w * 0.28);
            geology.orogen_age[i] = 2.5 * w;
        }
    }

    // 3. Волновой рост тектонических плит сквозь кратоны в океан:
    //    Кратонные клетки жёстко принадлежат своей континентальной плите и проводят волну без сопротивления.
    let mut plate_owner = vec![usize::MAX; num_cells];
    let mut plate_wave_cost = vec![f32::MAX; num_cells];
    heap.clear();

    for i in 0..num_cells {
        let k_id = cell_kernel[i];
        if k_id != usize::MAX {
            let p_id = kernels[k_id].plate_id;
            plate_owner[i] = p_id;
            plate_wave_cost[i] = 0.0;
            heap.push(WaveNode { cost: 0.0, cell: i });
        }
    }

    for (p_id, plate) in geology.plates.iter().enumerate() {
        if plate.lobe_count == 0 && plate.craton_radius <= 1e-4 {
            let seed_cell = find_nearest_cell(topology, plate.center_dir);
            if plate_owner[seed_cell] == usize::MAX {
                plate_owner[seed_cell] = p_id;
                plate_wave_cost[seed_cell] = 0.0;
                heap.push(WaveNode {
                    cost: 0.0,
                    cell: seed_cell,
                });
            }
        }
    }

    while let Some(WaveNode { cost, cell }) = heap.pop() {
        if cost > plate_wave_cost[cell] + 1e-6 {
            continue;
        }
        let p_id = plate_owner[cell];
        if p_id == usize::MAX {
            continue;
        }
        let seed_offset = geology.plates[p_id].seed_offset;

        let start = cache.edge_offsets[cell];
        let end = cache.edge_offsets[cell + 1];
        for e in start..end {
            let nbr = cache.edge_neighbors[e];
            // Континентальное ядро никогда не отрезается чужой океанической плитой
            if cell_kernel[nbr] != usize::MAX {
                continue;
            }
            let step_rad = cache.edge_dists_rad[e];
            let n_dir = topology.cells[nbr].center;
            let wave_noise = craton_growth_noise(n_dir, seed_offset + 37.1);
            let next_cost = cost + step_rad * (1.0 - 0.25 * wave_noise).clamp(0.65, 1.35);

            if next_cost < plate_wave_cost[nbr] {
                plate_wave_cost[nbr] = next_cost;
                plate_owner[nbr] = p_id;
                heap.push(WaveNode {
                    cost: next_cost,
                    cell: nbr,
                });
            }
        }
    }

    // 4. Актуализируем центры плит по фактическим центроидам захваченных клеток и записываем `plate_ids`
    let mut sum_dirs = vec![Vec3::ZERO; num_plates];
    let mut sum_craton_dirs = vec![Vec3::ZERO; num_plates];
    let mut craton_counts = vec![0usize; num_plates];

    for i in 0..num_cells {
        let p_id = plate_owner[i].min(num_plates - 1);
        geology.plate_ids[i] = p_id;
        let dir = topology.cells[i].center;
        sum_dirs[p_id] += dir;
        if geology.continental_factor[i] > 0.35 {
            sum_craton_dirs[p_id] += dir;
            craton_counts[p_id] += 1;
        }
    }

    for (p_id, plate) in geology.plates.iter_mut().enumerate() {
        if sum_dirs[p_id].length_squared() > 1e-4 {
            plate.center_dir = sum_dirs[p_id].normalize();
        }
        if craton_counts[p_id] > 0 && sum_craton_dirs[p_id].length_squared() > 1e-4 {
            plate.craton_center = sum_craton_dirs[p_id].normalize();
            let area_frac = craton_counts[p_id] as f32 / (num_cells as f32);
            plate.craton_radius = (4.0 * area_frac).sqrt().clamp(0.10, 0.85);
        }
    }

    // 5. Инициализируем матрицу устойчивой полярности субдукции для всех пар плит
    geology.subduction_polarity.clear();
    geology
        .subduction_polarity
        .resize(num_plates * num_plates, 0);

    for i in 0..num_plates {
        for j in (i + 1)..num_plates {
            let pi = &geology.plates[i];
            let pj = &geology.plates[j];
            // Океаническая (или более плотная/древняя) плита ныряет под более лёгкую континентальную
            let buoy_i = pi.craton_radius * 2.5 + pi.base_elevation_offset * 8.0;
            let buoy_j = pj.craton_radius * 2.5 + pj.base_elevation_offset * 8.0;
            let i_subducts = if (buoy_i - buoy_j).abs() > 1e-4 {
                buoy_i < buoy_j
            } else {
                pi.seed_offset < pj.seed_offset
            };
            if i_subducts {
                geology.set_subduction_polarity(i, j);
            } else {
                geology.set_subduction_polarity(j, i);
            }
        }
    }
}

pub fn init_tectonics(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let num_cells = topology.cells.len();
    geology.resize(num_cells);
    geology.sim_step = 0;

    // Защита от вызова на пустом GeologyLayer: генерируем плиты и плюмы автоматически
    if geology.plates.len() < 3 {
        let mut fallback_rng = StdRng::seed_from_u64(0x9E37_79B9_7F4A_7C15);
        geology.plates = generate_plates(&mut fallback_rng, config);
        if geology.plumes.is_empty() && geology.volcanoes.is_empty() {
            geology.plumes = generate_plumes(&mut fallback_rng, config);
        }
    }

    if geology.plumes.is_empty() {
        if !geology.volcanoes.is_empty() {
            geology.plumes = geology
                .volcanoes
                .iter()
                .enumerate()
                .map(|(idx, v)| {
                    let next_dist =
                        config.hotspot_spacing_rad * (0.45 + ((idx * 37) % 10) as f32 * 0.10);
                    MantlePlume {
                        position: v.center.normalize_or_zero(),
                        strength: v
                            .height
                            .clamp(config.hotspot_height_min, config.hotspot_height_max),
                        radius: v.radius.max(config.volcano_radius_min),
                        pulse_period: 0.16 + (idx % 5) as f32 * 0.04,
                        pulse_timer: (idx % 3) as f32 * 0.05,
                        accum_drift_rad: next_dist * 0.85,
                        next_eruption_dist: next_dist,
                        eruption_counter: 0,
                    }
                })
                .collect();
        } else if config.hotspot_count > 0 {
            let mut fallback_rng = StdRng::seed_from_u64(0x85EB_CA6B_C2B2_AE35);
            geology.plumes = generate_plumes(&mut fallback_rng, config);
        }
    }
    geology.volcanoes.clear();

    // Строим кратоны «сверху вниз», древние сутуры и волновые границы плит
    build_top_down_cratons_sutures_and_plates(topology, cache, geology, config);

    super::boundaries::project_plates_and_boundaries(
        topology, cache, geology, scratch, config, 0.0,
    );
    // Гарантируем рождение стартовых вулканов и траппов, даже если simulation_steps == 0
    super::boundaries::spawn_hotspot_volcanoes_on_plates(
        topology,
        geology,
        config,
        config.time_step.max(0.015),
    );
    let _ = noise;
}