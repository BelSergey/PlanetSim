// src/ui/shell.rs

use bevy::prelude::*;
use bevy_egui::{EguiContext, egui};

use crate::{
    domains::{
        civilization::civ_ui::draw_civilization_tab_system,
        climate::climate_ui::draw_climate_tab_system,
        generator::GenerationTask,
        hydrology::hydrology_ui::draw_hydrology_tab_system,
        save::{SaveLoadTask, SaveUiState, draw_save_load_windows_system},
        tectonics::{GeologyLayer, tectonics_ui::draw_tectonics_tab_system},
    },
    simulation::clock::SimulationClock,
    viewport::{MapOverlay, StarSystemRoot, SunVisualConfig, ViewportViewConfig},
    world::{
        WorldConfig, pipeline::WorldPipeline, start_new_planet, topology::WorldTopology,
        world_ui::draw_world_tab,
    },
};

use crate::viewport::camera_3d::SystemInertia;

use super::ActiveTab;

/// Список слоёв карты — единственный источник истины для левой панели.
const LAYERS: &[(MapOverlay, &str)] = &[
    (MapOverlay::SpaceView, "Базовый"),
    (MapOverlay::Elevation, "Рельеф"),
    (MapOverlay::TectonicPlates, "Тектонические плиты"),
    (MapOverlay::Continentality, "Континентальность"),
    (MapOverlay::Lithology, "Породы"),
    (MapOverlay::Ruggedness, "Рельефность"),
    (MapOverlay::CrustAge, "Возраст коры"),
    (MapOverlay::TectonicStress, "Напряжение границ"),
    (MapOverlay::Temperature, "Температура"),
    (MapOverlay::Humidity, "Влажность"),
    (MapOverlay::Precipitation, "Осадки"),
    (MapOverlay::Albedo, "Альбедо"),
    (MapOverlay::Pressure, "Давление"),
    (MapOverlay::Winds, "Ветра"),
    (MapOverlay::Soils, "Почвы"),
    (MapOverlay::SoilMoisture, "Влажность почвы"),
    (MapOverlay::Biomes, "Биомы"),
    (MapOverlay::FoodPotential, "Продовольствие"),
    (MapOverlay::Minerals, "Ископаемые"),
    (MapOverlay::BioResources, "Биоресурсы"),
    (MapOverlay::CivSpecies, "Институт: Расы"),
    (MapOverlay::CivKinship, "Институт: Кланы"),
    (MapOverlay::CivCulture, "Институт: Культуры"),
    (MapOverlay::CivReligion, "Институт: Религии"),
    (MapOverlay::CivEconomy, "Институт: Рынки"),
    (MapOverlay::CivPolity, "Институт: Государства"),
    (MapOverlay::CivKnowledge, "Институт: Наука"),
];

#[derive(Resource, Clone, Copy, Debug)]
pub struct ShellLayout {
    pub top: egui::Rect,
    pub bottom: egui::Rect,
    pub left: egui::Rect,
    pub right: egui::Rect,
}

impl Default for ShellLayout {
    fn default() -> Self {
        Self {
            top: egui::Rect::ZERO,
            bottom: egui::Rect::ZERO,
            left: egui::Rect::ZERO,
            right: egui::Rect::ZERO,
        }
    }
}

pub fn render_ui_shell(world: &mut World) -> Result {
    let mut contexts = world.query::<&mut EguiContext>();
    let Ok(mut egui_ctx) = contexts.single_mut(world) else {
        return Ok(());
    };
    let ctx = egui_ctx.get_mut().clone();
    let rect = ctx.content_rect();

    let mut layout = ShellLayout::default();

    egui::Area::new(egui::Id::new("root_ui_shell"))
        .fixed_pos(rect.min)
        .order(egui::Order::Background)
        .show(&ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());

            layout.top = draw_top_panel(ui, world);
            layout.bottom = draw_bottom_panel(ui, world);
            layout.left = draw_left_panel(ui, world);
            layout.right = draw_right_panel(ui, world);
        });

    // Публикуем реальные размеры панелей на этот кадр.
    world.insert_resource(layout);

    let _ = world.run_system_cached_with(draw_save_load_windows_system, ctx);
    Ok(())
}
// === 0. ВЕРХНЯЯ СИСТЕМНАЯ ПАНЕЛЬ (Classic Top Bar) ===

fn draw_top_panel(ui: &mut egui::Ui, world: &mut World) -> egui::Rect {
    let mut panel_rect = egui::Rect::ZERO;
    egui::Panel::top("top_panel")
        .exact_size(32.0)
        .show(ui, |ui| {
            panel_rect = ui.max_rect();
            ui.horizontal_centered(|ui| {
                ui.strong("🌍 PlanetSim");
                ui.separator();

                world.resource_scope(|_world, mut save_ui: Mut<SaveUiState>| {
                    if ui
                        .selectable_label(save_ui.show_planets_window, "💾 Планеты")
                        .clicked()
                    {
                        save_ui.show_planets_window = !save_ui.show_planets_window;
                        if save_ui.show_planets_window {
                            save_ui.refresh_lists();
                        }
                    }
                    if ui
                        .selectable_label(save_ui.show_configs_window, "⚙ Пресеты конфигов")
                        .clicked()
                    {
                        save_ui.show_configs_window = !save_ui.show_configs_window;
                        if save_ui.show_configs_window {
                            save_ui.refresh_lists();
                        }
                    }
                });

                ui.separator();
                let new_planet_clicked = ui
                    .button("🚀 Новая планета")
                    .on_hover_text("Сгенерировать новую планету со случайным сидом")
                    .clicked();

                if new_planet_clicked {
                    let subdivs = {
                        let mut cfg = world.resource_mut::<WorldConfig>();
                        cfg.randomize_seed();
                        cfg.next_planet_subdivisions
                    };
                    start_new_planet(world, subdivs);
                }

                ui.separator();
                world.resource_scope(|_world, mut clock: Mut<SimulationClock>| {
                    let pause_label = if clock.paused { "▶" } else { "Ⅱ" };
                    if ui.small_button(pause_label).on_hover_text("Пауза / продолжить симуляцию").clicked() {
                        if clock.paused { clock.resume(); } else { clock.pause(); }
                    }
                    ui.add(
                        egui::Slider::new(&mut clock.time_scale, 3_600.0..=31_536_000.0)
                            .logarithmic(true)
                            .show_value(false)
                            .text("скорость"),
                    );
                    ui.weak(format!("День {:.1}", clock.elapsed_seconds / 86_400.0));
                });

                if ui.small_button("⟲ Вид").on_hover_text("Сбросить ручной поворот звёздной системы и масштаб камеры").clicked() {
                    let mut roots = world.query_filtered::<(&mut Transform, &mut SystemInertia), With<StarSystemRoot>>();
                    if let Ok((mut transform, mut inertia)) = roots.single_mut(world) {
                        transform.rotation = Quat::IDENTITY;
                        inertia.velocity = Vec2::ZERO;
                    }
                    let radius = world.resource::<WorldConfig>().radius;
                    let mut cameras = world.query_filtered::<&mut Transform, With<Camera3d>>();
                    if let Ok(mut camera) = cameras.single_mut(world) {
                        camera.translation = Vec3::new(0.0, 0.0, radius * 4.0);
                    }
                }

                let subdivs = world.resource::<WorldTopology>().subdivisions;
                let seed = world.resource::<WorldConfig>().seed;
                let has_geo = !world.resource::<GeologyLayer>().elevations.is_empty();

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
                    draw_status_text(ui, active_save_load, is_generating, seed, subdivs, has_geo);
                });
            });
        });
    panel_rect
}

fn draw_status_text(
    ui: &mut egui::Ui,
    active_save_load: Option<(String, bool)>,
    is_generating: bool,
    seed: u32,
    subdivs: u32,
    has_geo: bool,
) {
    const WARN: egui::Color32 = egui::Color32::from_rgb(240, 200, 80);

    if let Some((title, in_progress)) = active_save_load {
        ui.colored_label(WARN, format!(" {} ", title));
        if in_progress {
            ui.spinner();
        }
        ui.weak(format!("Сид: {} | subdiv {} | Статус:", seed, subdivs));
    } else if is_generating {
        ui.colored_label(WARN, " Генерация мира... ");
        ui.spinner();
        ui.weak(format!("Сид: {} | subdiv {} | Статус:", seed, subdivs));
    } else {
        let status = if has_geo {
            "Планета активна"
        } else {
            "Ожидание генерации"
        };
        ui.weak(format!(
            " Сид: {} | subdiv {} | Статус: {}",
            seed, subdivs, status
        ));
    }
}

// === 1. НИЖНЯЯ ПАНЕЛЬ (Вкладки доменов) ===

fn draw_bottom_panel(ui: &mut egui::Ui, world: &mut World) -> egui::Rect {
    let mut panel_rect = egui::Rect::ZERO;
    egui::Panel::bottom("bottom_panel")
        .exact_size(40.0)
        .show(ui, |ui| {
            panel_rect = ui.max_rect();
            let mut active_tab = world.resource_mut::<ActiveTab>();
            ui.horizontal_centered(|ui| {
                ui.selectable_value(&mut *active_tab, ActiveTab::World, "Планета");
                ui.selectable_value(&mut *active_tab, ActiveTab::Tectonics, "Тектоника");
                ui.selectable_value(&mut *active_tab, ActiveTab::Climate, "Климат");
                ui.selectable_value(&mut *active_tab, ActiveTab::Hydrology, "Гидрология");
                ui.selectable_value(&mut *active_tab, ActiveTab::Civilization, "Цивилизации");
            });
        });
    panel_rect
}

// === 2. ЛЕВАЯ ПАНЕЛЬ (Режимы отображения) ===

fn draw_left_panel(ui: &mut egui::Ui, world: &mut World) -> egui::Rect {
    let mut panel_rect = egui::Rect::ZERO;
    egui::Panel::left("left_panel")
        .default_size(200.0)
        .show(ui, |ui| {
            panel_rect = ui.max_rect();
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Отображение");
                ui.separator();
                ui.label("Слои карты:");

                let mut recolor = false;
                let mut reposition = false;

                world.resource_scope(|_world, mut overlay: Mut<MapOverlay>| {
                    for (layer, label) in LAYERS.iter() {
                        if ui
                            .radio_value(&mut *overlay, *layer, *label)
                            .changed()
                        {
                            recolor = true;
                        }
                    }
                });

                ui.separator();
                ui.label("Модификаторы:");

                world.resource_scope(|_world, mut sun_cfg: Mut<SunVisualConfig>| {
                    ui.checkbox(&mut sun_cfg.visible, "Видимый диск Солнца");
                });

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
                    ui.checkbox(&mut view_cfg.show_river_flow, "Стрелки течения рек");
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
        });
    panel_rect
}

// === 3. ПРАВАЯ ПАНЕЛЬ (делегируем отрисовку доменам) ===

fn draw_right_panel(ui: &mut egui::Ui, world: &mut World) -> egui::Rect {
    let mut panel_rect = egui::Rect::ZERO;
    let current_tab = *world.resource::<ActiveTab>();

    egui::Panel::right("right_panel")
        .default_size(300.0)
        .show(ui, |ui| {
            panel_rect = ui.max_rect();
            egui::ScrollArea::vertical().show(ui, |ui| match current_tab {
                ActiveTab::World => draw_world_tab(ui, world),
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
                    let _ = world.run_system_cached_with(draw_civilization_tab_system, ui);
                }
            });
        });
    panel_rect
}
