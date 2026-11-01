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
use crate::world::topology::WorldTopology;
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
    /// Единичный касательный вектор на сфере из центра клетки в сторону соседа
    pub edge_tangent_dirs: Vec<Vec3>,
}

static GEOMETRY_CACHE: OnceLock<Mutex<Option<Arc<TectonicsGeometryCache>>>> = OnceLock::new();

impl TectonicsGeometryCache {
    /// Возвращает закэшированную геометрию для текущей сетки или строит её за один проход
    pub fn get_or_build(topology: &WorldTopology) -> Arc<Self> {
        let mutex = GEOMETRY_CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(guard) = mutex.lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.num_cells == topology.cells.len() {
                    return Arc::clone(cached);
                }
            }
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
        for cell in &topology.cells {
            let r_i = cell.center;
            for &n_id in &cell.neighbors {
                let r_j = topology.cells[n_id].center;
                let dot = r_i.dot(r_j).clamp(-1.0, 1.0);
                let dist_rad = dot.acos().max(1e-5);
                // Проекция направления на касательную плоскость сферы в точке r_i
                let tangent = (r_j - r_i * dot).normalize_or_zero();

                edge_neighbors.push(n_id);
                edge_dists_rad.push(dist_rad);
                edge_tangent_dirs.push(tangent);
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

    /// Конвергенция на границе (> 0 сжатие, < 0 растяжение)
    pub boundary_convergence: Vec<f32>,
    /// Касательная сдвиговая скорость вдоль границы (трансформные разломы)
    pub boundary_shear: Vec<f32>,
    /// Соседняя плита через ближайшую границу (`usize::MAX`, если клетка внутри плиты)
    pub neighbor_plate: Vec<usize>,
    /// Единичная нормаль к границе (в сторону соседней плиты)
    pub boundary_normal: Vec<Vec3>,
    /// Временный буфер высот для эрозии/диффузии
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
        self.temp_elevations.resize(num_cells, 0.0);
    }
}

/// Однопроходная функция одного периода тектонического движения (готова для реалтайма):
/// 1. Силы (Slab pull + Ridge push + Collision resistance + Mantle drag + Plumes) -> скорости плит
/// 2. Адвекция коры и вулканов по сфере + баланс поглощения/раскола плит
/// 3. Классификация границ
/// 4. Изменение коры (спрединг, субдукция, коллизия, рифтинг) + рождение вулканов над плюмами
/// 5. Изостазия Эйри и динамическая топография желобов
/// 6. Термическая эрозия и накопление осадков
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

    // 1. Детектируем текущие границы плит и считаем суммарные силы/моменты вращения плит
    boundaries::detect_plate_boundaries(topology, cache, geology, scratch);
    forces::update_plate_velocities(topology, cache, geology, scratch, config, dt);

    // 2. Переносим кору и вулканы по сфере, проверяем поглощение и раскол плит
    advection::advect_crust_and_plates(topology, cache, geology, scratch, config, dt);

    // 3–4. Обновляем границы после сдвига, модифицируем кору (спрединг/субдукция/коллизия) и плюмы
    boundaries::detect_plate_boundaries(topology, cache, geology, scratch);
    boundaries::apply_boundary_and_plume_processes(topology, cache, geology, scratch, config, dt);

    // 5–6. Пересчитываем рельеф через изостазию и применяем эрозию с осадконакоплением
    isostasy::update_isostasy_and_erosion(cache, geology, scratch, config, dt);

    geology.sim_step = geology.sim_step.wrapping_add(1);
}

/// Главная функция генерации тектоники: инициализирует смешанные плиты,
/// в цикле прогоняет `step_tectonics`, а затем накладывает шум, породы, океаны и континентальность
pub fn simulate_tectonics(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    noise: &(impl NoiseFn<f64, 3> + Sync),
    config: &TectonicsConfig,
) {
    let cache = TectonicsGeometryCache::get_or_build(topology);
    let mut scratch = TectonicsScratch::default();

    // 0. Инициализируем начальные смешанные плиты, кратоны и стационарные плюмы
    init::init_tectonics(topology, &cache, geology, noise, config);

    // 1..N. Цикл пошагового движения плит
    let steps = config.simulation_steps;
    let dt = config.time_step;
    for _ in 0..steps {
        step_tectonics(topology, &cache, geology, &mut scratch, config, dt);
    }

    // Финализация: горный и детальный шум, умный поиск океанов, континентальность и породы
    lithology::finalize_tectonics(topology, &cache, geology, noise, config);
}

/// Удобная обертка для вызова ровно одного тика симуляции из UI / реалтайма с обновлением карты
#[allow(unused)]
pub fn step_tectonics_realtime(
    topology: &WorldTopology,
    geology: &mut GeologyLayer,
    config: &TectonicsConfig,
) {
    if geology.plates.is_empty() || geology.crust_thickness.len() != topology.cells.len() {
        return;
    }
    let cache = TectonicsGeometryCache::get_or_build(topology);
    let mut scratch = TectonicsScratch::default();
    let perlin = Fbm::<Perlin>::new(42);

    step_tectonics(
        topology,
        &cache,
        geology,
        &mut scratch,
        config,
        config.time_step,
    );
    lithology::finalize_tectonics(topology, &cache, geology, &perlin, config);
}