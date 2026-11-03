// src/domains/climate/mod.rs

mod climate_config;
pub mod climate_data;
pub mod climate_palettes;
pub mod climate_ui;
mod solver;

pub use climate_config::ClimateConfig;
pub use climate_data::{ClimateLayer, ClimateSeason};
pub use solver::{ClimateSolverCache, simulate_all_seasons};

use crate::viewport::MapOverlay;

use crate::{
    astronomy::{AstronomyUpdateSet, OrbitalSeasonState},
    simulation::clock::SimulationClock,
    world::{
        pipeline::{PipelineSet, WorldPipeline},
        states::AppState,
    },
};
use bevy::prelude::*;
use climate_palettes::{draw_ocean_currents_gizmos, draw_wind_arrows_gizmos};

#[derive(Resource, Clone, Copy, Debug)]
pub struct SeasonalControl {
    /// true: use the orbital phase; false: respect ClimateConfig.active_season.
    pub automatic: bool,
}

impl Default for SeasonalControl {
    fn default() -> Self {
        Self { automatic: true }
    }
}

pub struct ClimatePlugin;

impl Plugin for ClimatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClimateConfig>()
            .init_resource::<ClimateLayer>()
            .init_resource::<SeasonalControl>()
            .add_systems(
                Update,
                update_dynamic_seasonal_climate
                    .in_set(PipelineSet::Simulation)
                    .after(AstronomyUpdateSet)
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

fn update_dynamic_seasonal_climate(
    clock: Res<SimulationClock>,
    control: Res<SeasonalControl>,
    season_state: Res<OrbitalSeasonState>,
    time: Res<Time<Real>>,
    overlay: Res<MapOverlay>, // <--- ДОБАВИТЬ
    mut config: ResMut<ClimateConfig>,
    mut climate: ResMut<ClimateLayer>,
    mut pipeline: ResMut<WorldPipeline>,
    mut last_factor: Local<f32>,
    mut interpolate_cooldown: Local<f32>,
) {
    if !control.automatic
        || clock.paused
        || !clock.is_started()
        || climate.spring.temperatures.is_empty()
    {
        return;
    }

    *interpolate_cooldown += time.delta_secs();
    
    // Жёсткий лимит: не чаще 4 раз в секунду
    if *interpolate_cooldown < 1.0 {
        return;
    }
    
    // Обновляем, только если сезон сдвинулся существенно (на 2%)
    if (season_state.season_factor - *last_factor).abs() < 0.1 {
        *interpolate_cooldown = 0.0;
        return;
    }

    *interpolate_cooldown = 0.0;
    *last_factor = season_state.season_factor;
    climate.interpolate_from_orbit(season_state.season_factor);

    let new_season = if season_state.season_factor > 0.45 {
        ClimateSeason::Summer
    } else if season_state.season_factor < -0.45 {
        ClimateSeason::Winter
    } else {
        ClimateSeason::Spring
    };

    let affects_mesh = matches!(
        *overlay,
        MapOverlay::SpaceView
            | MapOverlay::Temperature
            | MapOverlay::Humidity
            | MapOverlay::Precipitation
            | MapOverlay::Albedo
            | MapOverlay::Continentality
            | MapOverlay::Pressure
            | MapOverlay::Winds
    );

    if new_season != config.active_season {
        config.active_season = new_season;
    }

    pipeline.recolor_mesh = true;
}