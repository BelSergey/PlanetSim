// src/domains/civilization/mod.rs

pub mod civ_config;
pub mod civ_data;
pub mod civ_palettes;
pub mod civ_ui;
pub mod solver;

use bevy::prelude::*;

pub use civ_config::{CivilizationConfig};
pub use civ_data::{CivilizationLayer, SocLayerKind};
pub use solver::{prepare_civilization_substrate, step_civilization_simulation};

use crate::{
    domains::{
        climate::climate_data::ClimateLayer, hydrology::HydrologyLayer, tectonics::GeologyLayer,
    },
    world::{
        WorldConfig,
        pipeline::{PipelineSet, WorldPipeline},
        states::AppState,
        topology::WorldTopology,
    },
};

pub struct CivilizationPlugin;

impl Plugin for CivilizationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CivilizationConfig>()
            .init_resource::<CivilizationLayer>()
            .add_systems(
                Update,
                run_civilization_ticks
                    .in_set(PipelineSet::Simulation)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn run_civilization_ticks(
    config: Res<CivilizationConfig>,
    world_config: Res<WorldConfig>,
    topology: Res<WorldTopology>,
    geology: Res<GeologyLayer>,
    climate: Res<ClimateLayer>,
    hydrology: Res<HydrologyLayer>,
    mut civ: ResMut<CivilizationLayer>,
    mut pipeline: ResMut<WorldPipeline>,
) {
    if geology.elevations.is_empty() || hydrology.biomes.is_empty() {
        return;
    }

    // Автоматически инициализируем подложку цивилизаций, если она еще пуста
    // (например, после загрузки сейва или первого старта)
    if civ.cells.len() != geology.elevations.len()
        || civ.flat_neighbors.len() != geology.elevations.len()
    {
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

    if !config.is_running {
        return;
    }

    let steps = config.steps_per_frame.clamp(1, 30);
    for _ in 0..steps {
        step_civilization_simulation(&geology, &hydrology, &mut civ, &config, world_config.seed);
    }
    pipeline.recolor_mesh = true;
}
