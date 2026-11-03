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
        civilization::{CivilizationLayer, prepare_civilization_substrate},
        climate::{ClimateConfig, ClimateLayer, ClimateSolverCache, simulate_all_seasons},
        hydrology::{
            HydrologyConfig, HydrologyLayer, apply_surface_feedback_to_climate, simulate_hydrology,
        },
        tectonics::{
            GeologyLayer, TectonicEvolutionConfig, TectonicsConfig, TectonicsGeometryCache,
            TectonicsScratch, finalize_tectonics, generate_hotspots, generate_plates,
            prepare_tectonics_scratch, simulate_tectonics, step_tectonics,
        },
    },
    world::{
        WorldConfig,
        pipeline::{PipelineStage, WorldPipeline},
        states::AppState,
        topology::WorldTopology,
    },
};

/// Точное количество шагов логгера в каждом домене
const TECTONICS_STEPS: usize = 4;
const CLIMATE_STEPS: usize = 3;
const HYDROLOGY_STEPS: usize = 5;
/// Дополнительные шаги при включенной обратной связи (1 шаг фидбека + Климат + Биосфера)
const COUPLED_EXTRA_STEPS: usize = 1 + CLIMATE_STEPS + HYDROLOGY_STEPS;

/// Запрос фоновой генерации. Эволюция отличается от полной регенерации геологии.
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

    pub fn is_pending(&self) -> bool {
        self.stage.is_some()
    }

    pub(crate) fn take(&mut self) -> Option<(PipelineStage, bool)> {
        let stage = self.stage.take();
        let evolve = std::mem::take(&mut self.evolve_plates);
        stage.map(|stage| (stage, evolve))
    }
}

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
    geology: Res<GeologyLayer>,
    t_config: Res<TectonicsConfig>,
    t_evolution_config: Res<TectonicEvolutionConfig>,
    c_config: Res<ClimateConfig>,
    climate: Res<ClimateLayer>,
    h_config: Res<HydrologyConfig>,
) {
    let Some((stage, evolve_requested)) = req.take() else {
        return;
    };
    let is_initial = geology.elevations.is_empty();
    let evolve = evolve_requested && !is_initial && geology.plates.len() >= 3;
    if evolve_requested && !evolve {
        warn!("Запрос эволюции проигнорирован: существующая геология ещё не готова");
        return;
    }

    next_state.set(AppState::InGame);

    for entity in existing_tasks.iter() {
        commands.entity(entity).despawn();
    }
    let seed = world_config.seed;

    let thread_pool = AsyncComputeTaskPool::get();
    let topology = (*topology).clone();
    let t_config = (*t_config).clone();
    let t_evolution_config = (*t_evolution_config).clone();
    let c_config = (*c_config).clone();
    let h_config = (*h_config).clone();

    let existing_geology = if stage > PipelineStage::Tectonics || evolve {
        Some((*geology).clone())
    } else {
        None
    };

    let existing_climate = if stage > PipelineStage::Climate {
        Some((*climate).clone())
    } else {
        None
    };

    let coupled = h_config.coupled_climate_feedback;
// Feedback в первичной генерации может быть отложен (см. HydrologyConfig).
let should_run_feedback_on_initial =
    coupled && !h_config.defer_initial_feedback;
let tectonics_steps = if evolve {
    TECTONICS_STEPS - 1
} else {
    TECTONICS_STEPS
};
let base_steps = match stage {
    PipelineStage::Tectonics => tectonics_steps + CLIMATE_STEPS + HYDROLOGY_STEPS,
    PipelineStage::Climate => CLIMATE_STEPS + HYDROLOGY_STEPS,
    PipelineStage::Hydrology => HYDROLOGY_STEPS,
};
// total_steps — это только для прогресс-бара. Точность здесь не критична.
let total_steps = if should_run_feedback_on_initial {
    base_steps + COUPLED_EXTRA_STEPS
} else {
    base_steps
};

    let logger = GenerationLogger::new(total_steps);
    let logger_clone = logger.clone();

    info!(
        "🚀 Старт генерации (сид: {}, стадия: {:?}, эволюция: {}, обратная связь: {})...",
        seed, stage, evolve, coupled
    );

    let task = thread_pool.spawn(async move {
        let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);

        // Кэш для климатического солвера: precalc + scratch переиспользуются
        // между первым проходом и feedback-проходом (экономит ~80-150 мс).
        let mut climate_cache = ClimateSolverCache::new();

        // === 1. ТЕКТОНИКА ===
        let (geology_ref, new_geology) = if evolve {
            // Продолжаем существующую геологию новой физической моделью.
            // Не вызываем init_tectonics(): она сбрасывает sim_step и повторно
            // проецирует/инициализирует поля как для новой геологии.
            logger_clone.start_step("Эволюция тектоники: подготовка границ и физические шаги...");
            let mut geo =
                existing_geology.expect("tectonic evolution requires an existing geology layer");
            let cache = TectonicsGeometryCache::get_or_build(&topology);
            let mut scratch = TectonicsScratch::default();

            if prepare_tectonics_scratch(&topology, &cache, &mut geo, &mut scratch, &t_config) {
                let steps = t_evolution_config.steps_per_epoch.clamp(1, 32);
                for _ in 0..steps {
                    step_tectonics(
                        &topology,
                        &cache,
                        &mut geo,
                        &mut scratch,
                        &t_config,
                        t_config.time_step,
                    );
                }

                let perlin = Fbm::<Perlin>::new(seed);
                finalize_tectonics(
                    &topology,
                    &cache,
                    &mut geo,
                    &perlin,
                    &t_config,
                    Some(&logger_clone),
                );
                let geo_ref = geo.clone();
                (geo_ref, Some(geo))
            } else {
                warn!("Эволюция тектоники пропущена: геология не прошла проверку готовности");
                (geo, None)
            }
        } else if let Some(geo) = existing_geology {
            (geo, None)
        } else {
            logger_clone.start_step("Тектоника: генерация плит, кратонов и плюмов...");
            let mut rng = StdRng::seed_from_u64(seed as u64);
            let perlin = Fbm::<Perlin>::new(seed);

            let mut geology = GeologyLayer::default();
            geology.resize(topology.cells.len());
            geology.plates = generate_plates(&mut rng, &t_config);
            geology.volcanoes = generate_hotspots(&mut rng, &geology.plates, &t_config);
            simulate_tectonics(&topology, &mut geology, &perlin, &t_config, &logger_clone);

            let geo_ref = geology.clone();
            (geo_ref, Some(geology))
        };

        let mut hydrology = HydrologyLayer::default();

        // === 2. КЛИМАТ И БИОСФЕРА / ГИДРОЛОГИЯ ===
        let new_climate = if let Some(mut clim) = existing_climate {
            // Заказан пересчет только со стадии Hydrology (5 шагов внутри)
            simulate_hydrology(
                &topology,
                &geology_ref,
                &clim,
                &mut hydrology,
                &h_config,
                seed,
                &logger_clone,
            );

            if coupled {
                logger_clone.start_step("Обратная связь: влияние биомов и озёр на климат...");
                apply_surface_feedback_to_climate(
                    &geology_ref,
                    &mut clim,
                    &hydrology,
                    &c_config,
                    &h_config,
                );
                simulate_all_seasons(
                    &topology,
                    &geology_ref,
                    &mut clim,
                    &c_config,
                    seed,
                    &logger_clone,
                    Some(&mut climate_cache),
                );
                simulate_hydrology(
                    &topology,
                    &geology_ref,
                    &clim,
                    &mut hydrology,
                    &h_config,
                    seed,
                    &logger_clone,
                );
                Some(clim)
            } else {
                None
            }
        } else {
            // Проход 1: Первичный климат (3 шага) + Первичная биосфера (5 шагов)
            let mut climate = ClimateLayer::default();
            climate.resize(topology.cells.len());
            climate.reset_surface_defaults(
                topology.cells.len(),
                c_config.land_albedo,
                c_config.land_moisture_recycling,
            );
            simulate_all_seasons(
                &topology,
                &geology_ref,
                &mut climate,
                &c_config,
                seed,
                &logger_clone,
                Some(&mut climate_cache),
            );

            simulate_hydrology(
                &topology,
                &geology_ref,
                &climate,
                &mut hydrology,
                &h_config,
                seed,
                &logger_clone,
            );

// Проход 2: Обратная связь (1 шаг + 3 шага климата + 5 шагов биосферы)
// Пропускается при defer_initial_feedback = true (по умолчанию).
// Пользователь может запустить кнопкой в Hydrology UI.
if should_run_feedback_on_initial {
    logger_clone.start_step("Обратная связь: влияние биомов и озёр на климат...");
    apply_surface_feedback_to_climate(
        &geology_ref,
        &mut climate,
        &hydrology,
        &c_config,
        &h_config,
    );
    simulate_all_seasons(
        &topology,
        &geology_ref,
        &mut climate,
        &c_config,
        seed,
        &logger_clone,
        Some(&mut climate_cache),
    );
    simulate_hydrology(
        &topology,
        &geology_ref,
        &climate,
        &mut hydrology,
        &h_config,
        seed,
        &logger_clone,
    );
}

            Some(climate)
        };

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
    mut geology: ResMut<GeologyLayer>,
    mut climate: ResMut<ClimateLayer>,
    mut hydrology: ResMut<HydrologyLayer>,
    mut civ: ResMut<CivilizationLayer>,
    mut pipeline: ResMut<WorldPipeline>,
    topology: Res<WorldTopology>,
    world_config: Res<WorldConfig>,
) {
    let Some((entity, mut gen_task)) = task_query.iter_mut().next() else {
        return;
    };

    if let Some(ref mut task) = gen_task.task {
        if let Some((opt_geology, opt_climate, gen_hydrology)) =
            future::block_on(future::poll_once(task))
        {
            if let Some(gen_geology) = opt_geology {
                *geology = gen_geology;
                pipeline.rebuild_mesh = true;
            } else {
                pipeline.recolor_mesh = true;
            }
            if let Some(gen_climate) = opt_climate {
                *climate = gen_climate;
            }
            *hydrology = gen_hydrology;
            pipeline.rebuild_rivers = true;

            prepare_civilization_substrate(
                &topology,
                &geology,
                &climate,
                &hydrology,
                &mut civ,
                world_config.seed,
            );

            gen_task.logger.finish();
            gen_task.task = None;
        }
    } else if gen_task.logger.finished_elapsed_ms() >= 600 {
        commands.entity(entity).despawn();
    }
}
