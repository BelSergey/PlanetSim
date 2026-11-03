// src/domains/tectonics/mod.rs

pub mod solver;
mod tectonics_config;
pub mod tectonics_data;
pub mod tectonics_evolution;
pub mod tectonics_palettes;
pub mod tectonics_ui;

pub use solver::{
    TectonicsGeometryCache, TectonicsScratch, finalize_tectonics, generate_hotspots,
    generate_plates, prepare_tectonics_scratch, simulate_tectonics, step_tectonics,
};
pub use tectonics_config::TectonicsConfig;
pub use tectonics_data::GeologyLayer;
pub use tectonics_evolution::{
    TectonicEvolutionConfig, TectonicEvolutionState, tick_tectonic_evolution,
};

use crate::{simulation::clock::SimulationClockSet, world::pipeline::PipelineSet};
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
                tick_tectonic_evolution
                    .in_set(PipelineSet::Simulation)
                    .after(SimulationClockSet::Advance),
            );
    }
}
