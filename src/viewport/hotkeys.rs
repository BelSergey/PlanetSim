// src/viewport/hotkeys.rs

use crate::{
    domains::generator::GenerationRequest,
    viewport::MapOverlay,
    world::pipeline::{PipelineStage, WorldPipeline},
};
use bevy::prelude::*;

pub fn hotkey_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut pipeline: ResMut<WorldPipeline>,
    mut gen_req: ResMut<GenerationRequest>,
    mut overlay: ResMut<MapOverlay>,
) {
    // R — полный асинхронный пересчет планеты с нуля
    if keys.just_pressed(KeyCode::KeyR) {
        gen_req.request(PipelineStage::Tectonics);
    }

    // Tab — переключение слоев карты по кругу
    if keys.just_pressed(KeyCode::Tab) {
        let was_space = *overlay == MapOverlay::SpaceView;
        *overlay = match *overlay {
            MapOverlay::SpaceView => MapOverlay::Elevation,
            MapOverlay::Elevation => MapOverlay::TectonicPlates,
            MapOverlay::TectonicPlates => MapOverlay::Lithology,
            MapOverlay::Lithology => MapOverlay::Temperature,
            MapOverlay::Temperature => MapOverlay::Humidity,
            MapOverlay::Humidity => MapOverlay::Precipitation,
            MapOverlay::Precipitation => MapOverlay::Albedo,
            MapOverlay::Albedo => MapOverlay::Continentality,
            MapOverlay::Continentality => MapOverlay::Pressure,
            MapOverlay::Pressure => MapOverlay::Winds,
            MapOverlay::Winds => MapOverlay::Soils,
            MapOverlay::Soils => MapOverlay::SoilMoisture,
            MapOverlay::SoilMoisture => MapOverlay::Biomes,
            MapOverlay::Biomes => MapOverlay::SpaceView,
            MapOverlay::Ruggedness => MapOverlay::Temperature,
        };
        let is_space = *overlay == MapOverlay::SpaceView;

        if was_space != is_space {
            pipeline.reposition_mesh = true;
        }
        pipeline.recolor_mesh = true;
    }
}
