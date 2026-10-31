// src/domains/tectonics/tectonics_evolution.rs
use bevy::prelude::*;

use crate::{
    domains::generator::{GenerationRequest, GenerationTask},
    simulation::clock::SimulationClock,
    world::pipeline::PipelineStage,
};

#[derive(Resource, Clone)]
pub struct TectonicEvolutionConfig {
    pub enabled: bool,
    /// Реальных секунд между эпохами (не может быть меньше, чем длится генерация).
    pub epoch_real_seconds: f32,
    /// Угол дрейфа центра плиты за одну эпоху (умножается на angular_vel плиты).
    pub rotation_per_epoch: f32,
}

impl Default for TectonicEvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            epoch_real_seconds: 180.0,
            rotation_per_epoch: 0.04,
        }
    }
}

#[derive(Resource, Default)]
pub struct TectonicEvolutionState {
    pub accumulated_real_seconds: f32,
    pub epochs: u32,
}

pub fn tick_tectonic_evolution(
    time: Res<Time>,
    clock: Res<SimulationClock>,
    config: Res<TectonicEvolutionConfig>,
    mut state: ResMut<TectonicEvolutionState>,
    mut gen_req: ResMut<GenerationRequest>,
    active_tasks: Query<(), With<GenerationTask>>,
) {
    if !config.enabled || clock.paused {
        return;
    }
    if !active_tasks.is_empty() {
        return;
    }
    state.accumulated_real_seconds += time.delta_secs();
    if state.accumulated_real_seconds >= config.epoch_real_seconds {
        state.accumulated_real_seconds = 0.0;
        state.epochs += 1;
        info!("🌋 Эпоха №{}: дрейф плит", state.epochs);
        gen_req.request_evolve(PipelineStage::Tectonics);
    }
}
