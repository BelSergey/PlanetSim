// src/world/world_ui.rs

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    domains::tectonics::GeologyLayer,
    world::{WorldConfig, pipeline::WorldPipeline, start_new_planet, topology::WorldTopology},
};

/// Правая панель вкладки «Планета»: текущая планета, радиус, сид и запуск новой генерации.
///
/// Это свободная функция, а не `System`: `start_new_planet` требует `&mut World`,
/// поэтому вызывается из `render_ui_shell` (который сам — эксклюзивная система).
pub fn draw_world_tab(ui: &mut egui::Ui, world: &mut World) {
    // --- Секция 1: текущая планета ---
    let (cur_subdivs, cell_count, has_data) = {
        let topo = world.resource::<WorldTopology>();
        let geo = world.resource::<GeologyLayer>();
        (
            topo.subdivisions,
            topo.cells.len(),
            !geo.elevations.is_empty(),
        )
    };

    ui.heading("Текущая планета");
    ui.label(format!(
        "Размерность сетки: subdiv {} ({} ячеек)",
        cur_subdivs, cell_count
    ));
    ui.separator();

    // --- Секция 2: радиус (live, без перегенерации) ---
    let mut radius_changed = false;
    world.resource_scope(|_world, mut cfg: Mut<WorldConfig>| {
        radius_changed = ui
            .add(
                egui::Slider::new(&mut cfg.radius, 1.0..=10.0)
                    .step_by(0.1)
                    .text("Радиус"),
            )
            .changed();
    });
    if radius_changed && has_data {
        let mut p = world.resource_mut::<WorldPipeline>();
        p.reposition_mesh = true;
        p.rebuild_rivers = true;
    }

    ui.separator();

    // --- Секция 3: сид и создание новой планеты ---
    ui.heading("Параметры генерации");

    let mut launch_subdivs: Option<(u32, bool)> = None;
    world.resource_scope(|_world, mut cfg: Mut<WorldConfig>| {
        ui.add(
            egui::Slider::new(&mut cfg.next_planet_subdivisions, 3..=10)
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
                .add(egui::TextEdit::singleline(&mut seed_str).desired_width(110.0))
                .changed()
            {
                let digits: String = seed_str.chars().filter(|c| c.is_ascii_digit()).collect();
                cfg.seed = digits.parse::<u32>().unwrap_or(0);
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
