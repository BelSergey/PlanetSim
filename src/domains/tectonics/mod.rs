// src/domains/tectonics/mod.rs

pub mod solver;
mod tectonics_config;
pub mod tectonics_data;
pub mod tectonics_palettes;
pub mod tectonics_ui;

#[allow(unused_imports)]
pub use solver::{
    finalize_tectonics, generate_hotspots, generate_plates, generate_plumes, init_tectonics,
    simulate_tectonics, step_elevation_diffusion, step_tectonics, step_tectonics_realtime,
    TectonicsGeometryCache, TectonicsScratch,
};
pub use tectonics_config::TectonicsConfig;
pub use tectonics_data::GeologyLayer;

use bevy::prelude::*;

pub struct TectonicsPlugin;

impl Plugin for TectonicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TectonicsConfig>()
            .init_resource::<GeologyLayer>();
    }
}