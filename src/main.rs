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
use ui::UiPlugin;
use viewport::ViewportPlugin;
use world::WorldPlugin;

use thread_priority::{ThreadPriority, set_current_thread_priority};

fn main() {
    let total_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Оставляем 2 потока под главный цикл игры и звук.
    let worker_threads = total_threads.saturating_sub(2).max(1);

    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_threads)
        .thread_name(|i| format!("planet-gen-{}", i))
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
        .run();
}
