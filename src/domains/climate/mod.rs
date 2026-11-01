// src/domains/climate/mod.rs

pub mod climate_config;
pub mod climate_data;
pub mod climate_palettes;
pub mod climate_ui;
pub mod solver;

pub use climate_config::ClimateConfig;
pub use climate_data::{ClimateLayer, ClimateSeason};
pub use solver::simulate_all_seasons;

use crate::astronomy::OrbitalSeasonState;
use crate::simulation::clock::SimulationClock;
use crate::world::{
    pipeline::{PipelineSet, WorldPipeline},
    states::AppState,
};
use bevy::prelude::*;
use climate_palettes::{draw_ocean_currents_gizmos, draw_wind_arrows_gizmos};

pub struct ClimatePlugin;

impl Plugin for ClimatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClimateConfig>()
            .init_resource::<ClimateLayer>()
            .add_systems(
                Update,
                update_dynamic_seasonal_climate
                    .in_set(PipelineSet::Simulation)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (draw_wind_arrows_gizmos, draw_ocean_currents_gizmos)
                    .in_set(PipelineSet::Render)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

pub fn update_dynamic_seasonal_climate(
    clock: Res<SimulationClock>,
    season_state: Res<OrbitalSeasonState>,
    time: Res<Time>,
    mut config: ResMut<ClimateConfig>,
    mut climate: ResMut<ClimateLayer>,
    mut pipeline: ResMut<WorldPipeline>,      // ← снова нужен
    mut last_factor: Local<f32>,
    mut interpolate_cooldown: Local<f32>,
) {
    if clock.paused || climate.spring.temperatures.is_empty() {
        return;
    }

    let dt = time.delta_secs();
    *interpolate_cooldown += dt;

    if (season_state.season_factor - *last_factor).abs() < 0.015 {
        return;
    }
    if *interpolate_cooldown < 0.08 {
        return;
    }
    *interpolate_cooldown = 0.0;
    *last_factor = season_state.season_factor;

    // Данные климата продолжают плавно интерполироваться (температуры, давления, ветра).
    climate.interpolate_from_orbit(season_state.season_factor);

    let new_season = if season_state.season_factor > 0.45 {
        ClimateSeason::Summer
    } else if season_state.season_factor < -0.45 {
        ClimateSeason::Winter
    } else {
        ClimateSeason::Spring
    };

    // А вот цвета вершин обновляем ТОЛЬКО на переходе сезона.
    if new_season != config.active_season {
        config.active_season = new_season;
        pipeline.recolor_mesh = true;
    }
}