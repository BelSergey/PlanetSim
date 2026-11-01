// src/ui/mod.rs

use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::astronomy::Sun;
use crate::simulation::clock::SimulationClock;

use crate::{
    domains::{
        climate::{climate_data::ClimateLayer, climate_ui::draw_climate_tab_system},
        generator::{GenerationRequest, GenerationTask},
        hydrology::{HydrologyLayer, hydrology_ui::draw_hydrology_tab_system},
        save::{SaveLoadTask, SaveUiState, draw_save_load_windows_system},
        tectonics::{GeologyLayer, tectonics_ui::draw_tectonics_tab_system},
    },
    viewport::{Globe, MapOverlay, StarSystemRoot, ViewportViewConfig, camera_3d::SystemInertia},
    world::{
        WorldConfig,
        pipeline::{PipelineStage, WorldPipeline},
        start_new_planet,
        states::AppState,
        topology::WorldTopology,
    },
};

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum ActiveTab {
    #[default]
    World,
    Tectonics,
    Climate,
    Hydrology,
    Civilization,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }

        app.init_resource::<ActiveTab>()
            .add_systems(
                EguiPrimaryContextPass,
                (
                    render_ui_shell,
                    main_menu_ui.run_if(in_state(AppState::MainMenu)),
                    generation_progress_overlay,
                    save_load_progress_overlay,
                )
                    .chain(),
            )
            .add_systems(Update, manage_scene_visibility);
    }
}

fn manage_scene_visibility(
    state: Res<State<AppState>>,
    geology: Res<GeologyLayer>,
    mut q_globe: Query<&mut Visibility, Or<(With<Globe>, With<Sun>)>>,
) {
    let should_show = *state.get() == AppState::InGame && !geology.elevations.is_empty();
    let target_vis = if should_show {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    for mut vis in q_globe.iter_mut() {
        if *vis != target_vis {
            *vis = target_vis;
        }
    }
}

/// В главном меню поверх центрального вьюпорта висит компактное стартовое окно,
/// а правая/верхняя панели уже полностью доступны для настройки конфигов и загрузки сейвов
#[allow(clippy::too_many_arguments)]
fn main_menu_ui(
    mut contexts: Query<&mut EguiContext>,
    mut next_state: ResMut<NextState<AppState>>,
    mut gen_req: ResMut<GenerationRequest>,
    mut save_ui: ResMut<SaveUiState>,
    mut topology: ResMut<WorldTopology>,
    mut geology: ResMut<GeologyLayer>,
    mut climate: ResMut<ClimateLayer>,
    mut hydrology: ResMut<HydrologyLayer>,
    mut pipeline: ResMut<WorldPipeline>,
    mut config: ResMut<WorldConfig>,
) -> Result {
    let Ok(mut egui_ctx) = contexts.single_mut() else {
        return Ok(());
    };

    let target_subdivs = config.next_planet_subdivisions;

    egui::Window::new("main_menu")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(egui_ctx.get_mut(), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.heading("🌍 PlanetSim");
                ui.weak("Настройте конфиги справа или выберите действие:");
                ui.add_space(16.0);

                let btn_label = format!("🚀 Новая планета (subdiv {})", target_subdivs);
                if ui
                    .add_sized([260.0, 44.0], egui::Button::new(btn_label))
                    .clicked()
                {
                    config.randomize_seed();
                    if topology.subdivisions != target_subdivs {
                        *topology = WorldTopology::new(target_subdivs);
                    }
                    *geology = GeologyLayer::default();
                    *climate = ClimateLayer::default();
                    *hydrology = HydrologyLayer::default();

                    let p = &mut *pipeline;
                    p.rebuild_mesh = true;
                    p.recolor_mesh = true;
                    p.rebuild_rivers = true;

                    gen_req.request(PipelineStage::Tectonics);
                    next_state.set(AppState::InGame);
                }

                ui.add_space(6.0);
                if ui
                    .add_sized([260.0, 36.0], egui::Button::new("📂 Загрузить планету..."))
                    .clicked()
                {
                    save_ui.refresh_lists();
                    save_ui.show_planets_window = true;
                }

                ui.add_space(4.0);
                if ui
                    .add_sized([260.0, 32.0], egui::Button::new("⚙ Пресеты конфигов..."))
                    .clicked()
                {
                    save_ui.refresh_lists();
                    save_ui.show_configs_window = true;
                }

                ui.add_space(10.0);
            });
        });

    Ok(())
}

/// Неблокирующий виджет лога генерации (показывает историю этапов, тайминги и прогресс-бар)
fn generation_progress_overlay(
    mut contexts: Query<&mut EguiContext>,
    tasks: Query<&GenerationTask>,
) -> Result {
    let Some(task) = tasks.iter().next() else {
        return Ok(());
    };
    let Ok(mut egui_ctx) = contexts.single_mut() else {
        return Ok(());
    };

    let snap = task.logger.snapshot();
    let progress = snap.progress();

    let (align, offset) = if task.is_initial {
        (egui::Align2::CENTER_CENTER, [0.0, 0.0])
    } else {
        (egui::Align2::CENTER_TOP, [0.0, 42.0])
    };

    let content_width = 440.0;

    egui::Window::new("generation_status")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(align, offset)
        .fixed_size([content_width, 0.0])
        .show(egui_ctx.get_mut(), |ui| {
            ui.set_width(content_width);
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.set_width(content_width);
                if snap.is_finished {
                    ui.label(
                        egui::RichText::new("✔")
                            .color(egui::Color32::from_rgb(80, 210, 120))
                            .strong(),
                    );
                    ui.strong("Генерация мира завершена!");
                } else {
                    ui.spinner();
                    ui.strong("Генерация мира...");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Шаг {}/{}  •  {:.2} с",
                            snap.completed_steps.min(snap.total_steps),
                            snap.total_steps,
                            snap.total_elapsed_ms as f32 / 1000.0
                        ))
                        .weak()
                        .monospace(),
                    );
                });
            });

            ui.separator();
            ui.add_space(2.0);

            for entry in &snap.entries {
                ui.horizontal(|ui| {
                    ui.set_width(content_width);
                    if let Some(ms) = entry.duration_ms {
                        ui.label(
                            egui::RichText::new("✔").color(egui::Color32::from_rgb(80, 210, 120)),
                        );
                        ui.label(
                            egui::RichText::new(&entry.message)
                                .color(egui::Color32::from_gray(200)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{} мс", ms)).weak().monospace());
                        });
                    } else {
                        let live_ms = entry.started_at.elapsed().as_millis();
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(&entry.message)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{} мс...", live_ms))
                                    .color(egui::Color32::from_rgb(240, 200, 80))
                                    .monospace(),
                            );
                        });
                    }
                });
                ui.add_space(2.0);
            }

            ui.add_space(6.0);
            ui.add(
                egui::ProgressBar::new(progress)
                    .show_percentage()
                    .desired_width(content_width),
            );
            ui.add_space(4.0);
        });

    Ok(())
}

/// Неблокирующий оверлей сохранения / загрузки планеты (позволяет сразу закрыть окно сейвов)
fn save_load_progress_overlay(
    mut contexts: Query<&mut EguiContext>,
    tasks: Query<&SaveLoadTask>,
) -> Result {
    let Some(task) = tasks.iter().next() else {
        return Ok(());
    };
    let Ok(mut egui_ctx) = contexts.single_mut() else {
        return Ok(());
    };

    let snap = task.logger.snapshot();
    let progress = snap.progress();

    let (align, offset) = if task.is_initial && !task.is_saving {
        (egui::Align2::CENTER_CENTER, [0.0, 0.0])
    } else {
        (egui::Align2::CENTER_TOP, [0.0, 42.0])
    };

    let content_width = 440.0;

    egui::Window::new("save_load_status")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(align, offset)
        .fixed_size([content_width, 0.0])
        .show(egui_ctx.get_mut(), |ui| {
            ui.set_width(content_width);
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.set_width(content_width);
                if let Some(ref err) = snap.error {
                    ui.label(
                        egui::RichText::new("✖")
                            .color(egui::Color32::from_rgb(230, 90, 90))
                            .strong(),
                    );
                    ui.strong(format!("Ошибка: {}", err));
                } else if snap.is_finished {
                    ui.label(
                        egui::RichText::new("✔")
                            .color(egui::Color32::from_rgb(80, 210, 120))
                            .strong(),
                    );
                    let done_text = if task.is_saving {
                        format!("{} — готово!", task.title)
                    } else {
                        format!("{} — завершена!", task.title)
                    };
                    ui.strong(done_text);
                } else {
                    ui.spinner();
                    let icon = if task.is_saving { "💾" } else { "📂" };
                    ui.strong(format!("{} {}...", icon, task.title));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Шаг {}/{}  •  {:.2} с",
                            snap.completed_steps.min(snap.total_steps),
                            snap.total_steps,
                            snap.total_elapsed_ms as f32 / 1000.0
                        ))
                        .weak()
                        .monospace(),
                    );
                });
            });

            ui.separator();
            ui.add_space(2.0);

            for entry in &snap.entries {
                ui.horizontal(|ui| {
                    ui.set_width(content_width);
                    if let Some(ms) = entry.duration_ms {
                        ui.label(
                            egui::RichText::new("✔").color(egui::Color32::from_rgb(80, 210, 120)),
                        );
                        ui.label(
                            egui::RichText::new(&entry.message)
                                .color(egui::Color32::from_gray(200)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{} мс", ms)).weak().monospace());
                        });
                    } else {
                        let live_ms = entry.started_at.elapsed().as_millis();
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(&entry.message)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{} мс...", live_ms))
                                    .color(egui::Color32::from_rgb(240, 200, 80))
                                    .monospace(),
                            );
                        });
                    }
                });
                ui.add_space(2.0);
            }

            ui.add_space(6.0);
            ui.add(
                egui::ProgressBar::new(progress)
                    .show_percentage()
                    .desired_width(content_width),
            );
            ui.add_space(4.0);
        });

    Ok(())
}

fn render_ui_shell(world: &mut World) -> Result {
    let mut contexts = world.query::<&mut EguiContext>();
    let Ok(mut egui_ctx) = contexts.single_mut(world) else {
        return Ok(());
    };
    let ctx = egui_ctx.get_mut().clone();

    let rect = ctx.content_rect();
    let mut reset_system_view = false;

    egui::Area::new(egui::Id::new("root_ui_shell"))
        .fixed_pos(rect.min)
        .order(egui::Order::Background)
        .show(&ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());

            // === 0. ВЕРХНЯЯ СИСТЕМНАЯ ПАНЕЛЬ (Classic Top Bar) ===
            egui::Panel::top("top_panel")
                .exact_size(32.0)
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.strong("🌍 PlanetSim");
                        ui.separator();

                        let mut open_planets = false;
                        let mut open_configs = false;
                        let mut trigger_new_gen = false;

                        world.resource_scope(|_world, mut save_ui: Mut<SaveUiState>| {
                            if ui
                                .selectable_label(save_ui.show_planets_window, "💾 Планеты")
                                .clicked()
                            {
                                save_ui.show_planets_window = !save_ui.show_planets_window;
                                open_planets = save_ui.show_planets_window;
                            }
                            if ui
                                .selectable_label(save_ui.show_configs_window, "⚙ Пресеты конфигов")
                                .clicked()
                            {
                                save_ui.show_configs_window = !save_ui.show_configs_window;
                                open_configs = save_ui.show_configs_window;
                            }
                            if open_planets || open_configs {
                                save_ui.refresh_lists();
                            }
                        });

                        ui.separator();
                        if ui
                            .button("🚀 Новая планета")
                            .on_hover_text("Сгенерировать новую планету со случайным сидом")
                            .clicked()
                        {
                            trigger_new_gen = true;
                        }
                        ui.separator();

                        if ui
                            .button("↺ Исходный вид")
                            .on_hover_text("Вернуть звёздную систему к исходной ориентации")
                            .clicked()
                        {
                            reset_system_view = true;
                        }
                        if trigger_new_gen {
                            let subdivs = {
                                let mut cfg = world.resource_mut::<WorldConfig>();
                                cfg.randomize_seed();
                                cfg.next_planet_subdivisions
                            };
                            start_new_planet(world, subdivs);
                        }

                        let subdivs = world.resource::<WorldTopology>().subdivisions;
                        let seed = world.resource::<WorldConfig>().seed;
                        let has_geo = !world.resource::<GeologyLayer>().elevations.is_empty();

                        // Проверяем активные фоновые задачи для отображения в статус-баре
                        let active_save_load = world
                            .query::<&SaveLoadTask>()
                            .iter(world)
                            .next()
                            .map(|t| (t.title.clone(), t.task.is_some()));
                        let is_generating = world
                            .query::<&GenerationTask>()
                            .iter(world)
                            .next()
                            .is_some();

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Управление модельным временем
                            world.resource_scope(|_world, mut clock: Mut<SimulationClock>| {
                                let started = clock.is_started();

                                let speed_options = [
                                    (1.0, "1 сек/с"),
                                    (10.0, "10 сек/с"),
                                    (100.0, "100 сек/с"),
                                    (3_600.0, "1 час/с"),
                                    (86_400.0, "1 день/с"),
                                    (864_000.0, "10 дней/с"),
                                ];

                                let mut selected_speed = "Своя скорость";

                                for (speed, label) in speed_options {
                                    if (clock.time_scale - speed).abs() < 0.01 {
                                        selected_speed = label;
                                    }
                                }

                                if started {
                                    ui.label(format!(
                                        "Время: {:.2} д.",
                                        clock.elapsed_seconds / 86_400.0
                                    ));
                                } else {
                                    ui.weak("Ожидание планеты");
                                }

                                ui.add_enabled_ui(started, |ui| {
                                    let pause_label = if clock.paused {
                                        "▶ Продолжить"
                                    } else {
                                        "⏸ Пауза"
                                    };

                                    if ui.button(pause_label).clicked() {
                                        if clock.paused {
                                            clock.resume();
                                        } else {
                                            clock.pause();
                                        }
                                    }

                                    egui::ComboBox::from_id_salt("simulation_speed")
                                        .selected_text(selected_speed)
                                        .width(105.0)
                                        .show_ui(ui, |ui| {
                                            for (speed, label) in speed_options {
                                                if ui
                                                    .selectable_label(
                                                        (clock.time_scale - speed).abs() < 0.01,
                                                        label,
                                                    )
                                                    .clicked()
                                                {
                                                    clock.set_speed(speed);
                                                }
                                            }
                                        });
                                });

                                ui.separator();
                            });
                            if let Some((title, in_progress)) = active_save_load {
                                ui.colored_label(
                                    egui::Color32::from_rgb(240, 200, 80),
                                    format!(" {} ", title),
                                );
                                if in_progress {
                                    ui.spinner();
                                }
                                ui.weak(format!("Сид: {} | subdiv {} | Статус:", seed, subdivs));
                            } else if is_generating {
                                ui.colored_label(
                                    egui::Color32::from_rgb(240, 200, 80),
                                    " Генерация мира... ",
                                );
                                ui.spinner();
                                ui.weak(format!("Сид: {} | subdiv {} | Статус:", seed, subdivs));
                            } else {
                                ui.weak(format!(
                                    " Сид: {} | subdiv {} | Статус: {}",
                                    seed,
                                    subdivs,
                                    if has_geo {
                                        "Планета активна"
                                    } else {
                                        "Ожидание генерации"
                                    }
                                ));
                            }
                        });
                    });
                });

            // === 1. НИЖНЯЯ ПАНЕЛЬ (Вкладки доменов / Taskbar) ===
            egui::Panel::bottom("bottom_panel")
                .exact_size(40.0)
                .show(ui, |ui| {
                    let mut active_tab = world.resource_mut::<ActiveTab>();
                    ui.horizontal_centered(|ui| {
                        ui.selectable_value(&mut *active_tab, ActiveTab::World, "Планета");
                        ui.selectable_value(&mut *active_tab, ActiveTab::Tectonics, "Тектоника");
                        ui.selectable_value(&mut *active_tab, ActiveTab::Climate, "Климат");
                        ui.selectable_value(&mut *active_tab, ActiveTab::Hydrology, "Гидрология");
                        ui.selectable_value(
                            &mut *active_tab,
                            ActiveTab::Civilization,
                            "Цивилизации",
                        );
                    });
                });

            // === 2. ЛЕВАЯ ПАНЕЛЬ (Режимы отображения) ===
            egui::Panel::left("left_panel")
                .default_size(200.0)
                .show(ui, |ui| {
                    ui.heading("Отображение");
                    ui.separator();

                    let mut recolor = false;
                    let mut reposition = false;

                    ui.label("Слои карты:");

                    world.resource_scope(|_world, mut overlay: Mut<MapOverlay>| {
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::SpaceView, "Базовый")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Elevation, "Рельеф")
                            .changed();
                        recolor |= ui
                            .radio_value(
                                &mut *overlay,
                                MapOverlay::TectonicPlates,
                                "Тектонические плиты",
                            )
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Lithology, "Породы")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Ruggedness, "Рельефность")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Temperature, "Температура")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Humidity, "Влажность")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Precipitation, "Осадки")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Albedo, "Альбедо")
                            .changed();
                        recolor |= ui
                            .radio_value(
                                &mut *overlay,
                                MapOverlay::Continentality,
                                "Континентальность",
                            )
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Pressure, "Давление")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Winds, "Ветра")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Soils, "Почвы")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::SoilMoisture, "Влажность почвы")
                            .changed();
                        recolor |= ui
                            .radio_value(&mut *overlay, MapOverlay::Biomes, "Биомы")
                            .changed();
                    });

                    ui.separator();
                    ui.label("Модификаторы:");

                    world.resource_scope(|_world, mut view_cfg: Mut<ViewportViewConfig>| {
                        if ui
                            .checkbox(&mut view_cfg.flat_water, "Плоский океан")
                            .changed()
                        {
                            reposition = true;
                        }
                        ui.checkbox(&mut view_cfg.show_axis_equator, "Ось и Экватор");
                        ui.checkbox(&mut view_cfg.show_rivers, "Русла рек");
                        ui.checkbox(&mut view_cfg.show_wind_arrows, "3D-стрелки ветров");
                        ui.checkbox(&mut view_cfg.show_ocean_currents, "3D-стрелки течений");
                    });

                    if reposition || recolor {
                        let mut pipeline = world.resource_mut::<WorldPipeline>();
                        if reposition {
                            pipeline.reposition_mesh = true;
                        }
                        if recolor {
                            pipeline.recolor_mesh = true;
                        }
                    }
                });

            // === 3. ПРАВАЯ ПАНЕЛЬ (Делегируем отрисовку доменам!) ===
            let current_tab = *world.resource::<ActiveTab>();

            egui::Panel::right("right_panel")
                .default_size(300.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| match current_tab {
                        ActiveTab::Tectonics => {
                            let _ = world.run_system_cached_with(draw_tectonics_tab_system, ui);
                        }
                        ActiveTab::Climate => {
                            let _ = world.run_system_cached_with(draw_climate_tab_system, ui);
                        }
                        ActiveTab::Hydrology => {
                            let _ = world.run_system_cached_with(draw_hydrology_tab_system, ui);
                        }
                        ActiveTab::Civilization => {
                            ui.label("Настройки цивилизаций (в разработке)");
                        }
                        ActiveTab::World => {
                            // --- Секция 1: текущая планета ---
                            let (cur_subdivs, cell_count) = {
                                let topo = world.resource::<WorldTopology>();
                                (topo.subdivisions, topo.cells.len())
                            };

                            ui.heading("Текущая планета");
                            ui.label(format!(
                                "Размерность сетки: subdiv {} ({} ячеек)",
                                cur_subdivs, cell_count
                            ));
                            ui.separator();

                            // --- Секция 2: радиус (live, без перегенерации) ---
                            // Радиус применяется масштабом родительского Globe. Не пересобираем
                            // геометрию и речной меш при каждом шаге перетаскивания слайдера.
                            world.resource_scope(|_world, mut cfg: Mut<WorldConfig>| {
                                ui.add(
                                    egui::Slider::new(&mut cfg.radius, 1.0..=20.0)
                                        .step_by(0.1)
                                        .text("Радиус"),
                                );
                            });

                            ui.separator();

                            // --- Секция 3: сид и создание новой планеты ---
                            ui.heading("Параметры генерации");

                            let mut launch_subdivs: Option<(u32, bool)> = None;
                            world.resource_scope(|_world, mut cfg: Mut<WorldConfig>| {
                                ui.add(
                                    egui::Slider::new(&mut cfg.next_planet_subdivisions, 3..=9)
                                        .integer()
                                        .text("Детализация"),
                                );

                                let est = 10u64 * 4u64.pow(cfg.next_planet_subdivisions) + 2;
                                ui.weak(format!("≈ {} ячеек", est));
                                ui.add_space(6.0);

                                ui.horizontal(|ui| {
                                    ui.label("Сид мира:");
                                    let mut seed_str = cfg.seed.to_string();
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut seed_str)
                                                .desired_width(110.0),
                                        )
                                        .changed()
                                    {
                                        let digits: String = seed_str
                                            .chars()
                                            .filter(|c| c.is_ascii_digit())
                                            .collect();
                                        if digits.is_empty() {
                                            cfg.seed = 0;
                                        } else if let Ok(parsed) = digits.parse::<u32>() {
                                            cfg.seed = parsed;
                                        }
                                    }

                                    if ui
                                        .button("🎲 Случайный")
                                        .on_hover_text("Сгенерировать случайный сид в поле ввода")
                                        .clicked()
                                    {
                                        cfg.randomize_seed();
                                    }
                                });

                                ui.add_space(8.0);

                                if ui
                                    .add_sized(
                                        [ui.available_width(), 34.0],
                                        egui::Button::new("🔄 Перегенерировать с введённым сидом"),
                                    )
                                    .clicked()
                                {
                                    launch_subdivs = Some((cfg.next_planet_subdivisions, false));
                                }

                                ui.add_space(4.0);

                                if ui
                                    .add_sized(
                                        [ui.available_width(), 36.0],
                                        egui::Button::new("🚀 Новая планета (случайный сид)"),
                                    )
                                    .clicked()
                                {
                                    launch_subdivs = Some((cfg.next_planet_subdivisions, true));
                                }
                            });

                            if let Some((subdivs, randomize)) = launch_subdivs {
                                if randomize {
                                    world.resource_mut::<WorldConfig>().randomize_seed();
                                }
                                start_new_planet(world, subdivs);
                            }
                        }
                    });
                });
        });

    // Сброс ориентации всей звёздной системы.
    if reset_system_view {
        let mut query =
            world.query_filtered::<(&mut Transform, &mut SystemInertia), With<StarSystemRoot>>();

        if let Ok((mut transform, mut inertia)) = query.single_mut(world) {
            *transform = Transform::default();
            inertia.velocity = Vec2::ZERO;
        }
    }
    // Отрисовка модальных окон сохранения/загрузки и диалога догенерации
    let _ = world.run_system_cached_with(draw_save_load_windows_system, ctx);

    Ok(())
}
