use bevy::prelude::*;

use crate::domains::tectonics::tectonics_data::GeologyLayer;
use crate::world::states::AppState;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimulationClockSet {
    Advance,
}

#[derive(Resource, Debug, Clone)]
pub struct SimulationClock {
    /// Текущее модельное время в секундах.
    pub elapsed_seconds: f64,

    /// Модельных секунд за одну реальную секунду.
    pub time_scale: f64,

    pub paused: bool,

    /// Запускались ли часы после создания планеты.
    started: bool,
}

impl Default for SimulationClock {
    fn default() -> Self {
        Self {
            elapsed_seconds: 0.0,
            time_scale: 86_400.0,
            paused: true,
            started: false,
        }
    }
}

impl SimulationClock {
    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn advance(&mut self, real_delta_seconds: f64) {
        if !self.paused {
            self.elapsed_seconds += real_delta_seconds * self.time_scale;
        }
    }

    // pub fn set_speed(&mut self, seconds_per_real_second: f64) {
    //     self.time_scale = seconds_per_real_second.max(0.0);
    // }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }
}

pub struct SimulationClockPlugin;

impl Plugin for SimulationClockPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimulationClock>()
            .add_systems(
                Update,
                sync_clock_with_planet.before(SimulationClockSet::Advance),
            )
            .add_systems(
                Update,
                advance_simulation_clock.in_set(SimulationClockSet::Advance),
            );
    }
}

fn sync_clock_with_planet(
    state: Res<State<AppState>>,
    geology: Res<GeologyLayer>,
    mut clock: ResMut<SimulationClock>,
) {
    let planet_ready = *state.get() == AppState::InGame && !geology.elevations.is_empty();

    // Планеты ещё нет или началась генерация новой.
    if !planet_ready {
        if clock.started {
            clock.elapsed_seconds = 0.0;
            clock.started = false;

            info!("Simulation clock reset: waiting for planet.");
        }

        // Не записываем paused=true каждый кадр: это лишний change tick ресурса.
        if !clock.paused {
            clock.paused = true;
        }

        return;
    }

    // Планета готова — запускаем часы ровно один раз.
    if !clock.started {
        clock.elapsed_seconds = 0.0;
        clock.paused = false;
        clock.started = true;

        info!("Planet is ready. Simulation clock started!");
    }
}

fn advance_simulation_clock(time: Res<Time>, mut clock: ResMut<SimulationClock>) {
    // Важно проверить паузу до вызова метода через ResMut:
    // иначе ресурс помечался изменённым даже при остановленном времени.
    if !clock.paused {
        clock.advance(time.delta_secs_f64());
    }
}
