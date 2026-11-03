mod astronomy;
mod domains;
mod simulation;
mod ui;
mod viewport;
mod world;

use astronomy::AstronomyPlugin;
use bevy::prelude::*;
use domains::DomainsPlugin;
use simulation::clock::SimulationClockPlugin;
use thread_priority::{ThreadPriority, set_current_thread_priority};
use ui::UiPlugin;
use viewport::ViewportPlugin;
use world::WorldPlugin;

fn report_frame_spikes(time: Res<Time<Real>>, mut cooldown: Local<f64>) {
    let dt = time.delta_secs_f64();
    *cooldown = (*cooldown - dt).max(0.0);

    if dt >= 0.020 && *cooldown <= 0.0 {
        warn!("[FRAME SPIKE] frame_time={:.2}ms", dt * 1000.0);
        *cooldown = 0.25;
    }
}

fn main() {
    let total_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Reserve capacity for the main game loop/audio; background work runs at low priority.
    let worker_threads = total_threads.saturating_sub(2).max(1);

    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_threads)
        .thread_name(|i| format!("planet-gen-{i}"))
        .start_handler(|_| {
            let _ = set_current_thread_priority(ThreadPriority::Min);
        })
        .build_global();

    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins((
            WorldPlugin,
            DomainsPlugin,
            AstronomyPlugin,
            ViewportPlugin,
            UiPlugin,
            SimulationClockPlugin,
        ))
        .add_systems(Update, report_frame_spikes)
        .run();
}
