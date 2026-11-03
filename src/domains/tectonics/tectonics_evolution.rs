// src/domains/tectonics/tectonics_evolution.rs
use bevy::prelude::*;

use crate::{
    domains::{
        generator::{GenerationRequest, GenerationTask},
        tectonics::GeologyLayer,
    },
    simulation::clock::SimulationClock,
    world::pipeline::PipelineStage,
};

#[derive(Resource, Clone)]
pub struct TectonicEvolutionConfig {
    pub enabled: bool,
    /// Реальные секунды между эпохами.
    pub epoch_real_seconds: f32,
    /// Количество физических шагов новой модели за эпоху.
    pub steps_per_epoch: u32,
}

impl Default for TectonicEvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            epoch_real_seconds: 180.0,
            steps_per_epoch: 2,
        }
    }
}

#[derive(Resource, Default)]
pub struct TectonicEvolutionState {
    pub accumulated_real_seconds: f32,
    pub epochs: u32,
}

pub fn tick_tectonic_evolution(
    time: Res<Time<Real>>,
    clock: Res<SimulationClock>,
    config: Res<TectonicEvolutionConfig>,
    geology: Res<GeologyLayer>,
    mut state: ResMut<TectonicEvolutionState>,
    mut gen_req: ResMut<GenerationRequest>,
    active_tasks: Query<(), With<GenerationTask>>,
) {
    if !config.enabled || clock.paused || !clock.is_started() {
        state.accumulated_real_seconds = 0.0;
        return;
    }

    if !active_tasks.is_empty() || gen_req.is_pending() {
        return;
    }

    if geology.elevations.is_empty() || geology.plates.len() < 3 {
        return;
    }

    state.accumulated_real_seconds += time.delta_secs();
    let interval = config.epoch_real_seconds.max(1.0);

    if state.accumulated_real_seconds >= interval {
        // Do not enqueue catch-up epochs: one generation task at a time.
        state.accumulated_real_seconds = 0.0;
        state.epochs = state.epochs.wrapping_add(1);
        info!(
            "Тектоническая эпоха №{}: физическое движение плит",
            state.epochs
        );
        gen_req.request_evolve(PipelineStage::Tectonics);
    }
}
