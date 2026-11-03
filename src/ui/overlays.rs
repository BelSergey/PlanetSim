// src/ui/overlays.rs

use bevy::prelude::*;
use bevy_egui::{EguiContext, egui};

use crate::{
    domains::{
        civilization::civ_palettes::{
            civ_culture_legend, civ_economy_legend, civ_kinship_legend, civ_knowledge_legend,
            civ_polity_legend, civ_religion_legend, civ_species_legend,
        },
        climate::climate_palettes::{
            albedo_legend, continentality_legend, humidity_legend, precipitation_legend,
            pressure_legend, temperature_legend, wind_legend,
        },
        generator::GenerationTask,
        hydrology::hydrology_palettes::{
            bio_resources_legend, biomes_legend, food_potential_legend, minerals_legend,
            soil_moisture_legend, soils_legend,
        },
        palettes::{LegendEntry, MapLegend},
        save::SaveLoadTask,
        tectonics::tectonics_palettes::{
            crust_age_legend, elevation_legend, lithology_legend, plates_legend, ruggedness_legend,
            tectonic_stress_legend,
        },
    },
    ui::shell::ShellLayout,
    viewport::{MapOverlay, palettes::space_view_legend},
};

/// Отрисовать строки «лога задач» (одинаковы для генерации и сейв/лоада).
macro_rules! draw_task_entries {
    ($ui:expr, $width:expr, $entries:expr) => {{
        for entry in $entries {
            $ui.horizontal(|ui| {
                ui.set_width($width);
                if let Some(ms) = entry.duration_ms {
                    ui.label(
                        egui::RichText::new("✔").color(egui::Color32::from_rgb(80, 210, 120)),
                    );
                    ui.label(
                        egui::RichText::new(&entry.message).color(egui::Color32::from_gray(200)),
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
            $ui.add_space(2.0);
        }
    }};
}

/// Неблокирующий виджет лога генерации.
pub fn generation_progress_overlay(
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

    let content_width = 540.0;

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

            egui::ScrollArea::vertical()
                .max_height(680.0)
                .auto_shrink([false, true])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    draw_task_entries!(ui, content_width, &snap.entries);
                });

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

/// Неблокирующий оверлей сохранения / загрузки планеты.
pub fn save_load_progress_overlay(
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

            draw_task_entries!(ui, content_width, &snap.entries);

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

/// Открыто/закрыто ли окно легенды.
#[derive(Resource)]
pub struct LegendState {
    pub open: bool,
}

impl Default for LegendState {
    fn default() -> Self {
        Self { open: true }
    }
}

/// Плавающая кнопка легенды в левом-нижнем углу свободной области вьюпорта.
pub fn map_legend_overlay(
    mut contexts: Query<&mut EguiContext>,
    mut legend_state: ResMut<LegendState>,
    overlay: Res<MapOverlay>,
    layout: Res<ShellLayout>,
) -> Result {
    let Ok(mut egui_ctx) = contexts.single_mut() else {
        return Ok(());
    };

    const PAD: f32 = 10.0;
    const SIDE: f32 = 170.0;
    // Оценка «хрома» окна: separator + паддинги + сама кнопка.
    const CHROME_H: f32 = 60.0;
    const MIN_LEGEND_H: f32 = 120.0;

    let offset = [
        layout.left.width() + PAD + 10.0,
        -(layout.bottom.height() + PAD),
    ];

    egui::Window::new("map_legend_panel")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::LEFT_BOTTOM, offset)
        .show(egui_ctx.get_mut(), |ui| {
            ui.set_min_width(SIDE);

            // Контент сверху, кнопка снизу → при раскрытии панель растёт
            // вверх, а кнопка остаётся на месте (bottom-anchor).
            if legend_state.open {
                let screen_h = ui.ctx().content_rect().height();
                let max_h = (screen_h * 0.7 - CHROME_H).max(MIN_LEGEND_H);

                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    // width: заполнить, height: сжаться до контента
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width(SIDE);
                        draw_legend_for(ui, &overlay);
                    });
                ui.separator();
            }

            let marker = if legend_state.open { "[-]" } else { "[+]" };
            let label = format!("Легенда  {}", marker);
            if ui
                .add_sized([170.0, 28.0], egui::Button::new(label))
                .clicked()
            {
                legend_state.open = !legend_state.open;
            }
        });

    Ok(())
}

fn draw_legend_for(ui: &mut egui::Ui, overlay: &MapOverlay) {
    let legend: MapLegend = match overlay {
        MapOverlay::SpaceView => space_view_legend(),
        MapOverlay::Elevation => elevation_legend(),
        MapOverlay::TectonicPlates => plates_legend(),
        MapOverlay::Continentality => continentality_legend(),
        MapOverlay::Lithology => lithology_legend(),
        MapOverlay::Ruggedness => ruggedness_legend(),
        MapOverlay::CrustAge => crust_age_legend(),
        MapOverlay::TectonicStress => tectonic_stress_legend(),
        MapOverlay::Temperature => temperature_legend(),
        MapOverlay::Humidity => humidity_legend(),
        MapOverlay::Precipitation => precipitation_legend(),
        MapOverlay::Albedo => albedo_legend(),
        MapOverlay::Pressure => pressure_legend(),
        MapOverlay::Winds => wind_legend(),
        MapOverlay::Soils => soils_legend(),
        MapOverlay::SoilMoisture => soil_moisture_legend(),
        MapOverlay::Biomes => biomes_legend(),
        MapOverlay::FoodPotential => food_potential_legend(),
        MapOverlay::Minerals => minerals_legend(),
        MapOverlay::BioResources => bio_resources_legend(),
        MapOverlay::CivSpecies => civ_species_legend(),
        MapOverlay::CivKinship => civ_kinship_legend(),
        MapOverlay::CivCulture => civ_culture_legend(),
        MapOverlay::CivReligion => civ_religion_legend(),
        MapOverlay::CivEconomy => civ_economy_legend(),
        MapOverlay::CivPolity => civ_polity_legend(),
        MapOverlay::CivKnowledge => civ_knowledge_legend(),
    };
    draw_legend(ui, &legend);
}

fn draw_legend(ui: &mut egui::Ui, legend: &MapLegend) {
    ui.add_space(4.0);
    ui.heading(legend.title);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    for entry in &legend.entries {
        draw_legend_entry(ui, entry);
    }
}

fn draw_legend_entry(ui: &mut egui::Ui, entry: &LegendEntry) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 14.0), egui::Sense::hover());
        let color = egui::Color32::from_rgb(entry.rgb[0], entry.rgb[1], entry.rgb[2]);
        // egui 0.31+: CornerRadius. Для более старых версий: egui::Rounding::same(2.0).
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(2), color);
        ui.add_space(4.0);
        ui.label(entry.label.as_str());
    });
    ui.add_space(2.0);
}
