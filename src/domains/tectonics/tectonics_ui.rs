// src/domains/tectonics/tectonics_ui.rs

use super::tectonics_config::TectonicsConfig;
use crate::{domains::generator::GenerationRequest, world::pipeline::PipelineStage};
use bevy::prelude::*;
use bevy_egui::egui;

pub fn draw_tectonics_tab_system(
    InMut(ui): InMut<'_, egui::Ui>,
    mut config: ResMut<TectonicsConfig>,
    mut gen_req: ResMut<GenerationRequest>,
) {
    ui.heading("Настройки Тектоники");
    ui.separator();

    let regen_btn = ui.add_sized(
        [ui.available_width(), 40.0],
        egui::Button::new("Сгенерировать ландшафт"),
    );
    if regen_btn.clicked() {
        gen_req.request(PipelineStage::Tectonics);
    }

    ui.separator();
    ui.label("1. Плиты и кратоны:");
    ui.add(egui::Slider::new(&mut config.num_plates, 5..=40).text("Кол-во плит"));
    ui.add(egui::Slider::new(&mut config.continental_ratio, 0.15..=0.80).text("Доля континентов"));
    ui.add(
        egui::Slider::new(&mut config.pure_oceanic_plate_fraction, 0.05..=0.45)
            .text("Доля чисто океанических плит"),
    );
    ui.add(
        egui::Slider::new(&mut config.craton_eccentricity, 0.0..=1.2)
            .text("Смещение кратона к краю плиты"),
    );
    ui.add(
        egui::Slider::new(&mut config.plate_base_height_variance, 0.0..=0.08)
            .text("Разброс базовой высоты плит"),
    );

    ui.separator();
    ui.label("2. Симуляция движения и силы:");
    ui.add(
        egui::Slider::new(&mut config.simulation_steps, 5..=120).text("Шагов движения плит"),
    );
    ui.add(egui::Slider::new(&mut config.slab_pull_strength, 0.0..=6.0).text("Тяга субдукции (Slab pull)"));
    ui.add(egui::Slider::new(&mut config.ridge_push_strength, 0.0..=4.0).text("Толчок хребтов (Ridge push)"));
    ui.add(
        egui::Slider::new(&mut config.collision_resistance_strength, 0.5..=8.0)
            .text("Сопротивление коллизии"),
    );
    ui.add(egui::Slider::new(&mut config.hotspot_count, 0..=40).text("Кол-во плюмов"));

    ui.separator();
    ui.label("3. Изостазия и океаны:");
    ui.add(egui::Slider::new(&mut config.cont_base_height, 0.02..=0.35).text("Базовая высота суши"));
    ui.add(egui::Slider::new(&mut config.ocean_base_height, -0.65..=-0.15).text("Глубина океана"));
    ui.add(
        egui::Slider::new(&mut config.min_ocean_size_ratio, 0.002..=0.06)
            .text("Мин. площадь океана"),
    );
    ui.add(
        egui::Slider::new(&mut config.ocean_abyssal_threshold, -0.45..=-0.08)
            .text("Порог абиссали океана"),
    );

    ui.separator();
    ui.label("4. Шум и детализация:");
    ui.add(egui::Slider::new(&mut config.warp_strength, 0.0..=1.0).text("Сила искажения границ"));
    ui.add(egui::Slider::new(&mut config.boundary_noise_amp, 0.0..=1.5).text("Амплитуда горного шума"));
    ui.add(
        egui::Slider::new(&mut config.detail_noise_amp, 0.0..=0.5)
            .text("Мелкий шум (шероховатость)"),
    );
}