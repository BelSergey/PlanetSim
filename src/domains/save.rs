// src/domains/save.rs

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task},
};
use bevy_egui::egui;
use futures_lite::future;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    domains::{
        climate::{ClimateConfig, climate_data::ClimateLayer},
        generator::GenerationRequest,
        hydrology::{HydrologyConfig, HydrologyLayer},
        tectonics::{GeologyLayer, TectonicsConfig},
    },
    world::{
        WorldConfig,
        pipeline::{PipelineStage, WorldPipeline},
        states::AppState,
        topology::WorldTopology,
    },
};

const PLANET_SAVE_DIR: &str = "saves/planets";
const CONFIG_SAVE_DIR: &str = "saves/configs";
const PLANET_FILE_VERSION: u32 = 1;

/// Бинарный контейнер сохранения планеты (НЕ хранит конфиги генерации).
/// Каждый слой упакован в отдельный чанк байтов, чтобы отсутствие или устаревание
/// одного этапа не ломало чтение предыдущих этапов.
#[derive(Serialize, Deserialize)]
pub struct PlanetSaveFile {
    pub version: u32,
    pub radius: f32,
    pub subdivisions: u32,
    pub seed: u32,
    pub geology_bytes: Option<Vec<u8>>,
    pub climate_bytes: Option<Vec<u8>>,
    pub hydrology_bytes: Option<Vec<u8>>,
}

/// Текстовый JSON-пресет настроек генератора (отдельно от планет)
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConfigPresetFile {
    pub tectonics: TectonicsConfig,
    pub climate: ClimateConfig,
    pub hydrology: HydrologyConfig,
}

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub size_kb: u64,
}

#[derive(Clone)]
pub struct MissingStagePrompt {
    pub planet_name: String,
    pub missing_from: PipelineStage,
    pub reason: String,
}

/// Запрос на фоновое сохранение или загрузку планеты
#[derive(Clone)]
pub enum SaveLoadRequest {
    SavePlanet {
        name: String,
        include_climate: bool,
        include_hydrology: bool,
    },
    LoadPlanet {
        name: String,
        path: PathBuf,
    },
}

// === ЛОГГЕР ПРОГРЕССА СОХРАНЕНИЯ / ЗАГРУЗКИ ДЛЯ ОВЕРЛЕЯ ===

#[derive(Clone)]
pub struct SaveLoadLogEntry {
    pub message: String,
    pub started_at: Instant,
    pub duration_ms: Option<u128>,
}

#[derive(Clone)]
pub struct SaveLoadSnapshot {
    pub entries: Vec<SaveLoadLogEntry>,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub total_elapsed_ms: u128,
    pub is_finished: bool,
    pub error: Option<String>,
}

impl SaveLoadSnapshot {
    pub fn progress(&self) -> f32 {
        if self.is_finished {
            return 1.0;
        }
        if self.total_steps == 0 {
            return 0.0;
        }
        (self.completed_steps as f32 / self.total_steps as f32).clamp(0.0, 0.99)
    }
}

struct InnerSaveLoadLog {
    entries: Vec<SaveLoadLogEntry>,
    completed_steps: usize,
    total_steps: usize,
    started_at: Instant,
    finished_elapsed_ms: Option<u128>,
    is_finished: bool,
    error: Option<String>,
}

#[derive(Clone)]
pub struct SaveLoadLogger {
    inner: Arc<Mutex<InnerSaveLoadLog>>,
}

impl SaveLoadLogger {
    pub fn new(total_steps: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerSaveLoadLog {
                entries: Vec::with_capacity(total_steps),
                completed_steps: 0,
                total_steps,
                started_at: Instant::now(),
                finished_elapsed_ms: None,
                is_finished: false,
                error: None,
            })),
        }
    }

    pub fn start_step(&self, msg: impl Into<String>) {
        let msg = msg.into();
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(last) = guard.entries.last_mut() {
                if last.duration_ms.is_none() {
                    let ms = last.started_at.elapsed().as_millis();
                    last.duration_ms = Some(ms);
                    guard.completed_steps += 1;
                }
            }
            guard.entries.push(SaveLoadLogEntry {
                message: msg,
                started_at: Instant::now(),
                duration_ms: None,
            });
        }
    }

    pub fn finish_ok(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(last) = guard.entries.last_mut() {
                if last.duration_ms.is_none() {
                    let ms = last.started_at.elapsed().as_millis();
                    last.duration_ms = Some(ms);
                    guard.completed_steps += 1;
                }
            }
            guard.finished_elapsed_ms = Some(guard.started_at.elapsed().as_millis());
            guard.is_finished = true;
        }
    }

    pub fn finish_err(&self, err: impl Into<String>) {
        let err = err.into();
        if let Ok(mut guard) = self.inner.lock() {
            guard.finished_elapsed_ms = Some(guard.started_at.elapsed().as_millis());
            guard.is_finished = true;
            guard.error = Some(err);
        }
    }

    pub fn snapshot(&self) -> SaveLoadSnapshot {
        let guard = self.inner.lock().unwrap();
        let total_elapsed_ms = guard
            .finished_elapsed_ms
            .unwrap_or_else(|| guard.started_at.elapsed().as_millis());
        SaveLoadSnapshot {
            entries: guard.entries.clone(),
            completed_steps: guard.completed_steps,
            total_steps: guard.total_steps,
            total_elapsed_ms,
            is_finished: guard.is_finished,
            error: guard.error.clone(),
        }
    }
}

pub struct LoadedPlanetPayload {
    pub planet_name: String,
    pub radius: f32,
    pub seed: u32,
    pub new_topology: Option<WorldTopology>,
    pub geology: GeologyLayer,
    pub climate: ClimateLayer,
    pub hydrology: HydrologyLayer,
    pub missing_stage: Option<(PipelineStage, String)>,
}

pub enum SaveLoadOutcome {
    Saved { planet_name: String },
    Loaded(Box<LoadedPlanetPayload>),
}

#[derive(Component)]
pub struct SaveLoadTask {
    pub title: String,
    pub is_saving: bool,
    pub is_initial: bool,
    pub logger: SaveLoadLogger,
    pub task: Option<Task<Result<SaveLoadOutcome, String>>>,
    pub finished_at: Option<Instant>,
}

#[derive(Resource)]
pub struct SaveUiState {
    pub show_planets_window: bool,
    pub show_configs_window: bool,

    pub planet_save_name: String,
    pub save_include_climate: bool,
    pub save_include_hydrology: bool,
    pub auto_regenerate_missing: bool,

    pub config_save_name: String,
    pub load_tectonics_cfg: bool,
    pub load_climate_cfg: bool,
    pub load_hydrology_cfg: bool,

    pub planet_files: Vec<FileEntry>,
    pub config_files: Vec<FileEntry>,

    pub status_message: Option<(String, bool)>, // (текст, is_error)
    pub pending_prompt: Option<MissingStagePrompt>,
    pub pending_request: Option<SaveLoadRequest>,
    pub is_busy: bool,
}

impl Default for SaveUiState {
    fn default() -> Self {
        let mut state = Self {
            show_planets_window: false,
            show_configs_window: false,
            planet_save_name: "planet_01".to_string(),
            save_include_climate: true,
            save_include_hydrology: true,
            auto_regenerate_missing: false,
            config_save_name: "preset_default".to_string(),
            load_tectonics_cfg: true,
            load_climate_cfg: true,
            load_hydrology_cfg: true,
            planet_files: Vec::new(),
            config_files: Vec::new(),
            status_message: None,
            pending_prompt: None,
            pending_request: None,
            is_busy: false,
        };
        state.refresh_lists();
        state
    }
}

impl SaveUiState {
    pub fn refresh_lists(&mut self) {
        self.planet_files = scan_dir(PLANET_SAVE_DIR, "planet");
        self.config_files = scan_dir(CONFIG_SAVE_DIR, "json");
    }

    pub fn set_ok(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        info!("{}", s);
        self.status_message = Some((s, false));
    }

    pub fn set_err(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        warn!("{}", s);
        self.status_message = Some((s, true));
    }
}

fn scan_dir(dir: &str, ext: &str) -> Vec<FileEntry> {
    let _ = fs::create_dir_all(dir);
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unnamed")
                    .to_string();
                let size_kb = entry.metadata().map(|m| m.len() / 1024).unwrap_or(0);
                out.push(FileEntry {
                    name,
                    path,
                    size_kb,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "unnamed".to_string();
    }
    trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn stage_label(stage: PipelineStage) -> &'static str {
    match stage {
        PipelineStage::Tectonics => "Тектоника (с нуля)",
        PipelineStage::Climate => "Климат -> Гидрология",
        PipelineStage::Hydrology => "Гидрология и Биомы",
    }
}

/// Система запуска фоновых задач сохранения и загрузки планет
#[allow(clippy::too_many_arguments)]
pub fn spawn_save_load_task(
    mut commands: Commands,
    mut save_ui: ResMut<SaveUiState>,
    existing_tasks: Query<(Entity, &SaveLoadTask)>,
    world_config: Res<WorldConfig>,
    topology: Res<WorldTopology>,
    geology: Res<GeologyLayer>,
    climate: Res<ClimateLayer>,
    hydrology: Res<HydrologyLayer>,
) {
    // Если есть активная незавершенная задача — ждём её окончания
    for (entity, t) in existing_tasks.iter() {
        if t.task.is_some() {
            return;
        }
        // Если предыдущая задача уже завершилась и просто доигрывает анимацию — убираем её
        if save_ui.pending_request.is_some() {
            commands.entity(entity).despawn();
        }
    }

    let Some(req) = save_ui.pending_request.take() else {
        return;
    };

    let pool = AsyncComputeTaskPool::get();
    let is_initial = geology.elevations.is_empty();

    match req {
        SaveLoadRequest::SavePlanet {
            name,
            include_climate,
            include_hydrology,
        } => {
            let clean_name = sanitize_filename(&name);
            let do_climate = include_climate && !climate.temperatures.is_empty();
            let do_hydrology = include_climate && include_hydrology && !hydrology.biomes.is_empty();

            let total_steps = 2 + usize::from(do_climate) + usize::from(do_hydrology);
            let logger = SaveLoadLogger::new(total_steps);
            let thread_logger = logger.clone();

            let radius = world_config.radius;
            let subdivisions = topology.subdivisions;
            let seed = world_config.seed;

            let geo_clone = (*geology).clone();
            let clim_clone = if do_climate {
                Some((*climate).clone())
            } else {
                None
            };
            let hydro_clone = if do_hydrology {
                Some((*hydrology).clone())
            } else {
                None
            };

            save_ui.is_busy = true;
            let title = format!("Сохранение планеты «{}»", clean_name);

            let task = pool.spawn(async move {
                thread_logger.start_step("Сериализация слоя тектоники и рельефа...");
                let geo_bytes = bincode::serialize(&geo_clone)
                    .map_err(|e| format!("Ошибка упаковки тектоники: {}", e))?;

                let clim_bytes = if let Some(clim) = clim_clone {
                    thread_logger.start_step("Сериализация климата и сезонов...");
                    Some(
                        bincode::serialize(&clim)
                            .map_err(|e| format!("Ошибка упаковки климата: {}", e))?,
                    )
                } else {
                    None
                };

                let hydro_bytes = if let Some(hydro) = hydro_clone {
                    thread_logger.start_step("Сериализация гидрологии и биомов...");
                    Some(
                        bincode::serialize(&hydro)
                            .map_err(|e| format!("Ошибка упаковки гидрологии: {}", e))?,
                    )
                } else {
                    None
                };

                thread_logger.start_step("Запись контейнера .planet на диск...");
                let _ = fs::create_dir_all(PLANET_SAVE_DIR);
                let path = Path::new(PLANET_SAVE_DIR).join(format!("{}.planet", clean_name));

                let save_data = PlanetSaveFile {
                    version: PLANET_FILE_VERSION,
                    radius,
                    subdivisions,
                    seed,
                    geology_bytes: Some(geo_bytes),
                    climate_bytes: clim_bytes,
                    hydrology_bytes: hydro_bytes,
                };

                let file_bytes = bincode::serialize(&save_data)
                    .map_err(|e| format!("Ошибка сборки файла: {}", e))?;
                fs::write(&path, file_bytes)
                    .map_err(|e| format!("Ошибка записи на диск: {}", e))?;

                thread_logger.finish_ok();
                Ok(SaveLoadOutcome::Saved {
                    planet_name: clean_name,
                })
            });

            commands.spawn(SaveLoadTask {
                title,
                is_saving: true,
                is_initial,
                logger,
                task: Some(task),
                finished_at: None,
            });
        }

        SaveLoadRequest::LoadPlanet { name, path } => {
            let current_subdivs = topology.subdivisions;
            let current_num_cells = topology.cells.len();

            let logger = SaveLoadLogger::new(4);
            let thread_logger = logger.clone();

            save_ui.is_busy = true;
            let title = format!("Загрузка планеты «{}»", name);

            let task = pool.spawn(async move {
                thread_logger.start_step("Чтение файла и распаковка контейнера...");
                let bytes =
                    fs::read(&path).map_err(|e| format!("Не удалось прочитать файл: {}", e))?;

                let save_file: PlanetSaveFile = bincode::deserialize(&bytes)
                    .map_err(|e| format!("Поврежденный заголовок сохранения: {}", e))?;

                let (new_topology, num_cells) = if save_file.subdivisions != current_subdivs {
                    thread_logger.start_step(format!(
                        "Построение топологии сферы (subdiv {})...",
                        save_file.subdivisions
                    ));
                    let topo = WorldTopology::new(save_file.subdivisions);
                    let len = topo.cells.len();
                    (Some(topo), len)
                } else {
                    thread_logger.start_step("Проверка геометрии сетки...");
                    (None, current_num_cells)
                };

                let mut missing_stage: Option<(PipelineStage, String)> = None;

                thread_logger.start_step("Распаковка тектоники и рельефа...");
                let geology = match save_file
                    .geology_bytes
                    .as_deref()
                    .map(bincode::deserialize::<GeologyLayer>)
                {
                    Some(Ok(loaded_geo)) if loaded_geo.elevations.len() == num_cells => loaded_geo,
                    Some(Ok(_)) => {
                        let mut g = GeologyLayer::default();
                        g.resize(num_cells);
                        missing_stage = Some((
                            PipelineStage::Tectonics,
                            "размер сетки рельефа не совпал с топологией".to_string(),
                        ));
                        g
                    }
                    Some(Err(_)) => {
                        let mut g = GeologyLayer::default();
                        g.resize(num_cells);
                        missing_stage = Some((
                            PipelineStage::Tectonics,
                            "формат слоя тектоники устарел".to_string(),
                        ));
                        g
                    }
                    None => {
                        let mut g = GeologyLayer::default();
                        g.resize(num_cells);
                        missing_stage = Some((
                            PipelineStage::Tectonics,
                            "отсутствует слой тектоники".to_string(),
                        ));
                        g
                    }
                };

                thread_logger.start_step("Распаковка климата, гидрологии и биомов...");
                let climate = if missing_stage.is_none() {
                    match save_file
                        .climate_bytes
                        .as_deref()
                        .map(bincode::deserialize::<ClimateLayer>)
                    {
                        Some(Ok(loaded_clim)) if loaded_clim.temperatures.len() == num_cells => {
                            loaded_clim
                        }
                        Some(Ok(_)) | Some(Err(_)) => {
                            let mut c = ClimateLayer::default();
                            c.resize(num_cells);
                            missing_stage = Some((
                                PipelineStage::Climate,
                                "данные климата устарели или не совпадают по сетке".to_string(),
                            ));
                            c
                        }
                        None => {
                            let mut c = ClimateLayer::default();
                            c.resize(num_cells);
                            missing_stage = Some((
                                PipelineStage::Climate,
                                "отсутствует этап климата".to_string(),
                            ));
                            c
                        }
                    }
                } else {
                    let mut c = ClimateLayer::default();
                    c.resize(num_cells);
                    c
                };

                let hydrology = if missing_stage.is_none() {
                    match save_file
                        .hydrology_bytes
                        .as_deref()
                        .map(bincode::deserialize::<HydrologyLayer>)
                    {
                        Some(Ok(loaded_hydro)) if loaded_hydro.biomes.len() == num_cells => {
                            loaded_hydro
                        }
                        Some(Ok(_)) | Some(Err(_)) => {
                            let mut h = HydrologyLayer::default();
                            h.resize(num_cells);
                            missing_stage = Some((
                                PipelineStage::Hydrology,
                                "данные гидрологии и биомов устарели".to_string(),
                            ));
                            h
                        }
                        None => {
                            let mut h = HydrologyLayer::default();
                            h.resize(num_cells);
                            missing_stage = Some((
                                PipelineStage::Hydrology,
                                "отсутствует этап гидрологии и биомов".to_string(),
                            ));
                            h
                        }
                    }
                } else {
                    let mut h = HydrologyLayer::default();
                    h.resize(num_cells);
                    h
                };

                thread_logger.finish_ok();
                Ok(SaveLoadOutcome::Loaded(Box::new(LoadedPlanetPayload {
                    planet_name: name,
                    radius: save_file.radius,
                    seed: save_file.seed,
                    new_topology,
                    geology,
                    climate,
                    hydrology,
                    missing_stage,
                })))
            });

            commands.spawn(SaveLoadTask {
                title,
                is_saving: false,
                is_initial,
                logger,
                task: Some(task),
                finished_at: None,
            });
        }
    }
}

/// Система опроса фоновых задач сохранения/загрузки и мгновенного применения результата
#[allow(clippy::too_many_arguments)]
pub fn handle_save_load_task(
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut SaveLoadTask)>,
    mut save_ui: ResMut<SaveUiState>,
    mut world_config: ResMut<WorldConfig>,
    mut topology: ResMut<WorldTopology>,
    mut geology: ResMut<GeologyLayer>,
    mut climate: ResMut<ClimateLayer>,
    mut hydrology: ResMut<HydrologyLayer>,
    mut pipeline: ResMut<WorldPipeline>,
    mut gen_req: ResMut<GenerationRequest>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let Some((entity, mut task_comp)) = tasks.iter_mut().next() else {
        return;
    };

    if let Some(ref mut async_task) = task_comp.task {
        if let Some(result) = future::block_on(future::poll_once(async_task)) {
            task_comp.task = None;
            task_comp.finished_at = Some(Instant::now());
            save_ui.is_busy = false;

            match result {
                Ok(SaveLoadOutcome::Saved { planet_name }) => {
                    save_ui.refresh_lists();
                    save_ui.set_ok(format!("Планета «{}» успешно сохранена", planet_name));
                }
                Ok(SaveLoadOutcome::Loaded(payload)) => {
                    let LoadedPlanetPayload {
                        planet_name,
                        radius,
                        seed,
                        new_topology,
                        geology: loaded_geo,
                        climate: loaded_clim,
                        hydrology: loaded_hydro,
                        missing_stage,
                    } = *payload;

                    world_config.radius = radius;
                    world_config.seed = seed;
                    if let Some(new_topo) = new_topology {
                        world_config.next_planet_subdivisions = new_topo.subdivisions;
                        *topology = new_topo;
                    }

                    *geology = loaded_geo;
                    *climate = loaded_clim;
                    *hydrology = loaded_hydro;

                    pipeline.rebuild_mesh = true;
                    pipeline.rebuild_rivers = true;
                    next_state.set(AppState::InGame);
                    save_ui.planet_save_name = planet_name.clone();

                    if let Some((stage, reason)) = missing_stage {
                        if save_ui.auto_regenerate_missing {
                            save_ui.set_ok(format!(
                                "Загружена «{}» ({}). Запущена авто-догенерация...",
                                planet_name, reason
                            ));
                            gen_req.request(stage);
                        } else {
                            save_ui.set_ok(format!(
                                "Планета «{}» загружена частично ({})",
                                planet_name, reason
                            ));
                            save_ui.pending_prompt = Some(MissingStagePrompt {
                                planet_name,
                                missing_from: stage,
                                reason,
                            });
                        }
                    } else {
                        save_ui.set_ok(format!("Планета «{}» полностью загружена!", planet_name));
                    }
                }
                Err(err) => {
                    task_comp.logger.finish_err(&err);
                    save_ui.set_err(err);
                }
            }
        }
    } else if let Some(done_at) = task_comp.finished_at {
        // Держим оверлей 0.65 с после завершения, чтобы пользователь увидел 100% и тайминги
        if done_at.elapsed() >= Duration::from_millis(650) {
            commands.entity(entity).despawn();
        }
    }
}

/// Система отрисовки модальных окон сохранения/загрузки планет, конфигов и диалога догенерации
#[allow(clippy::too_many_arguments)]
pub fn draw_save_load_windows_system(
    In(ctx): In<egui::Context>,
    mut save_ui: ResMut<SaveUiState>,
    geology: Res<GeologyLayer>,
    mut t_config: ResMut<TectonicsConfig>,
    mut c_config: ResMut<ClimateConfig>,
    mut h_config: ResMut<HydrologyConfig>,
    mut gen_req: ResMut<GenerationRequest>,
) {
    let is_busy = save_ui.is_busy;

    // === 1. ОКНО СОХРАНЕНИЯ / ЗАГРУЗКИ ПЛАНЕТ ===
    if save_ui.show_planets_window {
        let mut open = save_ui.show_planets_window;
        egui::Window::new("💾 Сохранения планет")
            .open(&mut open)
            .resizable(true)
            .default_width(420.0)
            .show(&ctx, |ui| {
                // --- Секция сохранения текущей планеты ---
                ui.heading("Сохранить текущую планету");
                let has_planet = !geology.elevations.is_empty();

                ui.horizontal(|ui| {
                    ui.label("Имя файла:");
                    ui.text_edit_singleline(&mut save_ui.planet_save_name);
                });

                ui.horizontal(|ui| {
                    ui.label("Включить этапы:");
                    let mut tect_always = true;
                    ui.add_enabled(false, egui::Checkbox::new(&mut tect_always, "Тектоника"));
                    ui.checkbox(&mut save_ui.save_include_climate, "Климат");
                    if !save_ui.save_include_climate {
                        save_ui.save_include_hydrology = false;
                    }
                    ui.add_enabled(
                        save_ui.save_include_climate,
                        egui::Checkbox::new(&mut save_ui.save_include_hydrology, "Гидрология"),
                    );
                });

                ui.add_space(4.0);
                ui.add_enabled_ui(has_planet && !is_busy, |ui| {
                    if ui
                        .add_sized(
                            [ui.available_width(), 32.0],
                            egui::Button::new("💾 Сохранить планету (.planet)"),
                        )
                        .clicked()
                    {
                        save_ui.pending_request = Some(SaveLoadRequest::SavePlanet {
                            name: save_ui.planet_save_name.clone(),
                            include_climate: save_ui.save_include_climate,
                            include_hydrology: save_ui.save_include_hydrology,
                        });
                    }
                });
                if !has_planet {
                    ui.small("Сначала сгенерируйте или загрузите планету, чтобы сохранить её.");
                }

                ui.separator();

                // --- Секция загрузки планет ---
                ui.horizontal(|ui| {
                    ui.heading("Доступные планеты");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!is_busy, egui::Button::new("🔄 Обновить"))
                            .clicked()
                        {
                            save_ui.refresh_lists();
                        }
                    });
                });

                ui.checkbox(
                    &mut save_ui.auto_regenerate_missing,
                    "Автоматически догенерировать недостающие этапы без вопроса",
                );
                ui.add_space(4.0);

                let files = save_ui.planet_files.clone();
                if files.is_empty() {
                    ui.weak("Нет сохраненных планет в папке saves/planets/");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for entry in files {
                                ui.horizontal(|ui| {
                                    ui.strong(&entry.name);
                                    ui.weak(format!("({} КБ)", entry.size_kb));

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add_enabled_ui(!is_busy, |ui| {
                                                if ui.button("🗑").on_hover_text("Удалить").clicked()
                                                {
                                                    let _ = fs::remove_file(&entry.path);
                                                    save_ui.refresh_lists();
                                                }
                                                if ui.button("📂 Загрузить").clicked() {
                                                    save_ui.pending_request =
                                                        Some(SaveLoadRequest::LoadPlanet {
                                                            name: entry.name.clone(),
                                                            path: entry.path.clone(),
                                                        });
                                                }
                                            });
                                        },
                                    );
                                });
                                ui.separator();
                            }
                        });
                }

                if let Some((ref msg, is_err)) = save_ui.status_message {
                    ui.add_space(4.0);
                    let color = if is_err {
                        egui::Color32::from_rgb(230, 90, 90)
                    } else {
                        egui::Color32::from_rgb(90, 210, 120)
                    };
                    ui.colored_label(color, msg);
                }
            });
        save_ui.show_planets_window = open;
    }

    // === 2. ОКНО ПРЕСЕТОВ КОНФИГОВ (ОТДЕЛЬНО ОТ ПЛАНЕТ) ===
    if save_ui.show_configs_window {
        let mut open = save_ui.show_configs_window;
        egui::Window::new("⚙ Пресеты конфигов генерации")
            .open(&mut open)
            .resizable(true)
            .default_width(400.0)
            .show(&ctx, |ui| {
                ui.heading("Сохранить текущие настройки");
                ui.horizontal(|ui| {
                    ui.label("Имя пресета:");
                    ui.text_edit_singleline(&mut save_ui.config_save_name);
                });

                if ui
                    .add_sized(
                        [ui.available_width(), 30.0],
                        egui::Button::new("💾 Сохранить пресет (.json)"),
                    )
                    .clicked()
                {
                    let clean_name = sanitize_filename(&save_ui.config_save_name);
                    let _ = fs::create_dir_all(CONFIG_SAVE_DIR);
                    let path = Path::new(CONFIG_SAVE_DIR).join(format!("{}.json", clean_name));

                    let preset = ConfigPresetFile {
                        tectonics: (*t_config).clone(),
                        climate: (*c_config).clone(),
                        hydrology: (*h_config).clone(),
                    };

                    match serde_json::to_string_pretty(&preset)
                        .map_err(|e| e.to_string())
                        .and_then(|json| fs::write(&path, json).map_err(|e| e.to_string()))
                    {
                        Ok(()) => {
                            save_ui.refresh_lists();
                            save_ui.set_ok(format!("Пресет «{}» сохранен", clean_name));
                        }
                        Err(err) => {
                            save_ui.set_err(format!("Ошибка сохранения пресета: {}", err));
                        }
                    }
                }

                ui.add_space(4.0);
                if ui
                    .button("↺ Сбросить все конфиги к значениям по умолчанию")
                    .clicked()
                {
                    *t_config = TectonicsConfig::default();
                    *c_config = ClimateConfig::default();
                    *h_config = HydrologyConfig::default();
                    save_ui.set_ok("Все конфиги сброшены к значениям по умолчанию");
                }

                ui.separator();
                ui.heading("Загрузить пресет");
                ui.horizontal(|ui| {
                    ui.label("Применять к:");
                    ui.checkbox(&mut save_ui.load_tectonics_cfg, "Тектоника");
                    ui.checkbox(&mut save_ui.load_climate_cfg, "Климат");
                    ui.checkbox(&mut save_ui.load_hydrology_cfg, "Гидрология");
                });

                let files = save_ui.config_files.clone();
                if files.is_empty() {
                    ui.weak("Нет сохраненных пресетов в папке saves/configs/");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for entry in files {
                                ui.horizontal(|ui| {
                                    ui.strong(&entry.name);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("🗑").clicked() {
                                                let _ = fs::remove_file(&entry.path);
                                                save_ui.refresh_lists();
                                            }
                                            if ui.button("📥 Применить").clicked() {
                                                match fs::read_to_string(&entry.path)
                                                    .map_err(|e| e.to_string())
                                                    .and_then(|s| {
                                                        serde_json::from_str::<ConfigPresetFile>(&s)
                                                            .map_err(|e| e.to_string())
                                                    }) {
                                                    Ok(preset) => {
                                                        if save_ui.load_tectonics_cfg {
                                                            *t_config = preset.tectonics;
                                                        }
                                                        if save_ui.load_climate_cfg {
                                                            *c_config = preset.climate;
                                                        }
                                                        if save_ui.load_hydrology_cfg {
                                                            *h_config = preset.hydrology;
                                                        }
                                                        save_ui.set_ok(format!(
                                                            "Пресет «{}» применен",
                                                            entry.name
                                                        ));
                                                    }
                                                    Err(err) => {
                                                        save_ui.set_err(format!(
                                                            "Ошибка чтения пресета: {}",
                                                            err
                                                        ));
                                                    }
                                                }
                                            }
                                        },
                                    );
                                });
                                ui.separator();
                            }
                        });
                }
            });
        save_ui.show_configs_window = open;
    }

    // === 3. МОДАЛЬНОЕ ОКНО ДОГЕНЕРАЦИИ НЕДОСТАЮЩИХ ЭТАПОВ ===
    if let Some(prompt) = save_ui.pending_prompt.clone() {
        egui::Window::new("⚠ В сохранении отсутствуют этапы")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([420.0, 0.0])
            .show(&ctx, |ui| {
                ui.label(format!(
                    "Планета «{}» загружена частично: {}.",
                    prompt.planet_name, prompt.reason
                ));
                ui.add_space(6.0);
                ui.strong(format!(
                    "Догенерировать недостающие стадии начиная с этапа «{}»?",
                    stage_label(prompt.missing_from)
                ));
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui
                        .button(format!(
                            "▶ Догенерировать ({})",
                            stage_label(prompt.missing_from)
                        ))
                        .clicked()
                    {
                        gen_req.request(prompt.missing_from);
                        save_ui.pending_prompt = None;
                    }

                    if ui.button("Оставить как есть").clicked() {
                        save_ui.pending_prompt = None;
                    }
                });
            });
    }
}
