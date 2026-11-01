// src/world/mod.rs

pub mod pipeline;
pub mod states;
pub mod topology;

use bevy::prelude::*;
use pipeline::{PipelineSet, PipelineStage, WorldPipeline};
use rand::RngExt;
use states::AppState;
use topology::WorldTopology;

use crate::domains::{
    climate::climate_data::ClimateLayer, generator::GenerationRequest, hydrology::HydrologyLayer,
    tectonics::GeologyLayer,
};

#[derive(Resource, Clone)]
pub struct WorldConfig {
    /// Радиус планеты в мировых координатах (live, применяется к текущей планете)
    pub radius: f32,
    /// Размерность, с которой будет создана СЛЕДУЮЩАЯ планета.
    /// Размерность ТЕКУЩЕЙ планеты хранится в `WorldTopology.subdivisions`.
    pub next_planet_subdivisions: u32,
    /// Глобальный сид генерации мира (тектоника, ветровые волны, меандры рек, экотоны биомов)
    pub seed: u32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        let mut rng = rand::rng();
        Self {
            radius: 5.0,                 // release - 7
            next_planet_subdivisions: 7, // release - 8
            seed: rng.random_range(0..=u32::MAX),
        }
    }
}

impl WorldConfig {
    pub fn randomize_seed(&mut self) {
        self.seed = rand::rng().random_range(0..=u32::MAX);
    }
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldConfig>()
            .init_resource::<WorldPipeline>()
            .init_state::<AppState>()
            .configure_sets(
                Update,
                (
                    PipelineSet::Input,
                    PipelineSet::Simulation,
                    PipelineSet::Render,
                )
                    .chain(),
            )
            .add_systems(PreStartup, setup_topology);
    }
}

fn setup_topology(mut commands: Commands, config: Res<WorldConfig>) {
    let topology = WorldTopology::new(config.next_planet_subdivisions);
    commands.insert_resource(topology);
}

/// Атомарно: пересоздаёт топологию (если размерность отличается),
/// сбрасывает слои, помечает меш к пересборке, ставит запрос на генерацию
/// и переключает состояние в `InGame`.
///
/// Вызывается из эксклюзивных систем (например, из `render_ui_shell`).
pub fn start_new_planet(world: &mut World, subdivisions: u32) {
    let current = world.resource::<WorldTopology>().subdivisions;
    if current != subdivisions {
        info!(
            "Пересборка топологии: {} -> {} subdiv",
            current, subdivisions
        );
        *world.resource_mut::<WorldTopology>() = WorldTopology::new(subdivisions);
    }

    *world.resource_mut::<GeologyLayer>() = GeologyLayer::default();
    *world.resource_mut::<ClimateLayer>() = ClimateLayer::default();
    *world.resource_mut::<HydrologyLayer>() = HydrologyLayer::default();

    let mut pipeline = world.resource_mut::<WorldPipeline>();
    pipeline.rebuild_mesh = true;
    pipeline.recolor_mesh = true;
    pipeline.rebuild_rivers = true;

    world
        .resource_mut::<GenerationRequest>()
        .request(PipelineStage::Tectonics);
    world
        .resource_mut::<NextState<AppState>>()
        .set(AppState::InGame);
}
