// src/domains/generator.rs

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task},
};
use futures_lite::future;
use noise::{Fbm, Perlin};
use rand::{SeedableRng, rngs::StdRng};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    domains::{
        climate::{ClimateConfig, climate_data::ClimateLayer, simulate_all_seasons},
        hydrology::{
            HydrologyConfig, HydrologyLayer, apply_surface_feedback_to_climate, simulate_hydrology,
        },
        tectonics::{
            GeologyLayer, TectonicEvolutionConfig, TectonicsConfig, generate_hotspots,
            generate_plates, simulate_tectonics,
        },
    },
    world::{
        WorldConfig,
        pipeline::{PipelineStage, WorldPipeline},
        states::AppState,
        topology::WorldTopology,
    },
};

/// Запись об одном этапе генерации для отображения в UI
#[derive(Clone)]
pub struct LogEntry {
    pub message: String,
    pub started_at: Instant,
    pub duration_ms: Option<u128>,
}

/// Снимок состояния логгера для безопасной отрисовки в egui без удержания мьютекса
pub struct LogSnapshot {
    pub entries: Vec<LogEntry>,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub total_elapsed_ms: u128,
    pub is_finished: bool,
}

impl LogSnapshot {
    pub fn progress(&self) -> f32 {
        if self.is_finished {
            return 1.0;
        }
        let active_bonus = if self.entries.len() > self.completed_steps {
            0.35
        } else {
            0.0
        };
        ((self.completed_steps as f32 + active_bonus) / self.total_steps.max(1) as f32)
            .clamp(0.0, 0.99)
    }
}

#[derive(Clone)]
pub struct GenerationLogger {
    inner: Arc<Mutex<InnerLog>>,
}

struct InnerLog {
    started_at: Instant,
    finished_at: Option<Instant>,
    total_steps: usize,
    entries: Vec<LogEntry>,
}

impl GenerationLogger {
    pub fn new(total_steps: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerLog {
                started_at: Instant::now(),
                finished_at: None,
                total_steps,
                entries: Vec::with_capacity(total_steps),
            })),
        }
    }

    /// Завершает предыдущий шаг (записывая его длительность) и начинает новый
    pub fn start_step(&self, message: impl Into<String>) {
        let msg = message.into();
        let now = Instant::now();
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(last) = guard.entries.last_mut()
                && last.duration_ms.is_none()
            {
                let dur = now.duration_since(last.started_at).as_millis();
                last.duration_ms = Some(dur);
                info!("  ✔ {} ({} мс)", last.message, dur);
            }
            guard.entries.push(LogEntry {
                message: msg,
                started_at: now,
                duration_ms: None,
            });
        }
    }

    /// Фиксирует завершение последнего шага и всей генерации
    pub fn finish(&self) {
        let now = Instant::now();
        if let Ok(mut guard) = self.inner.lock() {
            if guard.finished_at.is_some() {
                return;
            }
            if let Some(last) = guard.entries.last_mut()
                && last.duration_ms.is_none()
            {
                let dur = now.duration_since(last.started_at).as_millis();
                last.duration_ms = Some(dur);
                info!("  ✔ {} ({} мс)", last.message, dur);
            }
            guard.finished_at = Some(now);
            let total_ms = now.duration_since(guard.started_at).as_millis();
            info!("✅ Генерация мира завершена за {} мс", total_ms);
        }
    }

    /// Сколько миллисекунд прошло с момента завершения задачи (для плавного закрытия окна)
    pub fn finished_elapsed_ms(&self) -> u128 {
        if let Ok(guard) = self.inner.lock()
            && let Some(fin) = guard.finished_at
        {
            return fin.elapsed().as_millis();
        }
        0
    }

    pub fn snapshot(&self) -> LogSnapshot {
        let guard = self.inner.lock().unwrap();
        let completed_steps = guard
            .entries
            .iter()
            .filter(|e| e.duration_ms.is_some())
            .count();
        let total_elapsed_ms = match guard.finished_at {
            Some(fin) => fin.duration_since(guard.started_at).as_millis(),
            None => guard.started_at.elapsed().as_millis(),
        };
        LogSnapshot {
            entries: guard.entries.clone(),
            completed_steps,
            total_steps: guard.total_steps,
            total_elapsed_ms,
            is_finished: guard.finished_at.is_some(),
        }
    }
}

#[derive(Component)]
pub struct GenerationTask {
    pub logger: GenerationLogger,
    pub is_initial: bool,
    pub task: Option<Task<(Option<GeologyLayer>, Option<ClimateLayer>, HydrologyLayer)>>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_generation_task(
    mut commands: Commands,
    mut req: ResMut<GenerationRequest>,
    mut next_state: ResMut<NextState<AppState>>,
    existing_tasks: Query<Entity, With<GenerationTask>>,
    world_config: Res<WorldConfig>,
    topology: Res<WorldTopology>,
    geology_res: Res<GeologyLayer>,
    t_config: Res<TectonicsConfig>,
    t_evolution_config: Res<TectonicEvolutionConfig>, // <-- новое
    c_config: Res<ClimateConfig>,
    climate_res: Res<ClimateLayer>,
    h_config: Res<HydrologyConfig>,
) {
    let Some((stage, evolve)) = req.take() else {
        return;
    };
    next_state.set(AppState::InGame);

    // Если старая задача еще крутилась, отменяем её
    for entity in existing_tasks.iter() {
        commands.entity(entity).despawn();
    }

    let is_initial = geology_res.elevations.is_empty();
    let seed = world_config.seed;

    let thread_pool = AsyncComputeTaskPool::get();
    let topology = (*topology).clone();
    let t_config = (*t_config).clone();
    let c_config = (*c_config).clone();
    let h_config = (*h_config).clone();
    let t_evolution_config = (*t_evolution_config).clone();

    let existing_geology = if stage > PipelineStage::Tectonics || evolve {
        Some((*geology_res).clone())
    } else {
        None
    };

    let existing_climate = if stage > PipelineStage::Climate {
        Some((*climate_res).clone())
    } else {
        None
    };

    let coupled = h_config.coupled_climate_feedback;
    let base_steps = match stage {
        PipelineStage::Tectonics => 6, // 2 (Тектоника) + 3 (Климат) + 1 (Гидрология)
        PipelineStage::Climate => 4,   // 3 (Климат) + 1 (Гидрология)
        PipelineStage::Hydrology => 1, // 1 (Гидрология)
    };
    let total_steps = if coupled { base_steps + 5 } else { base_steps };

    let logger = GenerationLogger::new(total_steps);
    let logger_clone = logger.clone();

    info!(
        "🚀 Старт генерации (сид: {}, стадия: {:?}, обратная связь: {})...",
        seed, stage, coupled
    );

    let task = thread_pool.spawn(async move {
        // Шаг 1–2: Тектоника (если нужна)
        let (geology, is_new_geology) = if evolve {
            // === Эволюция: дрейф плит на месте, без реролла ===
            logger_clone.start_step("Эволюция тектоники: дрейф плит...");
            let mut geo = existing_geology.expect("evolve requires existing geology");

            for plate in &mut geo.plates {
                let angle = plate.angular_vel * t_evolution_config.rotation_per_epoch;
                let rot = Quat::from_axis_angle(plate.euler_pole, angle);
                plate.center_dir = (rot * plate.center_dir).normalize();
            }

            // Пересоздать вулканы по новым положениям плит
            let mut rng = StdRng::seed_from_u64(seed as u64);
            geo.volcanoes = generate_hotspots(&mut rng, &geo.plates, &t_config);

            logger_clone.start_step("Симуляция движения коры и литологии...");
            let perlin = Fbm::<Perlin>::new(seed);
            simulate_tectonics(&topology, &mut geo, &perlin, &t_config);

            (geo, true)
        } else if let Some(geo) = existing_geology {
            (geo, false)
        } else {
            // === Полная регенерация (как раньше) ===
            logger_clone.start_step("Формирование тектонических плит и вулканов...");
            let mut rng = StdRng::seed_from_u64(seed as u64);
            let perlin = Fbm::<Perlin>::new(seed);

            let mut geology = GeologyLayer::default();
            geology.resize(topology.cells.len());
            geology.plates = generate_plates(&mut rng, &t_config);
            geology.volcanoes = generate_hotspots(&mut rng, &geology.plates, &t_config);

            logger_clone.start_step("Симуляция движения коры и литологии...");
            simulate_tectonics(&topology, &mut geology, &perlin, &t_config);

            (geology, true)
        };

        let mut hydrology = HydrologyLayer::default();

        // Шаг 3+: Климат и Гидрология
        let new_climate = if let Some(mut clim) = existing_climate {
            // Заказан пересчет со стадии Hydrology
            logger_clone.start_step("Формирование почв (CLORPT), рек и биомов...");
            simulate_hydrology(&topology, &geology, &clim, &mut hydrology, &h_config, seed);

            if coupled {
                logger_clone.start_step("Обратная связь: влияние биомов на альбедо и влагу...");
                apply_surface_feedback_to_climate(
                    &geology, &mut clim, &hydrology, &c_config, &h_config,
                );
                simulate_all_seasons(
                    &topology,
                    &geology,
                    &mut clim,
                    &c_config,
                    seed,
                    &logger_clone,
                );

                logger_clone.start_step("Финальный пересчет водного баланса и биомов...");
                simulate_hydrology(&topology, &geology, &clim, &mut hydrology, &h_config, seed);
                Some(clim)
            } else {
                // Без обратной связи климат не изменился — возвращать его копию не нужно.
                None
            }
        } else {
            // Проход 1: Первичный климат и первичная гидрология/биомы
            let mut climate = ClimateLayer::default();
            climate.resize(topology.cells.len());
            climate.reset_surface_defaults(
                topology.cells.len(),
                c_config.land_albedo,
                c_config.land_moisture_recycling,
            );
            simulate_all_seasons(
                &topology,
                &geology,
                &mut climate,
                &c_config,
                seed,
                &logger_clone,
            );

            logger_clone.start_step("Формирование почв (CLORPT), рек и биомов...");
            simulate_hydrology(
                &topology,
                &geology,
                &climate,
                &mut hydrology,
                &h_config,
                seed,
            );

            // Проход 2: Обратная связь (Биомы + Реки + Почвы -> Альбедо + Испарение -> Климат -> Гидрология)
            if coupled {
                logger_clone.start_step("Обратная связь: влияние биомов на альбедо и влагу...");
                apply_surface_feedback_to_climate(
                    &geology,
                    &mut climate,
                    &hydrology,
                    &c_config,
                    &h_config,
                );
                simulate_all_seasons(
                    &topology,
                    &geology,
                    &mut climate,
                    &c_config,
                    seed,
                    &logger_clone,
                );

                logger_clone.start_step("Финальный пересчет водного баланса и биомов...");
                simulate_hydrology(
                    &topology,
                    &geology,
                    &climate,
                    &mut hydrology,
                    &h_config,
                    seed,
                );
            }

            Some(climate)
        };

        let new_geology = if is_new_geology { Some(geology) } else { None };

        (new_geology, new_climate, hydrology)
    });

    commands.spawn(GenerationTask {
        logger,
        is_initial,
        task: Some(task),
    });
}

pub fn handle_generation_task(
    mut commands: Commands,
    mut task_query: Query<(Entity, &mut GenerationTask)>,
    mut geology_res: ResMut<GeologyLayer>,
    mut climate_res: ResMut<ClimateLayer>,
    mut hydrology_res: ResMut<HydrologyLayer>,
    mut pipeline: ResMut<WorldPipeline>,
) {
    let Some((entity, mut gen_task)) = task_query.iter_mut().next() else {
        return;
    };

    if let Some(ref mut task) = gen_task.task {
        if let Some((opt_geology, opt_climate, gen_hydrology)) =
            future::block_on(future::poll_once(task))
        {
            if let Some(gen_geology) = opt_geology {
                *geology_res = gen_geology;
                pipeline.rebuild_mesh = true;
            } else {
                pipeline.recolor_mesh = true;
            }
            if let Some(gen_climate) = opt_climate {
                *climate_res = gen_climate;
            }
            *hydrology_res = gen_hydrology;
            pipeline.rebuild_rivers = true;

            // Фиксируем окончание в логгере и освобождаем Task
            gen_task.logger.finish();
            gen_task.task = None;
        }
    } else if gen_task.logger.finished_elapsed_ms() >= 600 {
        // Держим окно лога 0.6 сек после 100% готовности, чтобы пользователь увидел результат
        commands.entity(entity).despawn();
    }
}

#[derive(Resource, Default)]
pub struct GenerationRequest {
    pub stage: Option<PipelineStage>,
    pub evolve_plates: bool,
}

impl GenerationRequest {
    pub fn request(&mut self, stage: PipelineStage) {
        self.stage = Some(stage);
        self.evolve_plates = false;
    }
    pub fn request_evolve(&mut self, stage: PipelineStage) {
        self.stage = Some(stage);
        self.evolve_plates = true;
    }
    pub(crate) fn take(&mut self) -> Option<(PipelineStage, bool)> {
        let stage = self.stage.take()?;
        let evolve = self.evolve_plates;
        self.evolve_plates = false;
        Some((stage, evolve))
    }
}
