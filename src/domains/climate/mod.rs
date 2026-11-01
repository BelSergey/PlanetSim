// src/domains/climate/mod.rs

mod climate_config;
pub mod climate_data;
pub mod climate_palettes;
pub mod climate_ui;
mod solver;

pub use climate_config::ClimateConfig;
pub use climate_data::ClimateLayer;
pub use solver::simulate_all_seasons;

use crate::world::{pipeline::PipelineSet, states::AppState};
use bevy::prelude::*;
use climate_palettes::{draw_ocean_currents_gizmos, draw_wind_arrows_gizmos};

pub struct ClimatePlugin;

impl Plugin for ClimatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClimateConfig>()
            .init_resource::<ClimateLayer>()
            .add_systems(
                Update,
                (draw_wind_arrows_gizmos, draw_ocean_currents_gizmos)
                    .in_set(PipelineSet::Render)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
