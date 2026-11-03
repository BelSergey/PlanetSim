// src/ui/main_menu.rs

use bevy::{ecs::system::SystemParam, prelude::*};
use bevy_egui::{EguiContext, egui};

use crate::{
    domains::{
        climate::climate_data::ClimateLayer, generator::GenerationRequest,
        hydrology::HydrologyLayer, save::SaveUiState, tectonics::GeologyLayer,
    },
    world::{
        WorldConfig,
        pipeline::{PipelineStage, WorldPipeline},
        states::AppState,
        topology::WorldTopology,
    },
};

/// Все ресурсы, нужные главному меню — одним SystemParam.
#[derive(SystemParam)]
pub struct MainMenuState<'w> {
    next_state: ResMut<'w, NextState<AppState>>,
    gen_req: ResMut<'w, GenerationRequest>,
    save_ui: ResMut<'w, SaveUiState>,
    topology: ResMut<'w, WorldTopology>,
    geology: ResMut<'w, GeologyLayer>,
    climate: ResMut<'w, ClimateLayer>,
    hydrology: ResMut<'w, HydrologyLayer>,
    pipeline: ResMut<'w, WorldPipeline>,
    config: ResMut<'w, WorldConfig>,
}

/// В главном меню поверх центрального вьюпорта висит компактное стартовое окно,
/// а правая/верхняя панели уже полностью доступны для настройки конфигов и загрузки сейвов.
pub fn main_menu_ui(mut contexts: Query<&mut EguiContext>, mut st: MainMenuState) -> Result {
    let Ok(mut egui_ctx) = contexts.single_mut() else {
        return Ok(());
    };

    let target_subdivs = st.config.next_planet_subdivisions;

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
                    st.config.randomize_seed();
                    if st.topology.subdivisions != target_subdivs {
                        *st.topology = WorldTopology::new(target_subdivs);
                    }
                    *st.geology = GeologyLayer::default();
                    *st.climate = ClimateLayer::default();
                    *st.hydrology = HydrologyLayer::default();

                    st.pipeline.rebuild_mesh = true;
                    st.pipeline.recolor_mesh = true;
                    st.pipeline.rebuild_rivers = true;

                    st.gen_req.request(PipelineStage::Tectonics);
                    st.next_state.set(AppState::InGame);
                }

                ui.add_space(6.0);
                if ui
                    .add_sized([260.0, 36.0], egui::Button::new("📂 Загрузить планету..."))
                    .clicked()
                {
                    st.save_ui.refresh_lists();
                    st.save_ui.show_planets_window = true;
                }

                ui.add_space(4.0);
                if ui
                    .add_sized([260.0, 32.0], egui::Button::new("⚙ Пресеты конфигов..."))
                    .clicked()
                {
                    st.save_ui.refresh_lists();
                    st.save_ui.show_configs_window = true;
                }

                ui.add_space(10.0);
            });
        });

    Ok(())
}
