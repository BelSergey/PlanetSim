// src/domains/tectonics/solver/mod.rs

pub mod advection;
pub mod boundaries;
pub mod forces;
pub mod init;
pub mod isostasy;
pub mod lithology;
pub mod ocean;

pub use init::{generate_hotspots, generate_plates, generate_plumes, init_tectonics};
pub use isostasy::step_elevation_diffusion;
pub use lithology::finalize_tectonics;

use super::{tectonics_config::TectonicsConfig, tectonics_data::GeologyLayer};
use crate::{domains::generator::GenerationLogger, world::topology::WorldTopology};
use bevy::math::Vec3;
use noise::{Fbm, NoiseFn, Perlin};
use std::f32::consts::PI;
use std::sync::{Arc, Mutex, OnceLock};

#[inline]
pub(crate) fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Предвычисленный геометрический кэш сферы (CSR-формат, готовый для передачи в GPU)
pub struct TectonicsGeometryCache {
    pub num_cells: usize,
    /// Среднее угловое расстояние между центрами соседних клеток в радианах: `sqrt(4*PI / N)`
    pub avg_cell_rad: f32,
    /// Диапазон индексов соседей клетки `i`: `edge_offsets[i] .. edge_offsets[i + 1]`
    pub edge_offsets: Vec<usize>,
    /// Индексы соседних клеток
    pub edge_neighbors: Vec<usize>,
    /// Точное угловое расстояние между клеткой и соседом в радианах
    pub edge_dists_rad: Vec<f32>,
    /// Единичный геодезический вектор в касательной плоскости сферы из центра клетки `i` к центру соседа `j`
    pub edge_tangent_dirs: Vec<Vec3>,
}

static GEOMETRY_CACHE: OnceLock<Mutex<Option<Arc<TectonicsGeometryCache>>>> = OnceLock::new();

impl TectonicsGeometryCache {
    pub fn get_or_build(topology: &WorldTopology) -> Arc<Self> {
        let mutex = GEOMETRY_CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(guard) = mutex.lock()
            && let Some(cached) = guard.as_ref()
            && cached.num_cells == topology.cells.len()
        {
            return Arc::clone(cached);
        }
        let built = Arc::new(Self::build(topology));
        if let Ok(mut guard) = mutex.lock() {
            *guard = Some(Arc::clone(&built));
        }
        built
    }

    pub fn build(topology: &WorldTopology) -> Self {
        let num_cells = topology.cells.len();
        let avg_cell_rad = (4.0 * PI / (num_cells.max(1) as f32)).sqrt();

        let total_edges: usize = topology.cells.iter().map(|c| c.neighbors.len()).sum();
        let mut edge_offsets = Vec::with_capacity(num_cells + 1);
        let mut edge_neighbors = Vec::with_capacity(total_edges);
        let mut edge_dists_rad = Vec::with_capacity(total_edges);
        let mut edge_tangent_dirs = Vec::with_capacity(total_edges);

        edge_offsets.push(0);
        for cell in topology.cells.iter() {
            let r_i = cell.center;
            for &n_id in &cell.neighbors {
                let r_j = topology.cells[n_id].center;
                let dot = r_i.dot(r_j).clamp(-1.0, 1.0);
                let dist_rad = dot.acos().max(1e-5);
                // Единичное направление геодезической от клетки i к соседу n_id в касательной плоскости сферы
                let dir_to_neighbor = (r_j - r_i * dot).normalize_or_zero();

                edge_neighbors.push(n_id);
                edge_dists_rad.push(dist_rad);
                edge_tangent_dirs.push(dir_to_neighbor);
            }
            edge_offsets.push(edge_neighbors.len());
        }

        Self {
            num_cells,
            avg_cell_rad,
            edge_offsets,
            edge_neighbors,
            edge_dists_rad,
            edge_tangent_dirs,
        }
    }
}

/// Рабочие Ping-Pong буферы для безаллокационного выполнения `step_tectonics`
#[derive(Default, Clone)]
pub struct TectonicsScratch {
    pub next_plate_ids: Vec<usize>,
    pub next_thickness: Vec<f32>,
    pub next_density: Vec<f32>,
    pub next_age: Vec<f32>,
    pub next_sediment: Vec<f32>,
    pub next_orogeny: Vec<f32>,
    pub next_shift: Vec<[f32; 3]>,

    pub boundary_convergence: Vec<f32>,
    pub boundary_shear: Vec<f32>,
    pub neighbor_plate: Vec<usize>,
    pub boundary_normal: Vec<Vec3>,
    /// Континентальность коры соседней плиты напротив данной клетки `[0.0 .. 1.0]`
    pub neighbor_continental: Vec<f32>,
    /// Истина, если в данной паре плит текущая плита является нижней (погружающейся в желоб)
    pub is_subducting_slab: Vec<bool>,
    pub temp_elevations: Vec<f32>,
}

impl TectonicsScratch {
    pub fn ensure_size(&mut self, num_cells: usize) {
        self.next_plate_ids.resize(num_cells, 0);
        self.next_thickness.resize(num_cells, 0.0);
        self.next_density.resize(num_cells, 0.0);
        self.next_age.resize(num_cells, 0.0);
        self.next_sediment.resize(num_cells, 0.0);
        self.next_orogeny.resize(num_cells, 0.0);
        self.next_shift.resize(num_cells, [0.0; 3]);
        self.boundary_convergence.resize(num_cells, 0.0);
        self.boundary_shear.resize(num_cells, 0.0);
        self.neighbor_plate.resize(num_cells, usize::MAX);
        self.boundary_normal.resize(num_cells, Vec3::ZERO);
        self.neighbor_continental.resize(num_cells, 0.0);
        self.is_subducting_slab.resize(num_cells, false);
        self.temp_elevations.resize(num_cells, 0.0);
    }
}

/// Подготавливает граничные поля для продолжения тектонической симуляции.
/// `project_plates_and_boundaries` обновляет и `scratch`, и переданную геологию;
/// вызывающая сторона должна передать рабочую копию GeologyLayer, не ECS-ресурс.
pub fn prepare_tectonics_scratch(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
) -> bool {
    let num_cells = topology.cells.len();
    if num_cells == 0 || geology.plates.len() < 3 {
        return false;
    }

    // Do not resize mismatched saved data here: that could silently invent
    // missing crust fields and continue evolution from an inconsistent world.
    let field_lengths = [
        geology.plate_ids.len(),
        geology.elevations.len(),
        geology.crust_thickness.len(),
        geology.crust_density.len(),
        geology.crust_age.len(),
        geology.sediment_thickness.len(),
        geology.tectonic_stress.len(),
        geology.orogeny_root.len(),
        geology.subcell_shift.len(),
        geology.is_ocean.len(),
        geology.continentality.len(),
        geology.volcano_mask.len(),
        geology.lithology.len(),
        geology.terrain_ruggedness.len(),
    ];
    if field_lengths.iter().any(|&len| len != num_cells) {
        return false;
    }

    scratch.ensure_size(num_cells);

    // This projection updates both scratch and geology. The caller passes a
    // worker-owned clone of the ECS resource, so mutating it is intentional.
    boundaries::project_plates_and_boundaries(topology, cache, geology, scratch, config, 0.0);
    true
}

/// Однопроходная функция одного периода тектонического движения (без дублирования проходов):
/// 1. Силы (Slab pull + Ridge push + Collision resistance + Mantle drag + Plumes) -> скорости плит
/// 2. Непрерывная адвекция плит, кратонов, вулканов и полей коры по сфере
/// 3. Единый проход границ, орогенеза, возраста океанической коры и рождения вулканов над плюмами
/// 4. Изостазия и эрозия
pub fn step_tectonics(
    topology: &WorldTopology,
    cache: &TectonicsGeometryCache,
    geology: &mut GeologyLayer,
    scratch: &mut TectonicsScratch,
    config: &TectonicsConfig,
    dt: f32,
) {
    let num_cells = topology.cells.len();
    scratch.ensure_size(num_cells);

    // 1. Обновляем угловые скорости плит по данным границ в `scratch`
    forces::update_plate_velocities(topology, cache, geology, scratch, config, dt);

    // 2. Переносим плиты, вулканы и поля коры по сфере
    advection::advect_crust_and_plates(topology, cache, geology, scratch, config, dt);

    // 3. Единым проходом обновляем границы (и в `geology`, и в `scratch`), возраст коры и плюмы
    boundaries::apply_boundary_and_plume_processes(topology, cache, geology, scratch, config, dt);

    // 4. Пересчитываем рельеф через изостазию и применяем мягкую эрозию
    isostasy::update_isostasy_and_erosion(topology, cache, geology, scratch, config, dt);

    geology.sim_step = geology.sim_step.wrapping_add(1);
}

pub fn simulate_tectonics(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
    logger: &GenerationLogger,
) {
    let cache = TectonicsGeometryCache::get_or_build(topology);
    let mut scratch = TectonicsScratch::default();
    scratch.ensure_size(topology.cells.len());

    init::init_tectonics(topology, &cache, geology, &mut scratch, noise, config);

    let steps = config.simulation_steps;
    let dt = config.time_step;
    logger.start_step(format!(
        "Тектоника: симуляция движения плит и субдукции ({} шагов)...",
        steps
    ));
    for _ in 0..steps {
        step_tectonics(topology, &cache, geology, &mut scratch, config, dt);
    }

    lithology::finalize_tectonics(topology, &cache, geology, noise, config, Some(logger));
}

/// Удобная обертка для вызова ровно одного тика симуляции из UI / реалтайма с обновлением карты
#[allow(unused)]
pub fn step_tectonics_realtime(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
    logger: &GenerationLogger,
) {
    if geology.plates.is_empty() || geology.crust_thickness.len() != topology.cells.len() {
        return;
    }
    let cache = TectonicsGeometryCache::get_or_build(topology);
    let mut scratch = TectonicsScratch::default();
    scratch.ensure_size(topology.cells.len());
    let perlin = Fbm::<Perlin>::new(42);

    // Инициализируем граничные векторы в scratch для расчёта сил в step_tectonics
    boundaries::project_plates_and_boundaries(topology, &cache, geology, &mut scratch, config, 0.0);

    // Выполняем фазы шага без повторного пересчёта project_plates_and_boundaries до адвекции
    let dt = config.time_step;
    forces::update_plate_velocities(topology, &cache, geology, &scratch, config, dt);
    advection::advect_crust_and_plates(topology, &cache, geology, &mut scratch, config, dt);
    boundaries::apply_boundary_and_plume_processes(
        topology,
        &cache,
        geology,
        &mut scratch,
        config,
        dt,
    );
    isostasy::update_isostasy_and_erosion(topology, &cache, geology, &mut scratch, config, dt);
    geology.sim_step = geology.sim_step.wrapping_add(1);

    lithology::finalize_tectonics(topology, &cache, geology, &perlin, config, Some(logger));
}
