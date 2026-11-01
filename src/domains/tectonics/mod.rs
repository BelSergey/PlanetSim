// src/domains/tectonics/mod.rs
mod tectonics_config;
pub mod tectonics_data;
pub mod tectonics_evolution;
pub mod tectonics_palettes;
mod tectonics_solver;
pub mod tectonics_ui;

pub use tectonics_config::TectonicsConfig;
pub use tectonics_data::GeologyLayer;
pub use tectonics_evolution::{
    TectonicEvolutionConfig, TectonicEvolutionState, tick_tectonic_evolution,
};
pub use tectonics_solver::{generate_hotspots, generate_plates, simulate_tectonics};

use crate::world::pipeline::PipelineSet;
use bevy::prelude::*;

pub struct TectonicsPlugin;

impl Plugin for TectonicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TectonicsConfig>()
            .init_resource::<GeologyLayer>()
            .init_resource::<TectonicEvolutionConfig>()
            .init_resource::<TectonicEvolutionState>()
            .add_systems(
                Update,
                tick_tectonic_evolution.in_set(PipelineSet::Simulation),
            );
    }
}
