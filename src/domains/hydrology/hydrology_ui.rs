// src/domains/hydrology/hydrology_ui.rs

use bevy::prelude::*;
use bevy_egui::egui;

use super::hydrology_config::HydrologyConfig;
use crate::{
    domains::generator::GenerationRequest,
    world::pipeline::{PipelineStage, WorldPipeline},
};

pub fn draw_hydrology_tab_system(
    InMut(ui): InMut<'_, egui::Ui>,
    mut config: ResMut<HydrologyConfig>,
    mut gen_req: ResMut<GenerationRequest>,
    mut pipeline: ResMut<WorldPipeline>,
) {
    ui.heading("Биосфера и Поверхность");
    ui.separator();

    let regen_btn = ui.add_sized(
        [ui.available_width(), 40.0],
        egui::Button::new("Пересчитать почвы, озёра, реки и биомы"),
    );
    if regen_btn.clicked() {
        gen_req.request(PipelineStage::Hydrology);
    }

    ui.separator();
    ui.label("1. Модель почв (CLORPT):");
    ui.add(
        egui::Slider::new(&mut config.chemical_weathering_rate, 0.0..=1.2)
            .text("Хим. выветривание (Глины)"),
    );
    ui.add(
        egui::Slider::new(&mut config.slope_erosion_sensitivity, 2.0..=40.0)
            .text("Смыв на склонах"),
    );
    ui.add(
        egui::Slider::new(&mut config.mountain_slope_threshold, 0.01..=0.10)
            .text("Порог горных скал"),
    );
    ui.add(
        egui::Slider::new(&mut config.permafrost_temp_threshold, -15.0..=2.0)
            .text("Порог мерзлоты (°C)"),
    );
    ui.add(
        egui::Slider::new(&mut config.aridity_evap_scale, 0.4..=2.0).text("Масштаб засушливости"),
    );

    ui.add_space(10.0);
    ui.separator();
    ui.label("2. Реки и Озёра:");

    ui.add(egui::Slider::new(&mut config.runoff_multiplier, 0.2..=3.0).text("Множитель стока"));
    ui.add(
        egui::Slider::new(&mut config.lithology_erosion_bias, 0.0..=0.05)
            .text("Влияние мягкости пород"),
    );
    ui.add(
        egui::Slider::new(&mut config.min_lake_depth, 0.0005..=0.015)
            .text("Мин. глубина чаши озера"),
    );
    ui.add(
        egui::Slider::new(&mut config.lake_evaporation_scale, 0.3..=2.5)
            .text("Испарение с озёр (пресные ↔ солёные)"),
    );

    let mut mesh_changed = false;
    mesh_changed |= ui
        .add(
            egui::Slider::new(&mut config.river_threshold, 0.00008..=0.0025)
                .logarithmic(true)
                .text("Порог рек (ручьи ↔ крупные)"),
        )
        .changed();
    mesh_changed |= ui
        .add(egui::Slider::new(&mut config.river_width_scale, 0.3..=5.0).text("Ширина русел"))
        .changed();

    ui.add(
        egui::Slider::new(&mut config.river_groundwater_seepage, 0.0..=2.5)
            .text("Подпитка почвы от рек и озёр"),
    );

    ui.add_space(4.0);
    ui.separator();
    ui.label("Обратная связь с климатом:");

    ui.checkbox(
        &mut config.coupled_climate_feedback,
        "Включить обратную связь (Альбедо и Испарение)",
    );

    ui.checkbox(
        &mut config.defer_initial_feedback,
        "Отложить при первичной генерации",
    )
    .on_hover_text(
        "Экономит ~1300 мс при первом показе планеты.\n\
     Биомы строятся по усреднённому альбедо суши.\n\
     Полный feedback запускается кнопкой ниже.",
    );

    let feedback_enabled = config.coupled_climate_feedback && !config.defer_initial_feedback;

    let feedback_btn = ui.add_sized(
        [ui.available_width(), 34.0],
        egui::Button::new("🌡 Применить обратную связь климата (~1.2 с)"),
    );

    if feedback_btn
        .on_hover_text(
            "Пересчитает климат, используя текущие биомы, реки и озёра.\n\
         Запускает PipelineStage::Hydrology.",
        )
        .clicked()
    {
        gen_req.request(PipelineStage::Hydrology);
    }

    if !feedback_enabled {
        ui.weak("ℹ Сейчас feedback отложен — биомы построены по усреднённому альбедо.");
    }

    ui.add_space(10.0);
    ui.separator();
    ui.label("3. Биомы и Экология:");

    ui.add(
        egui::Slider::new(&mut config.soil_biome_influence, 0.0..=1.2)
            .text("Влияние почв на биомы"),
    );
    ui.add(
        egui::Slider::new(&mut config.riparian_river_multiplier, 1.0..=5.0)
            .text("Порог речных оазисов"),
    );
    ui.add(
        egui::Slider::new(&mut config.wetland_max_slope, 0.002..=0.025).text("Уклон заболачивания"),
    );

    ui.add_space(10.0);
    ui.separator();
    ui.label("4. Ресурсы и Агропотенциал:");

    ui.add(
        egui::Slider::new(&mut config.food_yield_multiplier, 0.3..=2.5)
            .text("Продовольственная ёмкость"),
    );
    ui.add(
        egui::Slider::new(&mut config.mineral_density, 0.3..=2.5)
            .text("Плотность руд и ископаемых"),
    );
    ui.add(
        egui::Slider::new(&mut config.bio_resource_abundance, 0.3..=2.5)
            .text("Обилие биоресурсов и фауны"),
    );

    if mesh_changed {
        pipeline.rebuild_rivers = true;
    }
}
