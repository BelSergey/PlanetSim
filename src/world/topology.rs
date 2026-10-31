// src/world/topology.rs

use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

pub type CellId = usize;

#[derive(Clone)]
pub struct TopologicalCell {
    /// Направление центра ячейки на единичной сфере
    pub center: Vec3,
    /// Географические координаты: x = широта [-PI/2..PI/2], y = долгота [-PI..PI]
    pub lat_lon: Vec2,
    /// Индексы соседних ячеек (отсортированы по кругу)
    pub neighbors: Vec<CellId>,
    /// Направления углов гекса/пентагона на единичной сфере
    pub corners: Vec<Vec3>,
}

#[derive(Resource, Clone)]
pub struct WorldTopology {
    pub subdivisions: u32,
    /// Неизменяемая топология разделяется между ECS и фоновыми задачами без глубокого клонирования.
    pub cells: Arc<[TopologicalCell]>,
}

impl WorldTopology {
    pub fn new(subdivisions: u32) -> Self {
        let (mut vertices, mut faces) = Self::build_base_icosphere();

        for _ in 0..subdivisions {
            faces = Self::subdivide(&mut vertices, &faces);
        }

        let (incident_faces, raw_neighbors, face_centers) =
            Self::calculate_topology(&vertices, &faces);
        let cells = Self::build_cells(&vertices, &incident_faces, &raw_neighbors, &face_centers);

        Self {
            subdivisions,
            cells: Arc::from(cells),
        }
    }

    // pub fn len(&self) -> usize {
    //     self.cells.len()
    // }

    fn build_base_icosphere() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let t = (1.0 + 5.0_f32.sqrt()) / 2.0;
        let vertices = vec![
            Vec3::new(-1.0, t, 0.0).normalize(),
            Vec3::new(1.0, t, 0.0).normalize(),
            Vec3::new(-1.0, -t, 0.0).normalize(),
            Vec3::new(1.0, -t, 0.0).normalize(),
            Vec3::new(0.0, -1.0, t).normalize(),
            Vec3::new(0.0, 1.0, t).normalize(),
            Vec3::new(0.0, -1.0, -t).normalize(),
            Vec3::new(0.0, 1.0, -t).normalize(),
            Vec3::new(t, 0.0, -1.0).normalize(),
            Vec3::new(t, 0.0, 1.0).normalize(),
            Vec3::new(-t, 0.0, -1.0).normalize(),
            Vec3::new(-t, 0.0, 1.0).normalize(),
        ];

        let faces = vec![
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];

        (vertices, faces)
    }

    fn subdivide(vertices: &mut Vec<Vec3>, faces: &[[u32; 3]]) -> Vec<[u32; 3]> {
        let mut mid_cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut new_faces = Vec::with_capacity(faces.len() * 4);

        let mut get_midpoint = |v1: u32, v2: u32, verts: &mut Vec<Vec3>| -> u32 {
            let key = if v1 < v2 { (v1, v2) } else { (v2, v1) };
            if let Some(&idx) = mid_cache.get(&key) {
                return idx;
            }
            let mid = ((verts[v1 as usize] + verts[v2 as usize]) * 0.5).normalize();
            let idx = verts.len() as u32;
            verts.push(mid);
            mid_cache.insert(key, idx);
            idx
        };

        for &[a, b, c] in faces {
            let ab = get_midpoint(a, b, vertices);
            let bc = get_midpoint(b, c, vertices);
            let ca = get_midpoint(c, a, vertices);
            new_faces.push([a, ab, ca]);
            new_faces.push([b, bc, ab]);
            new_faces.push([c, ca, bc]);
            new_faces.push([ab, bc, ca]);
        }
        new_faces
    }

    fn calculate_topology(
        vertices: &[Vec3],
        faces: &[[u32; 3]],
    ) -> (Vec<Vec<usize>>, Vec<Vec<usize>>, Vec<Vec3>) {
        let num_cells = vertices.len();
        let mut raw_neighbors = vec![Vec::with_capacity(6); num_cells];
        let mut incident_faces = vec![Vec::with_capacity(6); num_cells];

        let face_centers: Vec<Vec3> = faces
            .iter()
            .map(|&[a, b, c]| {
                (vertices[a as usize] + vertices[b as usize] + vertices[c as usize]).normalize()
            })
            .collect();

        for (f_idx, &[a, b, c]) in faces.iter().enumerate() {
            let (ua, ub, uc) = (a as usize, b as usize, c as usize);
            incident_faces[ua].push(f_idx);
            incident_faces[ub].push(f_idx);
            incident_faces[uc].push(f_idx);

            for &(u, v) in &[(ua, ub), (ub, uc), (uc, ua)] {
                if !raw_neighbors[u].contains(&v) {
                    raw_neighbors[u].push(v);
                }
                if !raw_neighbors[v].contains(&u) {
                    raw_neighbors[v].push(u);
                }
            }
        }
        (incident_faces, raw_neighbors, face_centers)
    }

    fn build_cells(
        vertices: &[Vec3],
        incident_faces: &[Vec<usize>],
        raw_neighbors: &[Vec<usize>],
        face_centers: &[Vec3],
    ) -> Vec<TopologicalCell> {
        let num_cells = vertices.len();
        let mut cells = Vec::with_capacity(num_cells);

        for i in 0..num_cells {
            let center = vertices[i];
            let lat = center.y.clamp(-1.0, 1.0).asin();
            let lon = center.z.atan2(center.x);

            let up = if center.y.abs() < 0.99 {
                Vec3::Y
            } else {
                Vec3::X
            };
            let tangent = center.cross(up).normalize();
            let bitangent = center.cross(tangent);

            let angle_around_center = |dir: Vec3| -> f32 {
                let d = dir - center;
                d.dot(bitangent).atan2(d.dot(tangent))
            };

            // Угол каждого угла ячейки вычисляем один раз: sort comparator вызывается многократно.
            let mut corners_with_angles: Vec<(f32, Vec3)> = incident_faces[i]
                .iter()
                .map(|&f_idx| {
                    let corner = face_centers[f_idx];
                    (angle_around_center(corner), corner)
                })
                .collect();
            corners_with_angles.sort_by(|a, b| a.0.total_cmp(&b.0));
            let corners: Vec<Vec3> = corners_with_angles
                .into_iter()
                .map(|(_, corner)| corner)
                .collect();

            let n = corners.len();
            let mut ordered_neighbors = Vec::with_capacity(n);
            for k in 0..n {
                let edge_mid = (corners[k] + corners[(k + 1) % n]).normalize();
                let best_neighbor = *raw_neighbors[i]
                    .iter()
                    .max_by(|&&n_a, &&n_b| {
                        vertices[n_a]
                            .dot(edge_mid)
                            .total_cmp(&vertices[n_b].dot(edge_mid))
                    })
                    .unwrap();
                ordered_neighbors.push(best_neighbor);
            }

            cells.push(TopologicalCell {
                center,
                lat_lon: Vec2::new(lat, lon),
                neighbors: ordered_neighbors,
                corners,
            });
        }
        cells
    }
}
