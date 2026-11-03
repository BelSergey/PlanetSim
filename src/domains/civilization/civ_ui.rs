// src/domains/civilization/civ_ui.rs

use std::collections::HashSet;

use bevy::prelude::*;
use bevy_egui::egui;

use super::{
    civ_config::CivilizationConfig,
    civ_data::{CivilizationLayer, SocLayerKind},
    solver::{prepare_civilization_substrate, step_civilization_simulation},
};
use crate::{
    domains::{
        climate::climate_data::ClimateLayer, hydrology::hydrology_data::HydrologyLayer,
        tectonics::tectonics_data::GeologyLayer,
    },
    world::{WorldConfig, pipeline::WorldPipeline, topology::WorldTopology},
};

#[allow(clippy::too_many_arguments)]
pub fn draw_civilization_tab_system(
    InMut(ui): InMut<'_, egui::Ui>,
    mut config: ResMut<CivilizationConfig>,
    mut civ: ResMut<CivilizationLayer>,
    topology: Res<WorldTopology>,
    geology: Res<GeologyLayer>,
    climate: Res<ClimateLayer>,
    hydrology: Res<HydrologyLayer>,
    world_config: Res<WorldConfig>,
    mut pipeline: ResMut<WorldPipeline>,
) {
    ui.heading("🏛 Симуляция Общества");
    ui.separator();

    // 1. Управление временем (Play / Pause / Шаги / Сброс)
    ui.horizontal(|ui| {
        let play_label = if config.is_running {
            "⏸ Пауза"
        } else {
            "▶ Старт"
        };
        if ui
            .add_sized([95.0, 30.0], egui::Button::new(play_label))
            .clicked()
        {
            config.is_running = !config.is_running;
            pipeline.recolor_mesh = true;
        }

        if ui
            .add_enabled(!config.is_running, egui::Button::new("⏭ +1 шаг"))
            .clicked()
        {
            step_civilization_simulation(
                &geology,
                &hydrology,
                &mut civ,
                &config,
                world_config.seed,
            );
            pipeline.recolor_mesh = true;
        }

        if ui
            .add_enabled(!config.is_running, egui::Button::new("⏩ +25"))
            .clicked()
        {
            for _ in 0..25 {
                step_civilization_simulation(
                    &geology,
                    &hydrology,
                    &mut civ,
                    &config,
                    world_config.seed,
                );
            }
            pipeline.recolor_mesh = true;
        }

        if ui
            .button("🔄 Сброс")
            .on_hover_text("Очистить историю и заново засеять стартовые очаги")
            .clicked()
        {
            config.is_running = false;
            prepare_civilization_substrate(
                &topology,
                &geology,
                &climate,
                &hydrology,
                &mut civ,
                world_config.seed,
            );
            pipeline.recolor_mesh = true;
        }
    });

    ui.add_space(4.0);
    ui.add(egui::Slider::new(&mut config.steps_per_frame, 1..=15).text("Шагов за кадр (Скорость)"));
    ui.weak(format!("Эпоха (Тик): {}", civ.tick));

    ui.separator();

    // 2. Выбор института для настройки его физики (включение/выключение и параметры)
    ui.label("Настраиваемый институт (Слой 0..6):");
    for layer_kind in SocLayerKind::ALL {
        let idx = layer_kind.index();
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut config.specs[idx].active, "")
                .on_hover_text("Включить/выключить расчет этого слоя")
                .changed()
            {
                pipeline.recolor_mesh = true;
            }
            ui.selectable_value(&mut config.inspected_layer, layer_kind, layer_kind.label());
        });
    }

    // 3. Телеметрия по выбранному институту
    let k = config.inspected_layer.index();
    let mut unique_entities = HashSet::new();
    let mut inhabited_cells = 0usize;
    let mut max_bits = 0u32;

    for c in &civ.cells {
        if c.entity_id[k] != 0 && c.strength[k] >= 0.05 {
            unique_entities.insert(c.entity_id[k]);
            inhabited_cells += 1;
            let b = c.ideas[k].count_functional_ones();
            if b > max_bits {
                max_bits = b;
            }
        }
    }

    ui.separator();
    ui.label(
        egui::RichText::new(format!(
            "Сущностей: {}  |  Клеток: {}  |  Макс. идей: {}",
            unique_entities.len(),
            inhabited_cells,
            max_bits
        ))
        .weak(),
    );

    // 4. Ползунки физики выбранного слоя (InstitutionSpec)
    ui.separator();
    ui.strong(format!("Параметры: {}", config.inspected_layer.label()));

    let spec = &mut config.specs[k];
    ui.add(egui::Slider::new(&mut spec.speed, 0.01..=0.50).text("Скорость развития"));
    ui.add(egui::Slider::new(&mut spec.spatial_diffusion, 0.05..=1.0).text("Экспансия на соседей"));
    ui.add(egui::Slider::new(&mut spec.distance_decay, 0.0..=0.80).text("Штраф размера (Распад)"));
    ui.add(
        egui::Slider::new(&mut spec.fracture_threshold, 0.20..=1.20).text("Порог войны / раскола"),
    );
    ui.add(
        egui::Slider::new(&mut spec.emergence_threshold, 0.10..=0.90).text("Порог авторождения"),
    );

    ui.add_space(6.0);
    ui.label("Эволюция и диффузия битов (IdeaBits):");
    ui.add(egui::Slider::new(&mut spec.env_imprint_rate, 0.0..=0.60).text("Считывание природы"));
    ui.add(
        egui::Slider::new(&mut spec.cross_layer_learn_rate, 0.0..=0.80)
            .text("Обучение от других слоев"),
    );
    ui.add(
        egui::Slider::new(&mut spec.internal_diffusion_rate, 0.0..=1.0)
            .text("Внутренний обмен (Рынок)"),
    );
    ui.add(
        egui::Slider::new(&mut spec.border_osmosis_rate, 0.0..=0.60)
            .text("Просачивание через границу"),
    );
    ui.add(
        egui::Slider::new(&mut spec.conquest_loot_chance, 0.0..=1.0).text("Захват идей в войне"),
    );
}
