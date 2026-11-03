// src/domains/climate/climate_ui.rs

use bevy::prelude::*;
use bevy_egui::egui;

use super::SeasonalControl;
use super::{
    climate_config::ClimateConfig,
    climate_data::{ClimateLayer, ClimateSeason},
};
use crate::{
    domains::generator::GenerationRequest,
    world::pipeline::{PipelineStage, WorldPipeline},
};

pub fn draw_climate_tab_system(
    InMut(ui): InMut<'_, egui::Ui>,
    mut config: ResMut<ClimateConfig>,
    mut climate: ResMut<ClimateLayer>,
    mut seasonal_control: ResMut<SeasonalControl>,
    mut pipeline: ResMut<WorldPipeline>,
    mut gen_req: ResMut<GenerationRequest>,
) {
    ui.heading("Климат и Атмосфера");
    ui.separator();

    // Кнопка запуска пересчета климата (как в тектонике!)
    let regen_btn = ui.add_sized(
        [ui.available_width(), 40.0],
        egui::Button::new("Пересчитать климат"),
    );
    if regen_btn.clicked() {
        gen_req.request(PipelineStage::Climate);
    }

    ui.separator();

    // --- БЫСТРЫЙ ПЕРЕКЛЮЧАТЕЛЬ ПРЕДРАССЧИТАННЫХ СЕЗОНОВ ---
    ui.checkbox(
        &mut seasonal_control.automatic,
        "Автоматическая смена сезонов по орбите",
    );
    // (Оставляем мгновенным, так как все 3 сезона уже лежат в памяти!)
    ui.label("Отображаемый сезон (Сев. полушарие):");
    let mut season_changed = false;
    ui.horizontal(|ui| {
        season_changed |= ui
            .selectable_value(
                &mut config.active_season,
                ClimateSeason::Spring,
                "Весна / Осень",
            )
            .changed();
        season_changed |= ui
            .selectable_value(&mut config.active_season, ClimateSeason::Summer, "Лето")
            .changed();
        season_changed |= ui
            .selectable_value(&mut config.active_season, ClimateSeason::Winter, "Зима")
            .changed();
    });

    if season_changed {
        seasonal_control.automatic = false;
        climate.apply_active_season(config.active_season);
        pipeline.recolor_mesh = true;
    }

    ui.separator();
    ui.label("Температура и Инсоляция:");
    ui.add(egui::Slider::new(&mut config.equator_temp, 0.0..=60.0).text("Экватор (°C)"));
    ui.add(egui::Slider::new(&mut config.pole_temp, -70.0..=10.0).text("Полюса (°C)"));
    ui.add(
        egui::Slider::new(&mut config.altitude_lapse_rate, 0.0..=100.0).text("Остывание в горах"),
    );
    ui.add(egui::Slider::new(&mut config.freezing_temp, -25.0..=5.0).text("Порог замерзания льда"));
    ui.add(egui::Slider::new(&mut config.axial_tilt, 0.0..=0.8).text("Наклон оси (рад)"));

    ui.separator();
    ui.label("Континентальность:");
    ui.add(egui::Slider::new(&mut config.base_ocean_moderation, 0.0..=1.0).text("Инерция океана"));
    ui.add(
        egui::Slider::new(&mut config.land_continentality_scale, 0.1..=1.5)
            .text("Размер береговой зоны (рад)"),
    );
    ui.add(
        egui::Slider::new(&mut config.ocean_continentality_scale, 0.02..=0.6)
            .text("Зона окраинных морей (рад)"),
    );

    ui.separator();
    ui.label("Давление и Ветра:");
    ui.add(
        egui::Slider::new(&mut config.latitudinal_pressure_weight, 0.0..=2.0)
            .text("Сила поясов (Хэдли и др.)"),
    );
    ui.add(
        egui::Slider::new(&mut config.thermal_pressure_weight, 0.0..=2.0).text("Влияние материков"),
    );
    ui.add(
        egui::Slider::new(&mut config.coriolis_strength, 0.0..=5.00).text("Сила Кориолиса (рад)"),
    );
    ui.add(egui::Slider::new(&mut config.mountain_wind_barrier, 0.0..=25.0).text("Барьер гор"));
    ui.add(egui::Slider::new(&mut config.wind_speed_multiplier, 1.0..=30.0).text("Скорость ветра"));
    ui.add(
        egui::Slider::new(&mut config.heat_advection_strength, 0.0..=1.5)
            .text("Распространение температуры"),
    );
    ui.add(
        egui::Slider::new(&mut config.equilibrium_iterations, 1..=8)
            .text("Итерации давления-температуры"),
    );

    ui.separator();
    ui.label("Влажность и Осадки:");
    ui.add(
        egui::Slider::new(&mut config.ocean_evaporation_rate, 0.1..=2.5).text("Испарение океана"),
    );
    ui.add(
        egui::Slider::new(&mut config.moisture_advection_strength, 0.1..=2.0)
            .text("Перенос влаги ветром"),
    );
    ui.add(
        egui::Slider::new(&mut config.orographic_rain_shadow, 0.0..=25.0).text("Дождевая тень гор"),
    );
    ui.add(
        egui::Slider::new(&mut config.subsidence_drying_strength, 0.0..=3.0)
            .text("Сухость в антициклонах"),
    );
    ui.add(
        egui::Slider::new(&mut config.precipitation_rate, 0.1..=2.5).text("Интенсивность осадков"),
    );
    ui.add(
        egui::Slider::new(&mut config.land_moisture_recycling, 0.0..=0.85)
            .text("Рециклинг влаги сушей"),
    );
}
