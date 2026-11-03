// src/ui/mod.rs

mod main_menu;
mod overlays;
mod shell;

use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};

use crate::{
    astronomy::Sun,
    domains::tectonics::GeologyLayer,
    viewport::{Globe, SunVisualConfig},
    world::states::AppState,
};

use main_menu::main_menu_ui;
use overlays::{
    LegendState, generation_progress_overlay, map_legend_overlay, save_load_progress_overlay,
};
pub use shell::ShellLayout;
use shell::render_ui_shell;

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum ActiveTab {
    #[default]
    World,
    Tectonics,
    Climate,
    Hydrology,
    Civilization,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }

        app.init_resource::<ActiveTab>()
            .init_resource::<LegendState>()
            .init_resource::<ShellLayout>()
            .add_systems(
                EguiPrimaryContextPass,
                (
                    render_ui_shell,
                    main_menu_ui.run_if(in_state(AppState::MainMenu)),
                    generation_progress_overlay,
                    save_load_progress_overlay,
                    map_legend_overlay.run_if(in_state(AppState::InGame)),
                )
                    .chain(),
            )
            .add_systems(Update, manage_scene_visibility);
    }
}

fn manage_scene_visibility(
    state: Res<State<AppState>>,
    geology: Res<GeologyLayer>,
    sun_config: Res<SunVisualConfig>,
    mut globe_query: Query<&mut Visibility, (With<Globe>, Without<Sun>)>,
    mut sun_query: Query<&mut Visibility, (With<Sun>, Without<Globe>)>,
) {
    let planet_ready = *state.get() == AppState::InGame && !geology.elevations.is_empty();
    let globe_visibility = if planet_ready {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let sun_visibility = if planet_ready && sun_config.visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    for mut visibility in &mut globe_query {
        if *visibility != globe_visibility {
            *visibility = globe_visibility;
        }
    }
    for mut visibility in &mut sun_query {
        if *visibility != sun_visibility {
            *visibility = sun_visibility;
        }
    }
}
