// src/viewport/mesh_globe.rs
use rayon::prelude::*;

use crate::world::topology::WorldTopology;
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::mesh::{Indices, PrimitiveTopology},
};

use crate::domains::{
    hydrology::{HydrologyConfig, HydrologyLayer},
    tectonics::tectonics_data::GeologyLayer,
};

use bevy::render::mesh::VertexAttributeValues;

/// Оценивает точное количество вершин и индексов поверхности глобуса.
/// Используется для резервирования памяти без фиксированных многомиллионных capacity.
fn globe_mesh_sizes(topology: &WorldTopology, elevations: &[f32]) -> (usize, usize) {
    topology
        .cells
        .par_iter()
        .enumerate()
        .map(|(cell_id, cell)| {
            let corners = cell.corners.len();
            let walls = cell
                .neighbors
                .iter()
                .filter(|&&neighbor_id| elevations[cell_id] > elevations[neighbor_id])
                .count();
            (
                1 + corners + walls * 4,
                corners * 3 + walls * 6,
            )
        })
        .reduce(
            || (0usize, 0usize),
            |a, b| (a.0.saturating_add(b.0), a.1.saturating_add(b.1)),
        )
}

#[derive(Default)]
struct GlobeMeshChunk {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

pub fn build_globe_mesh(
    topology: &WorldTopology,
    elevations: &[f32],
    flatten_water: bool,
    cell_colors: &[[f32; 4]],
    radius: f32,
    height_scale: f32,
) -> Mesh {
    const CELLS_PER_CHUNK: usize = 512;

    let (vertex_capacity, index_capacity) = globe_mesh_sizes(topology, elevations);
    let chunks: Vec<GlobeMeshChunk> = topology
        .cells
        .par_chunks(CELLS_PER_CHUNK)
        .enumerate()
        .map(|(chunk_index, cells)| {
            let mut chunk = GlobeMeshChunk::default();
            let first_cell = chunk_index * CELLS_PER_CHUNK;

            for (local_cell_id, cell) in cells.iter().enumerate() {
                let cell_id = first_cell + local_cell_id;
                let real_h = elevations[cell_id];
                let render_h = if flatten_water { real_h.max(0.0) } else { real_h };
                let r = radius + render_h * height_scale;
                let top_color = cell_colors[cell_id];
                let top_normal = cell.center.to_array();

                let base_idx = chunk.positions.len() as u32;
                chunk.positions.push((cell.center * r).to_array());
                chunk.normals.push(top_normal);
                chunk.colors.push(top_color);

                let n_corners = cell.corners.len();
                for &corner_dir in &cell.corners {
                    chunk.positions.push((corner_dir * r).to_array());
                    chunk.normals.push(top_normal);
                    chunk.colors.push(top_color);
                }

                for k in 0..n_corners as u32 {
                    let next_k = (k + 1) % n_corners as u32;
                    chunk.indices.extend_from_slice(&[
                        base_idx,
                        base_idx + 1 + k,
                        base_idx + 1 + next_k,
                    ]);
                }

                let wall_color = [
                    top_color[0] * 0.75,
                    top_color[1] * 0.75,
                    top_color[2] * 0.75,
                    top_color[3],
                ];

                for k in 0..n_corners {
                    let neighbor_id = cell.neighbors[k];
                    let real_neighbor_h = elevations[neighbor_id];
                    if real_h > real_neighbor_h {
                        let render_neighbor_h = if flatten_water {
                            real_neighbor_h.max(0.0)
                        } else {
                            real_neighbor_h
                        };
                        let r_bottom = radius + render_neighbor_h * height_scale;

                        let c1 = cell.corners[k];
                        let c2 = cell.corners[(k + 1) % n_corners];
                        let v_top_left = c1 * r;
                        let v_top_right = c2 * r;
                        let v_bot_left = c1 * r_bottom;
                        let v_bot_right = c2 * r_bottom;

                        let edge_dir = v_top_right - v_top_left;
                        let down_dir = v_bot_left - v_top_left;
                        let wall_normal = down_dir.cross(edge_dir).normalize_or_zero().to_array();

                        let wall_base = chunk.positions.len() as u32;
                        chunk.positions.extend_from_slice(&[
                            v_top_left.to_array(),
                            v_top_right.to_array(),
                            v_bot_left.to_array(),
                            v_bot_right.to_array(),
                        ]);
                        chunk.normals.extend_from_slice(&[wall_normal; 4]);
                        chunk.colors.extend_from_slice(&[wall_color; 4]);
                        chunk.indices.extend_from_slice(&[
                            wall_base,
                            wall_base + 2,
                            wall_base + 1,
                            wall_base + 1,
                            wall_base + 2,
                            wall_base + 3,
                        ]);
                    }
                }
            }
            chunk
        })
        .collect();

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(vertex_capacity);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(vertex_capacity);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(vertex_capacity);
    let mut indices: Vec<u32> = Vec::with_capacity(index_capacity);

    // Chunk collection preserves cell order; only the vertex base offset changes when flattening.
    for chunk in chunks {
        let vertex_offset = positions.len() as u32;
        positions.extend(chunk.positions);
        normals.extend(chunk.normals);
        colors.extend(chunk.colors);
        indices.extend(chunk.indices.into_iter().map(|index| index + vertex_offset));
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[allow(dead_code)]
pub fn recolor_globe_mesh(
    topology: &WorldTopology,
    elevations: &[f32],
    cell_colors: &[[f32; 4]],
) -> Vec<[f32; 4]> {
    // 1. Параллельно считаем цвета каждой клетки в отдельный маленький Vec.
    let per_cell: Vec<Vec<[f32; 4]>> = topology
        .cells
        .par_iter()
        .enumerate()
        .map(|(cell_id, cell)| {
            let h = elevations[cell_id];
            let top_color = cell_colors[cell_id];
            let n_corners = cell.corners.len();

            let wall_color = [
                top_color[0] * 0.75,
                top_color[1] * 0.75,
                top_color[2] * 0.75,
                top_color[3],
            ];

            let mut local: Vec<[f32; 4]> = Vec::with_capacity(1 + n_corners + 4 * n_corners);

            local.push(top_color); // центр
            local.extend(std::iter::repeat_n(top_color, n_corners)); // углы
            for k in 0..n_corners {
                if h > elevations[cell.neighbors[k]] {
                    // отвесные стены
                    local.extend_from_slice(&[wall_color; 4]);
                }
            }
            local
        })
        .collect();

    // 2. Плоский буфер: заранее знаем размер, конкатенация в один проход.
    let total_len: usize = per_cell.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(total_len);
    for chunk in per_cell {
        out.extend_from_slice(&chunk);
    }
    out
}

pub fn reposition_globe_mesh(
    topology: &WorldTopology,
    real_elevations: &[f32],
    flatten_water: bool,
    radius: f32,
    height_scale: f32,
) -> Vec<[f32; 3]> {
    const CELLS_PER_CHUNK: usize = 512;

    let (vertex_capacity, _) = globe_mesh_sizes(topology, real_elevations);
    let chunks: Vec<Vec<[f32; 3]>> = topology
        .cells
        .par_chunks(CELLS_PER_CHUNK)
        .enumerate()
        .map(|(chunk_index, cells)| {
            let first_cell = chunk_index * CELLS_PER_CHUNK;
            let estimated_capacity = cells.iter().map(|cell| 1 + cell.corners.len() * 5).sum();
            let mut chunk_positions = Vec::with_capacity(estimated_capacity);

            for (local_cell_id, cell) in cells.iter().enumerate() {
                let cell_id = first_cell + local_cell_id;
                let real_h = real_elevations[cell_id];
                let render_h = if flatten_water { real_h.max(0.0) } else { real_h };
                let r = radius + render_h * height_scale;
                chunk_positions.push((cell.center * r).to_array());

                let n_corners = cell.corners.len();
                for &corner_dir in &cell.corners {
                    chunk_positions.push((corner_dir * r).to_array());
                }

                // Compare real elevations so the number and order of wall vertices stay unchanged.
                for k in 0..n_corners {
                    let neighbor_id = cell.neighbors[k];
                    let real_neighbor_h = real_elevations[neighbor_id];
                    if real_h > real_neighbor_h {
                        let render_neighbor_h = if flatten_water {
                            real_neighbor_h.max(0.0)
                        } else {
                            real_neighbor_h
                        };
                        let r_bottom = radius + render_neighbor_h * height_scale;
                        let c1 = cell.corners[k];
                        let c2 = cell.corners[(k + 1) % n_corners];
                        chunk_positions.extend_from_slice(&[
                            (c1 * r).to_array(),
                            (c2 * r).to_array(),
                            (c1 * r_bottom).to_array(),
                            (c2 * r_bottom).to_array(),
                        ]);
                    }
                }
            }
            chunk_positions
        })
        .collect();

    let mut positions = Vec::with_capacity(vertex_capacity);
    for chunk in chunks {
        positions.extend(chunk);
    }
    positions
}

pub fn build_river_mesh(
    topology: &WorldTopology,
    geology: &GeologyLayer,
    hydrology: &HydrologyLayer,
    config: &HydrologyConfig,
    flatten_water: bool,
    radius: f32,
    height_scale: f32,
) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let elevations = &geology.elevations;
    let flows = &hydrology.flow_accumulation;
    let flow_dirs = &hydrology.flow_dir;
    let threshold = config.river_threshold.max(1e-6);

    if flows.len() != topology.cells.len() {
        let mut empty = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        empty.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        empty.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        empty.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        empty.insert_indices(Indices::U32(indices));
        return empty;
    }

    // Небольшой подъем ленты над плоской крышей гекса, чтобы исключить Z-fighting
    let surface_lift = 0.0014 * radius;

    // Функция перевода потока в полуширину реки (в долях от радиуса гекса)
    let calc_half_width = |flow: f32, hex_scale: f32, erodibility: f32| -> f32 {
        let ratio = (flow / threshold).max(1.0);
        // Корень из полноводности: от тонкого ручья (5% гекса) до крупной реки (28% гекса)
        let norm_w = (0.045 * ratio.powf(0.38) * config.river_width_scale).clamp(0.03, 0.28);
        // В мягких осадочных породах русло шире, в твердых гранитах — уже (каньон)
        let rock_mod = 0.80 + 0.35 * erodibility;
        hex_scale * norm_w * rock_mod
    };

    let river_color = Color::srgb(0.10, 0.48, 0.88).to_linear().to_f32_array();
    let waterfall_color = Color::srgb(0.45, 0.78, 0.98).to_linear().to_f32_array();

    let mut inlets: Vec<(usize, f32)> = Vec::with_capacity(6);

    for (cell_id, cell) in topology.cells.iter().enumerate() {
        let real_h = elevations[cell_id];
        let my_flow = flows[cell_id];
        if real_h <= 0.0 || my_flow < threshold {
            continue;
        }
        let Some(out_k_u8) = flow_dirs[cell_id] else {
            continue;
        };
        let out_k = out_k_u8 as usize;
        let n_corners = cell.corners.len();
        if out_k >= n_corners {
            continue;
        }

        let render_h = if flatten_water {
            real_h.max(0.0)
        } else {
            real_h
        };
        let r_top = radius + render_h * height_scale;
        let up_normal = cell.center;
        let lift_vec = up_normal * surface_lift;

        // Центр гекса и середина выходной грани (строго в плоскости треугольника крыши!)
        let p_center = cell.center * r_top + lift_vec;
        let out_c1 = cell.corners[out_k];
        let out_c2 = cell.corners[(out_k + 1) % n_corners];
        let p_out_edge = (out_c1 + out_c2) * (0.5 * r_top) + lift_vec;

        let hex_scale = (p_out_edge - p_center).length();
        let my_erod = geology.lithology[cell_id].erodibility();
        let w_out = calc_half_width(my_flow, hex_scale, my_erod);

        // Собираем все входящие притоки (соседей, которые текут в нашу клетку и превышают порог)
        inlets.clear();
        for (k, &nbr_id) in cell.neighbors.iter().enumerate() {
            if k == out_k || elevations[nbr_id] <= 0.0 || flows[nbr_id] < threshold {
                continue;
            }
            if let Some(nbr_out_k) = flow_dirs[nbr_id] {
                let nbr_target = topology.cells[nbr_id].neighbors[nbr_out_k as usize];
                if nbr_target == cell_id {
                    inlets.push((k, flows[nbr_id]));
                }
            }
        }

        if inlets.is_empty() {
            // СЛУЧАЙ 1: Исток реки — плавно расширяется от центра гекса к выходной грани
            push_flat_ribbon(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut indices,
                p_center,
                p_out_edge,
                w_out * 0.35,
                w_out,
                up_normal,
                river_color,
            );
        } else if inlets.len() == 1 {
            // СЛУЧАЙ 2: Одиночное русло — гладкая кривая Безье (Входная грань -> Центр -> Выходная грань)
            let (in_k, in_flow) = inlets[0];
            let in_c1 = cell.corners[in_k];
            let in_c2 = cell.corners[(in_k + 1) % n_corners];
            let p_in_edge = (in_c1 + in_c2) * (0.5 * r_top) + lift_vec;
            let w_in = calc_half_width(in_flow, hex_scale, my_erod);

            let steps = 4;
            let mut prev_p = p_in_edge;
            let mut prev_w = w_in;

            for s in 1..=steps {
                let t = s as f32 / steps as f32;
                let omt = 1.0 - t;
                // Квадратичная кривая Безье в плоскости крыши гекса
                let next_p = omt * omt * p_in_edge + 2.0 * omt * t * p_center + t * t * p_out_edge;
                let next_w = w_in * omt + w_out * t;

                push_flat_ribbon(
                    &mut positions,
                    &mut normals,
                    &mut colors,
                    &mut indices,
                    prev_p,
                    next_p,
                    prev_w,
                    next_w,
                    up_normal,
                    river_color,
                );
                prev_p = next_p;
                prev_w = next_w;
            }
        } else {
            // СЛУЧАЙ 3: Слияние притоков — все входящие ветви сходятся в центральном узле и выходят через out_k
            for &(in_k, in_flow) in &inlets {
                let in_c1 = cell.corners[in_k];
                let in_c2 = cell.corners[(in_k + 1) % n_corners];
                let p_in_edge = (in_c1 + in_c2) * (0.5 * r_top) + lift_vec;
                let w_in = calc_half_width(in_flow, hex_scale, my_erod);

                push_flat_ribbon(
                    &mut positions,
                    &mut normals,
                    &mut colors,
                    &mut indices,
                    p_in_edge,
                    p_center,
                    w_in,
                    w_out * 0.90,
                    up_normal,
                    river_color,
                );
            }

            // Круглая шайба-узел в центре слияния, закрывающая угловые стыки между притоками
            push_center_hub(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut indices,
                p_center,
                w_out * 0.92,
                up_normal,
                river_color,
            );

            // Выходное русло от центра к выходной грани
            push_flat_ribbon(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut indices,
                p_center,
                p_out_edge,
                w_out * 0.92,
                w_out,
                up_normal,
                river_color,
            );
        }

        // --- ВОДОПАД НА УСТУПЕ ГЕКСА (если текущая клетка выше соседа) ---
        let out_nbr_id = cell.neighbors[out_k];
        let real_nbr_h = elevations[out_nbr_id];
        if real_h > real_nbr_h {
            let render_nbr_h = if flatten_water {
                real_nbr_h.max(0.0)
            } else {
                real_nbr_h
            };
            let r_bot = radius + render_nbr_h * height_scale;

            if r_top > r_bot + 1e-5 {
                let edge_dir = (out_c2 - out_c1).normalize_or_zero();
                let down_dir = -up_normal;
                let wall_normal = down_dir.cross(edge_dir).normalize_or_zero();
                let wall_offset = wall_normal * surface_lift;

                let edge_mid_unit = (out_c1 + out_c2) * 0.5;
                let top_mid = edge_mid_unit * r_top + lift_vec + wall_offset;
                let bot_mid = edge_mid_unit * r_bot + up_normal * surface_lift + wall_offset;

                let side = edge_dir * w_out;
                let v_top_left = (top_mid - side).to_array();
                let v_top_right = (top_mid + side).to_array();
                let v_bot_left = (bot_mid - side).to_array();
                let v_bot_right = (bot_mid + side).to_array();

                let base = positions.len() as u32;
                let wn = wall_normal.to_array();
                positions.extend_from_slice(&[v_top_left, v_top_right, v_bot_left, v_bot_right]);
                normals.extend_from_slice(&[wn; 4]);
                colors.extend_from_slice(&[
                    river_color,
                    river_color,
                    waterfall_color,
                    waterfall_color,
                ]);
                indices.extend_from_slice(&[
                    base,
                    base + 2,
                    base + 1,
                    base + 1,
                    base + 2,
                    base + 3,
                ]);
            }
        }
    }

    if positions.is_empty() {
        positions.extend_from_slice(&[[0.0, 0.0, 0.0]; 3]);
        normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 3]);
        colors.extend_from_slice(&[[0.0, 0.0, 0.0, 0.0]; 3]);
        indices.extend_from_slice(&[0, 1, 2]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[allow(clippy::too_many_arguments)]
fn push_flat_ribbon(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    a: Vec3,
    b: Vec3,
    w_a: f32,
    w_b: f32,
    up_normal: Vec3,
    color: [f32; 4],
) {
    let dir = b - a;
    if dir.length_squared() < 1e-12 {
        return;
    }
    let forward = dir.normalize();
    let right = forward.cross(up_normal).normalize_or_zero();

    let v_a_left = (a - right * w_a).to_array();
    let v_a_right = (a + right * w_a).to_array();
    let v_b_left = (b - right * w_b).to_array();
    let v_b_right = (b + right * w_b).to_array();

    let base = positions.len() as u32;
    let n_arr = up_normal.to_array();

    positions.extend_from_slice(&[v_a_left, v_a_right, v_b_left, v_b_right]);
    normals.extend_from_slice(&[n_arr; 4]);
    colors.extend_from_slice(&[color; 4]);

    indices.extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
}

#[allow(clippy::too_many_arguments)]
fn push_center_hub(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    center: Vec3,
    radius: f32,
    up_normal: Vec3,
    color: [f32; 4],
) {
    let up_ref = if up_normal.y.abs() < 0.99 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t1 = up_normal.cross(up_ref).normalize_or_zero();
    let t2 = up_normal.cross(t1).normalize_or_zero();

    let base = positions.len() as u32;
    let n_arr = up_normal.to_array();

    positions.push(center.to_array());
    normals.push(n_arr);
    colors.push(color);

    const N_SIDES: u32 = 6;
    for k in 0..N_SIDES {
        let angle = (k as f32) * std::f32::consts::TAU / (N_SIDES as f32);
        let pt = center + (t1 * angle.cos() + t2 * angle.sin()) * radius;
        positions.push(pt.to_array());
        normals.push(n_arr);
        colors.push(color);
    }

    for k in 0..N_SIDES {
        let next_k = (k + 1) % N_SIDES;
        indices.extend_from_slice(&[base, base + 1 + k, base + 1 + next_k]);
    }
}

pub fn recolor_globe_mesh_in_place(
    mesh: &mut Mesh,
    topology: &WorldTopology,
    elevations: &[f32],
    cell_colors: &[[f32; 4]],
) {
    if let Some(VertexAttributeValues::Float32x4(colors)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR)
    {
        let mut offset = 0;
        for (cell_id, cell) in topology.cells.iter().enumerate() {
            let h = elevations[cell_id];
            let top_color = cell_colors[cell_id];
            let n_corners = cell.corners.len();

            let wall_color = [
                top_color[0] * 0.75,
                top_color[1] * 0.75,
                top_color[2] * 0.75,
                top_color[3],
            ];

            // Центр
            colors[offset] = top_color;
            offset += 1;

            // Углы
            for _ in 0..n_corners {
                colors[offset] = top_color;
                offset += 1;
            }

            // Отвесные стены
            for k in 0..n_corners {
                if h > elevations[cell.neighbors[k]] {
                    colors[offset..offset + 4].copy_from_slice(&[wall_color; 4]);
                    offset += 4;
                }
            }
        }
    }
}
