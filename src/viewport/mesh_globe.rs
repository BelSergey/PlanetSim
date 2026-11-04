// src/viewport/mesh_globe.rs

use crate::world::topology::WorldTopology;
use bevy::mesh::VertexAttributeValues;
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::mesh::{Indices, PrimitiveTopology},
};
use rayon::prelude::*;

use crate::domains::{
    hydrology::{HydrologyConfig, HydrologyLayer},
    tectonics::tectonics_data::GeologyLayer,
};

const WALL_THRESHOLD: f32 = 0.005;

/// Промежуточный буфер для параллельной сборки речного меша.
#[derive(Default)]
struct RiverMeshChunk {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl RiverMeshChunk {
    fn with_capacity(cap: usize) -> Self {
        Self {
            positions: Vec::with_capacity(cap * 4),
            normals: Vec::with_capacity(cap * 4),
            colors: Vec::with_capacity(cap * 4),
            indices: Vec::with_capacity(cap * 6),
        }
    }

    /// Добавляет чанк к текущему, сдвигая индексы на текущий `positions.len()`.
    fn merge(&mut self, other: RiverMeshChunk) {
        let offset = self.positions.len() as u32;
        self.positions.extend(other.positions);
        self.normals.extend(other.normals);
        self.colors.extend(other.colors);
        self.indices
            .extend(other.indices.into_iter().map(|i| i + offset));
    }
}

/// Оценивает точное количество вершин и индексов с учётом стенок.
fn globe_mesh_sizes(topology: &WorldTopology, elevations: &[f32]) -> (usize, usize) {
    topology
        .cells
        .par_iter()
        .enumerate()
        .map(|(cell_id, cell)| {
            let Some(&height) = elevations.get(cell_id) else {
                return (0, 0);
            };
            let walls = cell
                .neighbors
                .iter()
                .filter(|&&neighbor_id| {
                    elevations
                        .get(neighbor_id)
                        .is_some_and(|&neighbor| height > neighbor)
                })
                .count();
            let corners = cell.corners.len();
            (1 + corners + walls * 4, corners * 3 + walls * 6)
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
}

#[derive(Default)]
struct GlobeMeshChunk {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

/// Возвращает ближайшее представление долготы относительно заданной ветви.
#[inline]
fn unwrap_longitude(mut longitude: f32, reference: f32) -> f32 {
    let half_turn = std::f32::consts::PI;
    let full_turn = std::f32::consts::TAU;
    while longitude - reference > half_turn {
        longitude -= full_turn;
    }
    while longitude - reference < -half_turn {
        longitude += full_turn;
    }
    longitude
}

/// Выбирает опорную долготу ячейки для развёртки её углов в одну ветвь.
/// Вблизи полюсов долгота центра не определена, поэтому используем первый
/// угол ячейки как устойчивую опорную точку.
#[inline]
fn cell_reference_longitude(center: Vec3, corners: &[Vec3]) -> f32 {
    if center.x * center.x + center.z * center.z > 1e-8 {
        center.x.atan2(center.z)
    } else {
        corners.first().map(|p| p.x.atan2(p.z)).unwrap_or(0.0)
    }
}
/// Проецирует точку без ограничения по краям карты. Это используется при
/// разрезании треугольников на антимеридиане.
#[inline]
fn project_map_point_unclamped(dir: Vec3, ref_lon: f32, radius: f32, height: f32) -> Vec3 {
    let dir = dir.normalize_or_zero();
    let latitude = dir.y.clamp(-1.0, 1.0).asin();
    let horizontal_sq = dir.x * dir.x + dir.z * dir.z;
    let longitude = if horizontal_sq < 1e-8 {
        ref_lon
    } else {
        unwrap_longitude(dir.x.atan2(dir.z), ref_lon)
    };
    Vec3::new(longitude * radius, latitude * radius, height)
}

/// Проецирует географическое ребро стены в диапазон карты. Ребро,
/// пересекающее антимеридиан, не рисуется как длинная линия через всю карту:
/// это край карты, а не реальный склон.
#[inline]
fn project_map_wall_edge(c1: Vec3, c2: Vec3) -> Option<(Vec2, Vec2)> {
    use std::f32::consts::{PI, TAU};

    let lon1 = c1.x.atan2(c1.z);
    let mut lon2 = unwrap_longitude(c2.x.atan2(c2.z), lon1);
    let mut lon1 = lon1;
    let midpoint = 0.5 * (lon1 + lon2);
    let wrap = (midpoint / TAU).round() * TAU;
    lon1 -= wrap;
    lon2 -= wrap;

    let tolerance = 1e-5;
    if lon1 < -PI - tolerance
        || lon1 > PI + tolerance
        || lon2 < -PI - tolerance
        || lon2 > PI + tolerance
    {
        return None;
    }

    let lat1 = c1.y.clamp(-1.0, 1.0).asin();
    let lat2 = c2.y.clamp(-1.0, 1.0).asin();
    Some((Vec2::new(lon1, lat1), Vec2::new(lon2, lat2)))
}

#[inline]
fn push_map_triangle(output: &mut Vec<Vec3>, a: Vec3, mut b: Vec3, mut c: Vec3, area_epsilon: f32) {
    // Карта видна со стороны +Z. Единая ориентация треугольников исключает
    // исчезновение южных/полярных фрагментов из-за back-face culling.
    let area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area2 < 0.0 {
        std::mem::swap(&mut b, &mut c);
    }
    if area2.abs() > area_epsilon {
        output.extend_from_slice(&[a, b, c]);
    }
}

/// Отсекает полигон по вертикальной границе карты X=bound.
fn clip_polygon_x(polygon: Vec<Vec3>, bound: f32, keep_greater: bool) -> Vec<Vec3> {
    if polygon.is_empty() {
        return polygon;
    }
    let inside = |p: Vec3| {
        if keep_greater {
            p.x >= bound
        } else {
            p.x <= bound
        }
    };
    let mut output = Vec::with_capacity(polygon.len() + 1);
    let mut previous = *polygon.last().unwrap();
    let mut previous_inside = inside(previous);

    for current in polygon {
        let current_inside = inside(current);
        if current_inside != previous_inside {
            let dx = current.x - previous.x;
            if dx.abs() > 1e-8 {
                let t = ((bound - previous.x) / dx).clamp(0.0, 1.0);
                let mut intersection = previous.lerp(current, t);
                intersection.x = bound;
                output.push(intersection);
            }
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_inside = current_inside;
    }
    output
}

/// Создаёт top-поверхность одной ячейки в виде обрезанных треугольников.
/// Треугольники около ±180° дублируются на противоположной кромке, а полярные
/// вершины получают долготу середины своего ребра (у полюса долгота не определена).
fn project_cell_top_map_triangles(
    cell: &crate::world::topology::TopologicalCell,
    radius: f32,
    height: f32,
) -> Vec<Vec3> {
    use std::f32::consts::{PI, TAU};

    let mut output = Vec::with_capacity(cell.corners.len() * 6);
    let edge = PI * radius;
    let polar = cell.center.x * cell.center.x + cell.center.z * cell.center.z < 1e-8;

    // На полюсе все долготы сходятся в одну точку. В равнопрямоугольной
    // проекции эта точка должна стать верхней/нижней кромкой карты, а не
    // единственным центральным пиком. Поэтому полярную ячейку строим полосой:
    // верхняя сторона идёт от -180° до +180°, нижняя повторяет её контур.
    if polar {
        let north = cell.center.y >= 0.0;
        let pole_y = cell.center.y.signum() * 0.5 * PI * radius;
        let mut ring = cell
            .corners
            .iter()
            .map(|corner| {
                let direction = corner.normalize_or_zero();
                (
                    direction.x.atan2(direction.z),
                    direction.y.clamp(-1.0, 1.0).asin(),
                )
            })
            .collect::<Vec<_>>();
        ring.sort_by(|a, b| a.0.total_cmp(&b.0));

        if ring.len() >= 2 {
            let (left_lon, left_lat) = ring[0];
            let (right_lon, right_lat) = *ring.last().unwrap();
            let wrapped_left = left_lon + TAU;
            let denominator = (wrapped_left - right_lon).abs().max(1e-6);
            let t = ((PI - right_lon) / denominator).clamp(0.0, 1.0);
            let seam_lat = right_lat + (left_lat - right_lat) * t;

            let mut boundary = Vec::with_capacity(ring.len() + 2);
            boundary.push((-PI, seam_lat));
            boundary.extend(
                ring.into_iter()
                    .filter(|(lon, _)| *lon > -PI + 1e-5 && *lon < PI - 1e-5),
            );
            boundary.push((PI, seam_lat));

            for pair in boundary.windows(2) {
                let (lon0, lat0) = pair[0];
                let (lon1, lat1) = pair[1];
                let bottom_left = Vec3::new(lon0 * radius, lat0 * radius, height);
                let bottom_right = Vec3::new(lon1 * radius, lat1 * radius, height);
                let top_left = Vec3::new(lon0 * radius, pole_y, height);
                let top_right = Vec3::new(lon1 * radius, pole_y, height);
                let area_epsilon = radius * radius * 1e-9;
                if north {
                    push_map_triangle(
                        &mut output,
                        bottom_left,
                        bottom_right,
                        top_right,
                        area_epsilon,
                    );
                    push_map_triangle(&mut output, bottom_left, top_right, top_left, area_epsilon);
                } else {
                    push_map_triangle(&mut output, top_left, top_right, bottom_right, area_epsilon);
                    push_map_triangle(
                        &mut output,
                        top_left,
                        bottom_right,
                        bottom_left,
                        area_epsilon,
                    );
                }
            }
        }
        return output;
    }

    for k in 0..cell.corners.len() {
        let c1 = cell.corners[k];
        let c2 = cell.corners[(k + 1) % cell.corners.len()];
        let ref_lon = cell_reference_longitude(cell.center, &cell.corners);
        let center = project_map_point_unclamped(cell.center, ref_lon, radius, height);
        let triangle = [
            center,
            project_map_point_unclamped(c1, ref_lon, radius, height),
            project_map_point_unclamped(c2, ref_lon, radius, height),
        ];

        // Исходный треугольник плюс копия только с той стороны, где он
        // пересекает антимеридиан. Это сохраняет обе части шва без тройной
        // обработки каждой обычной ячейки.
        let min_x = triangle.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_x = triangle
            .iter()
            .map(|p| p.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let mut shifts = vec![0.0];
        if min_x < -edge {
            shifts.push(TAU);
        }
        if max_x > edge {
            shifts.push(-TAU);
        }

        for shift in shifts {
            let shifted = triangle
                .iter()
                .map(|p| Vec3::new(p.x + shift * radius, p.y, p.z))
                .collect::<Vec<_>>();
            let clipped = clip_polygon_x(shifted, -edge, true);
            let clipped = clip_polygon_x(clipped, edge, false);
            if clipped.len() < 3 {
                continue;
            }
            for i in 1..clipped.len() - 1 {
                let a = clipped[0];
                let b = clipped[i];
                let c = clipped[i + 1];
                push_map_triangle(&mut output, a, b, c, radius * radius * 1e-9);
            }
        }
    }
    output
}

pub fn build_globe_mesh(
    topology: &WorldTopology,
    elevations: &[f32],
    is_ocean: &[bool],
    flatten_water: bool,
    is_flat_map: bool,
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

            for (local_id, cell) in cells.iter().enumerate() {
                let cell_id = first_cell + local_id;
                let Some(&real_h) = elevations.get(cell_id) else {
                    continue;
                };
                let top_color = cell_colors.get(cell_id).copied().unwrap_or([1.0; 4]);
                let cell_ocean = is_ocean.get(cell_id).copied().unwrap_or(real_h <= 0.0);
                let render_h = if flatten_water && cell_ocean {
                    real_h.max(0.0)
                } else {
                    real_h
                };
                let top_height = render_h * height_scale;
                let top_radius = radius + top_height;
                let top_normal = cell.center.to_array();
                let base_idx = chunk.positions.len() as u32;

                if is_flat_map {
                    let top_positions = project_cell_top_map_triangles(cell, radius, top_height);
                    for position in &top_positions {
                        chunk.positions.push(position.to_array());
                        // Сохраняем сферическую нормаль: она содержит широту/долготу
                        // и нужна для полуденного/ночного терминатора на карте.
                        chunk.normals.push(top_normal);
                        chunk.colors.push(top_color);
                    }
                    for local in (0..top_positions.len() as u32).step_by(3) {
                        chunk.indices.extend_from_slice(&[
                            base_idx + local,
                            base_idx + local + 1,
                            base_idx + local + 2,
                        ]);
                    }
                } else {
                    let center_pos = cell.center * top_radius;
                    chunk.positions.push(center_pos.to_array());
                    chunk.normals.push(top_normal);
                    chunk.colors.push(top_color);
                    for &corner in &cell.corners {
                        chunk.positions.push((corner * top_radius).to_array());
                        chunk.normals.push(top_normal);
                        chunk.colors.push(top_color);
                    }
                    for k in 0..cell.corners.len() as u32 {
                        let next = (k + 1) % cell.corners.len() as u32;
                        chunk.indices.extend_from_slice(&[
                            base_idx,
                            base_idx + 1 + k,
                            base_idx + 1 + next,
                        ]);
                    }
                }

                let wall_color = [
                    top_color[0] * 0.75,
                    top_color[1] * 0.75,
                    top_color[2] * 0.75,
                    top_color[3],
                ];
                for k in 0..cell.corners.len() {
                    let Some(&neighbor_h) =
                        cell.neighbors.get(k).and_then(|id| elevations.get(*id))
                    else {
                        continue;
                    };
                    if real_h <= neighbor_h + WALL_THRESHOLD {
                        continue;
                    }
                    let neighbor_id = cell.neighbors[k];
                    let neighbor_ocean = is_ocean
                        .get(neighbor_id)
                        .copied()
                        .unwrap_or(neighbor_h <= 0.0);
                    let render_neighbor_h = if flatten_water && neighbor_ocean {
                        neighbor_h.max(0.0)
                    } else {
                        neighbor_h
                    };
                    let bottom_height = render_neighbor_h * height_scale;
                    let bottom_radius = radius + bottom_height;
                    let c1 = cell.corners[k];
                    let c2 = cell.corners[(k + 1) % cell.corners.len()];
                    let (top_left, top_right, bottom_left, bottom_right) = if is_flat_map {
                        let Some((edge_left, edge_right)) = project_map_wall_edge(c1, c2) else {
                            continue;
                        };
                        (
                            Vec3::new(edge_left.x * radius, edge_left.y * radius, top_height),
                            Vec3::new(edge_right.x * radius, edge_right.y * radius, top_height),
                            Vec3::new(edge_left.x * radius, edge_left.y * radius, bottom_height),
                            Vec3::new(edge_right.x * radius, edge_right.y * radius, bottom_height),
                        )
                    } else {
                        (
                            c1 * top_radius,
                            c2 * top_radius,
                            c1 * bottom_radius,
                            c2 * bottom_radius,
                        )
                    };
                    let wall_normal = (bottom_left - top_left)
                        .cross(top_right - top_left)
                        .normalize_or_zero()
                        .to_array();
                    let wall_base = chunk.positions.len() as u32;
                    chunk.positions.extend_from_slice(&[
                        top_left.to_array(),
                        top_right.to_array(),
                        bottom_left.to_array(),
                        bottom_right.to_array(),
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
            chunk
        })
        .collect();

    let mut positions = Vec::with_capacity(vertex_capacity);
    let mut normals = Vec::with_capacity(vertex_capacity);
    let mut colors = Vec::with_capacity(vertex_capacity);
    let mut indices = Vec::with_capacity(index_capacity);
    for chunk in chunks {
        let offset = positions.len() as u32;
        positions.extend(chunk.positions);
        normals.extend(chunk.normals);
        colors.extend(chunk.colors);
        indices.extend(chunk.indices.into_iter().map(|i| i + offset));
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

/// Кэш структуры меша: offsets и битовая маска стенок по каждой клетке.
pub struct MeshColorLayout {
    pub offsets: Vec<usize>,
    pub wall_masks: Vec<u32>,
    pub top_vertex_counts: Vec<usize>,
}

pub fn compute_color_layout(
    topology: &WorldTopology,
    elevations: &[f32],
    is_flat_map: bool,
) -> MeshColorLayout {
    let mut offsets = Vec::with_capacity(topology.cells.len() + 1);
    let mut wall_masks = Vec::with_capacity(topology.cells.len());
    let mut top_vertex_counts = Vec::with_capacity(topology.cells.len());
    offsets.push(0);
    let mut total = 0usize;

    for (cell_id, cell) in topology.cells.iter().enumerate() {
        let height = elevations.get(cell_id).copied().unwrap_or(0.0);
        let mut mask = 0u32;
        let mut wall_count = 0usize;
        for (k, neighbor_id) in cell.neighbors.iter().enumerate() {
            let Some(&neighbor_h) = elevations.get(*neighbor_id) else {
                continue;
            };
            if height <= neighbor_h + WALL_THRESHOLD {
                continue;
            }
            let wall_is_visible = !is_flat_map
                || project_map_wall_edge(
                    cell.corners[k],
                    cell.corners[(k + 1) % cell.corners.len()],
                )
                .is_some();
            if wall_is_visible {
                wall_count += 1;
                if k < u32::BITS as usize {
                    mask |= 1u32 << k;
                }
            }
        }
        wall_masks.push(mask);
        let top_vertices = if is_flat_map {
            project_cell_top_map_triangles(cell, 1.0, 0.0).len()
        } else {
            1 + cell.corners.len()
        };
        top_vertex_counts.push(top_vertices);
        total += top_vertices + wall_count * 4;
        offsets.push(total);
    }
    MeshColorLayout {
        offsets,
        wall_masks,
        top_vertex_counts,
    }
}

pub fn recolor_globe_mesh_range(
    mesh: &mut Mesh,
    topology: &WorldTopology,
    cell_colors: &[[f32; 4]],
    layout: &MeshColorLayout,
    start_cell: usize,
    end_cell: usize,
    dirty_mask: Option<&[bool]>,
) {
    let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR)
    else {
        return;
    };
    let cells = &topology.cells;
    if start_cell >= end_cell || end_cell > cells.len() || layout.offsets.len() != cells.len() + 1 {
        return;
    }
    if dirty_mask.is_some_and(|mask| mask.len() < end_cell) {
        return;
    }
    let vertex_start = layout.offsets[start_cell];
    let vertex_end = layout.offsets[end_cell];
    if vertex_start > vertex_end || vertex_end > colors.len() {
        return;
    }

    const CELLS_PER_TASK: usize = 1024;
    let color_slice = &mut colors[vertex_start..vertex_end];
    let mut chunks = Vec::with_capacity((end_cell - start_cell).div_ceil(CELLS_PER_TASK));
    let mut remaining = color_slice;
    for chunk_start in (start_cell..end_cell).step_by(CELLS_PER_TASK) {
        let chunk_end = (chunk_start + CELLS_PER_TASK).min(end_cell);
        let local_start = layout.offsets[chunk_start] - vertex_start;
        let local_end = layout.offsets[chunk_end] - vertex_start;
        let (chunk_colors, rest) = remaining.split_at_mut(local_end - local_start);
        chunks.push((chunk_start, chunk_end, local_start, chunk_colors));
        remaining = rest;
    }

    chunks
        .into_par_iter()
        .for_each(|(chunk_start, chunk_end, chunk_offset, chunk_colors)| {
            for cell_id in chunk_start..chunk_end {
                if dirty_mask.is_some_and(|mask| !mask[cell_id]) {
                    continue;
                }
                let cell = &cells[cell_id];
                let local_start = layout.offsets[cell_id] - vertex_start - chunk_offset;
                let len = layout.offsets[cell_id + 1] - layout.offsets[cell_id];
                let cell_colors_out = &mut chunk_colors[local_start..local_start + len];
                let top_color = cell_colors.get(cell_id).copied().unwrap_or([1.0; 4]);
                let top_vertices = layout.top_vertex_counts[cell_id];
                if top_vertices > cell_colors_out.len() {
                    continue;
                }
                cell_colors_out[..top_vertices].fill(top_color);
                let wall_color = [
                    top_color[0] * 0.75,
                    top_color[1] * 0.75,
                    top_color[2] * 0.75,
                    top_color[3],
                ];
                let mask = layout.wall_masks[cell_id];
                let mut offset = top_vertices;
                for k in 0..cell.corners.len() {
                    if k < u32::BITS as usize && (mask & (1u32 << k)) != 0 {
                        if offset + 4 > cell_colors_out.len() {
                            break;
                        }
                        cell_colors_out[offset..offset + 4].fill(wall_color);
                        offset += 4;
                    }
                }
            }
        });
}

pub fn reposition_globe_mesh(
    topology: &WorldTopology,
    real_elevations: &[f32],
    is_ocean: &[bool],
    flatten_water: bool,
    is_flat_map: bool,
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
            let mut out = Vec::new();
            for (local_id, cell) in cells.iter().enumerate() {
                let cell_id = first_cell + local_id;
                let Some(&real_h) = real_elevations.get(cell_id) else {
                    continue;
                };
                let ocean = is_ocean.get(cell_id).copied().unwrap_or(real_h <= 0.0);
                let render_h = if flatten_water && ocean {
                    real_h.max(0.0)
                } else {
                    real_h
                };
                let top_height = render_h * height_scale;
                let top_radius = radius + top_height;
                if is_flat_map {
                    out.extend(
                        project_cell_top_map_triangles(cell, radius, top_height)
                            .into_iter()
                            .map(|position| position.to_array()),
                    );
                } else {
                    out.push((cell.center * top_radius).to_array());
                    for &corner in &cell.corners {
                        out.push((corner * top_radius).to_array());
                    }
                }

                for k in 0..cell.corners.len() {
                    let Some(&neighbor_h) = cell
                        .neighbors
                        .get(k)
                        .and_then(|id| real_elevations.get(*id))
                    else {
                        continue;
                    };
                    if real_h <= neighbor_h + WALL_THRESHOLD {
                        continue;
                    }
                    let neighbor_id = cell.neighbors[k];
                    let neighbor_ocean = is_ocean
                        .get(neighbor_id)
                        .copied()
                        .unwrap_or(neighbor_h <= 0.0);
                    let render_neighbor_h = if flatten_water && neighbor_ocean {
                        neighbor_h.max(0.0)
                    } else {
                        neighbor_h
                    };
                    let bottom_height = render_neighbor_h * height_scale;
                    let bottom_radius = radius + bottom_height;
                    let c1 = cell.corners[k];
                    let c2 = cell.corners[(k + 1) % cell.corners.len()];
                    if is_flat_map {
                        let Some((edge_left, edge_right)) = project_map_wall_edge(c1, c2) else {
                            continue;
                        };
                        out.extend_from_slice(&[
                            [edge_left.x * radius, edge_left.y * radius, top_height],
                            [edge_right.x * radius, edge_right.y * radius, top_height],
                            [edge_left.x * radius, edge_left.y * radius, bottom_height],
                            [edge_right.x * radius, edge_right.y * radius, bottom_height],
                        ]);
                    } else {
                        out.extend_from_slice(&[
                            (c1 * top_radius).to_array(),
                            (c2 * top_radius).to_array(),
                            (c1 * bottom_radius).to_array(),
                            (c2 * bottom_radius).to_array(),
                        ]);
                    }
                }
            }
            out
        })
        .collect();
    let mut positions = Vec::with_capacity(vertex_capacity);
    for chunk in chunks {
        positions.extend(chunk);
    }
    positions
}

#[derive(Clone, Copy)]
struct ProjectedRiverVertex {
    position: Vec3,
    normal: Vec3,
    color: Vec4,
}

impl ProjectedRiverVertex {
    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            position: self.position.lerp(other.position, t),
            normal: self.normal.lerp(other.normal, t).normalize_or_zero(),
            color: self.color.lerp(other.color, t),
        }
    }
}

fn clip_river_polygon_x(
    polygon: Vec<ProjectedRiverVertex>,
    bound: f32,
    keep_greater: bool,
) -> Vec<ProjectedRiverVertex> {
    if polygon.is_empty() {
        return polygon;
    }
    let inside = |p: ProjectedRiverVertex| {
        if keep_greater {
            p.position.x >= bound
        } else {
            p.position.x <= bound
        }
    };
    let mut output = Vec::with_capacity(polygon.len() + 1);
    let mut previous = *polygon.last().unwrap();
    let mut previous_inside = inside(previous);
    for current in polygon {
        let current_inside = inside(current);
        if current_inside != previous_inside {
            let dx = current.position.x - previous.position.x;
            if dx.abs() > 1e-8 {
                let t = ((bound - previous.position.x) / dx).clamp(0.0, 1.0);
                let mut intersection = previous.lerp(current, t);
                intersection.position.x = bound;
                output.push(intersection);
            }
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_inside = current_inside;
    }
    output
}

/// Проецирует речные треугольники и разрезает их по границе карты ±180°.
pub fn project_river_mesh_to_map3d(mesh: &mut Mesh, radius: f32) {
    let source_positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
        _ => return,
    };
    if source_positions
        .iter()
        .all(|position| Vec3::from_array(*position).length_squared() < 1e-12)
    {
        return;
    }
    let source_normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
        _ => vec![[0.0, 0.0, 1.0]; source_positions.len()],
    };
    let source_colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(values)) => values.clone(),
        _ => vec![[1.0; 4]; source_positions.len()],
    };
    let source_indices: Vec<u32> = match mesh.indices() {
        Some(Indices::U16(values)) => values.iter().map(|&i| i as u32).collect(),
        Some(Indices::U32(values)) => values.clone(),
        None => return,
    };

    let mut positions = Vec::with_capacity(source_indices.len());
    let mut normals = Vec::with_capacity(source_indices.len());
    let mut colors = Vec::with_capacity(source_indices.len());
    let mut indices = Vec::with_capacity(source_indices.len());

    let edge = std::f32::consts::PI * radius;
    for triangle in source_indices.chunks_exact(3) {
        let reference_lon = triangle
            .iter()
            .filter_map(|&source_index| source_positions.get(source_index as usize))
            .map(|raw| Vec3::from_array(*raw).normalize_or_zero())
            .find(|direction| direction.x * direction.x + direction.z * direction.z >= 1e-8)
            .map(|direction| direction.x.atan2(direction.z))
            .unwrap_or(0.0);
        let mut projected = Vec::with_capacity(3);

        for &source_index in triangle {
            let index = source_index as usize;
            let Some(&raw) = source_positions.get(index) else {
                projected.clear();
                break;
            };
            let point = Vec3::from_array(raw);
            let direction = point.normalize_or_zero();
            let height = point.length() - radius;
            let source_normal = source_normals
                .get(index)
                .copied()
                .unwrap_or([0.0, 0.0, 1.0]);
            let source_color = source_colors.get(index).copied().unwrap_or([1.0; 4]);
            projected.push(ProjectedRiverVertex {
                position: project_map_point_unclamped(direction, reference_lon, radius, height),
                normal: Vec3::from_array(source_normal),
                color: Vec4::from_array(source_color),
            });
        }
        if projected.len() != 3 {
            continue;
        }

        let min_x = projected
            .iter()
            .map(|p| p.position.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = projected
            .iter()
            .map(|p| p.position.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let mut shifts = vec![0.0];
        if min_x < -edge {
            shifts.push(std::f32::consts::TAU);
        }
        if max_x > edge {
            shifts.push(-std::f32::consts::TAU);
        }

        for shift in shifts {
            let shifted = projected
                .iter()
                .copied()
                .map(|mut vertex| {
                    vertex.position.x += shift * radius;
                    vertex
                })
                .collect::<Vec<_>>();
            let clipped = clip_river_polygon_x(shifted, -edge, true);
            let clipped = clip_river_polygon_x(clipped, edge, false);
            if clipped.len() < 3 {
                continue;
            }
            for i in 1..clipped.len() - 1 {
                let triangle_vertices = [clipped[0], clipped[i], clipped[i + 1]];
                let a = triangle_vertices[0].position;
                let b = triangle_vertices[1].position;
                let c = triangle_vertices[2].position;
                let area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if area2.abs() <= radius * radius * 1e-9 {
                    continue;
                }
                let mut triangle_vertices = triangle_vertices;
                if area2 < 0.0 {
                    triangle_vertices.swap(1, 2);
                }
                let base = positions.len() as u32;
                for vertex in triangle_vertices {
                    positions.push(vertex.position.to_array());
                    normals.push(vertex.normal.to_array());
                    colors.push(vertex.color.to_array());
                }
                indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
        }
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
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
    use rayon::prelude::*;

    let elevations = &geology.elevations;
    let flows = &hydrology.flow_accumulation;
    let flow_dirs = &hydrology.flow_dir;
    let threshold = config.river_threshold.max(1e-6);

    if flows.len() != topology.cells.len() {
        return empty_river_mesh();
    }

    let surface_lift = 0.0014 * radius;
    let river_color = Color::srgb(0.10, 0.48, 0.88).to_linear().to_f32_array();
    let waterfall_color = Color::srgb(0.45, 0.78, 0.98).to_linear().to_f32_array();

    const CELLS_PER_CHUNK: usize = 512;

    let chunks: Vec<RiverMeshChunk> = topology
        .cells
        .par_chunks(CELLS_PER_CHUNK)
        .enumerate()
        .map(|(chunk_idx, cells)| {
            let first_cell = chunk_idx * CELLS_PER_CHUNK;
            let mut chunk = RiverMeshChunk::with_capacity(cells.len());
            let mut inlets: Vec<(usize, f32)> = Vec::with_capacity(6);

            for (local_id, cell) in cells.iter().enumerate() {
                let cell_id = first_cell + local_id;

                // === ТЕЛО ЦИКЛА: копия из старой версии, но с заменой Vec → chunk ===
                let real_h = elevations[cell_id];
                let my_flow = flows[cell_id];
                let cell_is_ocean = geology
                    .is_ocean
                    .get(cell_id)
                    .copied()
                    .unwrap_or(real_h <= 0.0);
                let cell_is_water = hydrology.is_water_cell(geology, cell_id);
                if cell_is_water || my_flow < threshold {
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

                let render_h = if flatten_water && cell_is_ocean {
                    real_h.max(0.0)
                } else {
                    real_h
                };
                let r_top = radius + render_h * height_scale;
                let up_normal = cell.center;
                let lift_vec = up_normal * surface_lift;

                let p_center = cell.center * r_top + lift_vec;
                let out_c1 = cell.corners[out_k];
                let out_c2 = cell.corners[(out_k + 1) % n_corners];
                let p_out_edge = (out_c1 + out_c2) * (0.5 * r_top) + lift_vec;

                let hex_scale = (p_out_edge - p_center).length();
                let my_erod = geology.lithology[cell_id].erodibility();

                // Локальная копия closure `calc_half_width`
                let w_out = calc_half_width(my_flow, hex_scale, my_erod, threshold, config);

                inlets.clear();
                for (k, &nbr_id) in cell.neighbors.iter().enumerate() {
                    let nbr_is_ocean = geology
                        .is_ocean
                        .get(nbr_id)
                        .copied()
                        .unwrap_or(elevations[nbr_id] <= 0.0);
                    if k == out_k || nbr_is_ocean || flows[nbr_id] < threshold {
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
                    push_flat_ribbon(
                        &mut chunk,
                        p_center,
                        p_out_edge,
                        w_out * 0.35,
                        w_out,
                        up_normal,
                        river_color,
                    );
                } else if inlets.len() == 1 {
                    let (in_k, in_flow) = inlets[0];
                    let in_c1 = cell.corners[in_k];
                    let in_c2 = cell.corners[(in_k + 1) % n_corners];
                    let p_in_edge = (in_c1 + in_c2) * (0.5 * r_top) + lift_vec;
                    let w_in = calc_half_width(in_flow, hex_scale, my_erod, threshold, config);

                    let steps = 4;
                    let mut prev_p = p_in_edge;
                    let mut prev_w = w_in;

                    for s in 1..=steps {
                        let t = s as f32 / steps as f32;
                        let omt = 1.0 - t;
                        let next_p =
                            omt * omt * p_in_edge + 2.0 * omt * t * p_center + t * t * p_out_edge;
                        let next_w = w_in * omt + w_out * t;

                        push_flat_ribbon(
                            &mut chunk,
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
                    for &(in_k, in_flow) in &inlets {
                        let in_c1 = cell.corners[in_k];
                        let in_c2 = cell.corners[(in_k + 1) % n_corners];
                        let p_in_edge = (in_c1 + in_c2) * (0.5 * r_top) + lift_vec;
                        let w_in = calc_half_width(in_flow, hex_scale, my_erod, threshold, config);

                        push_flat_ribbon(
                            &mut chunk,
                            p_in_edge,
                            p_center,
                            w_in,
                            w_out * 0.90,
                            up_normal,
                            river_color,
                        );
                    }

                    push_center_hub(&mut chunk, p_center, w_out * 0.92, up_normal, river_color);

                    push_flat_ribbon(
                        &mut chunk,
                        p_center,
                        p_out_edge,
                        w_out * 0.92,
                        w_out,
                        up_normal,
                        river_color,
                    );
                }

                // --- Водопад ---
                let out_nbr_id = cell.neighbors[out_k];
                let real_nbr_h = elevations[out_nbr_id];
                let out_nbr_ocean = geology
                    .is_ocean
                    .get(out_nbr_id)
                    .copied()
                    .unwrap_or(real_nbr_h <= 0.0);
                if real_h > real_nbr_h {
                    let render_nbr_h = if flatten_water && out_nbr_ocean {
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
                        let bot_mid =
                            edge_mid_unit * r_bot + up_normal * surface_lift + wall_offset;

                        let side = edge_dir * w_out;
                        let v_top_left = (top_mid - side).to_array();
                        let v_top_right = (top_mid + side).to_array();
                        let v_bot_left = (bot_mid - side).to_array();
                        let v_bot_right = (bot_mid + side).to_array();

                        let base = chunk.positions.len() as u32;
                        let wn = wall_normal.to_array();
                        chunk.positions.extend_from_slice(&[
                            v_top_left,
                            v_top_right,
                            v_bot_left,
                            v_bot_right,
                        ]);
                        chunk.normals.extend_from_slice(&[wn; 4]);
                        chunk.colors.extend_from_slice(&[
                            river_color,
                            river_color,
                            waterfall_color,
                            waterfall_color,
                        ]);
                        chunk.indices.extend_from_slice(&[
                            base,
                            base + 2,
                            base + 1,
                            base + 1,
                            base + 2,
                            base + 3,
                        ]);
                    }
                }
                // === КОНЕЦ ТЕЛА ЦИКЛА ===
            }

            chunk
        })
        .collect();

    // Последовательное слияние чанков (тривиально, ~1-2 мс)
    let mut merged = RiverMeshChunk::with_capacity(0);
    for chunk in chunks {
        merged.merge(chunk);
    }

    if merged.positions.is_empty() {
        return empty_river_mesh();
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, merged.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, merged.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, merged.colors);
    mesh.insert_indices(Indices::U32(merged.indices));
    mesh
}

/// Хелпер для возврата пустого меша (используется в двух местах выше).
fn empty_river_mesh() -> Mesh {
    let mut empty = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    empty.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0_f32, 0.0, 0.0]; 3]);
    empty.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0_f32, 1.0, 0.0]; 3]);
    empty.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0_f32; 4]; 3]);
    empty.insert_indices(Indices::U32(vec![0, 1, 2]));
    empty
}

/// Вынесено из замыкания в свободную функцию, чтобы использовать
/// в параллельном цикле без захвата.
#[inline]
fn calc_half_width(
    flow: f32,
    hex_scale: f32,
    erodibility: f32,
    threshold: f32,
    config: &HydrologyConfig,
) -> f32 {
    let ratio = (flow / threshold).max(1.0);
    let norm_w = (0.045 * ratio.powf(0.38) * config.river_width_scale).clamp(0.03, 0.28);
    let rock_mod = 0.80 + 0.35 * erodibility;
    hex_scale * norm_w * rock_mod
}

// Небольшой подъем ленты над плоской крышей гекса, чтобы исключить Z-fighting
fn push_flat_ribbon(
    chunk: &mut RiverMeshChunk,
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

    let base = chunk.positions.len() as u32;
    let n_arr = up_normal.to_array();

    chunk
        .positions
        .extend_from_slice(&[v_a_left, v_a_right, v_b_left, v_b_right]);
    chunk.normals.extend_from_slice(&[n_arr; 4]);
    chunk.colors.extend_from_slice(&[color; 4]);

    chunk
        .indices
        .extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
}

fn push_center_hub(
    chunk: &mut RiverMeshChunk,
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

    let base = chunk.positions.len() as u32;
    let n_arr = up_normal.to_array();

    chunk.positions.push(center.to_array());
    chunk.normals.push(n_arr);
    chunk.colors.push(color);

    const N_SIDES: u32 = 6;
    for k in 0..N_SIDES {
        let angle = (k as f32) * std::f32::consts::TAU / (N_SIDES as f32);
        let pt = center + (t1 * angle.cos() + t2 * angle.sin()) * radius;
        chunk.positions.push(pt.to_array());
        chunk.normals.push(n_arr);
        chunk.colors.push(color);
    }

    for k in 0..N_SIDES {
        let next_k = (k + 1) % N_SIDES;
        chunk
            .indices
            .extend_from_slice(&[base, base + 1 + k, base + 1 + next_k]);
    }
}
