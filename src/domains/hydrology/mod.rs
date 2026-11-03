// src/domains/hydrology/mod.rs

mod hydrology_config;
pub mod hydrology_data;
pub mod hydrology_palettes;
pub mod hydrology_ui;
mod solver;

use crate::world::{pipeline::PipelineSet, states::AppState};
pub use hydrology_config::HydrologyConfig;
pub use hydrology_data::HydrologyLayer;
use hydrology_palettes::draw_river_flow_gizmos;
pub use solver::{apply_surface_feedback_to_climate, simulate_hydrology};

use bevy::prelude::*;

pub struct HydrologyPlugin;

impl Plugin for HydrologyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HydrologyConfig>()
            .init_resource::<HydrologyLayer>()
            .add_systems(
                Update,
                draw_river_flow_gizmos
                    .in_set(PipelineSet::Render)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
