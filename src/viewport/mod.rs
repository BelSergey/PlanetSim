// src/viewport/mod.rs

pub mod camera_3d;
pub mod hotkeys;
pub mod mesh_globe;
pub mod palettes;

use crate::{
    domains::{
        civilization::{
            CivilizationConfig, CivilizationLayer, SocLayerKind,
            civ_palettes::build_civ_layer_colors,
        },
        climate::{
            climate_data::ClimateLayer,
            climate_palettes::{
                build_albedo_colors, build_continentality_colors, build_humidity_colors,
                build_precipitation_colors, build_pressure_colors, build_temperature_colors,
                build_wind_colors,
            },
        },
        hydrology::{
            HydrologyConfig,
            hydrology_data::HydrologyLayer,
            hydrology_palettes::{
                build_bio_resource_colors, build_biome_colors, build_food_potential_colors,
                build_mineral_colors, build_soil_colors, build_soil_moisture_colors,
            },
        },
        tectonics::{
            tectonics_data::GeologyLayer,
            tectonics_palettes::{
                build_elevation_colors, build_lithology_colors, build_plate_colors,
                build_ruggedness_colors, build_crust_age_colors, build_tectonic_stress_colors,
            },
        },
    },
    world::{
        WorldConfig,
        pipeline::{PipelineSet, WorldPipeline},
        topology::WorldTopology,
    },
};
use bevy::prelude::*;
use camera_3d::{
    GlobeInertia, globe_apply_inertia, globe_drag_input, keyboard_zoom, reset_zoom,
    set_default_camera_zoom, zoom_camera,
};
use hotkeys::hotkey_controls;
use mesh_globe::{build_globe_mesh, build_river_mesh, recolor_globe_mesh, reposition_globe_mesh};
use palettes::build_space_colors;

#[derive(Component)]
pub struct Globe;

/// Режим проекции мира (задел под переключение между 3D-сферой и 2D-картой)
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum ViewMode {
    #[default]
    Globe3D,
    _Flat2D,
}

/// Активный слой раскраски карты (переключается по клавише Tab или из UI)
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum MapOverlay {
    #[default]
    SpaceView,
    Elevation,
    TectonicPlates,
    Continentality,
    Lithology,
    Ruggedness,
    CrustAge,
    TectonicStress,
    Temperature,
    Humidity,
    Precipitation,
    Albedo,
    Pressure,
    Winds,
    Soils,
    SoilMoisture,
    Biomes,
    FoodPotential,
    Minerals,
    BioResources,
    CivSpecies,
    CivKinship,
    CivCulture,
    CivReligion,
    CivEconomy,
    CivPolity,
    CivKnowledge,
}

#[derive(Component)]
pub struct RiverOverlayMesh;

#[derive(Resource)]
pub struct ViewportViewConfig {
    pub flat_water: bool,
    pub show_rivers: bool,
    pub show_wind_arrows: bool,
    pub show_ocean_currents: bool,
    pub show_river_flow: bool,
    pub show_axis_equator: bool,
}

impl Default for ViewportViewConfig {
    fn default() -> Self {
        Self {
            flat_water: true,
            show_rivers: true,
            show_wind_arrows: false,
            show_ocean_currents: false,
            show_river_flow: false,
            show_axis_equator: false,
        }
    }
}

pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewMode>()
            .init_resource::<MapOverlay>()
            .init_resource::<ViewportViewConfig>()
            .add_systems(PostStartup, set_default_camera_zoom)
            .add_systems(Startup, setup_scene)
            .add_systems(
                Update,
                (
                    globe_drag_input,
                    globe_apply_inertia,
                    zoom_camera,
                    hotkey_controls,
                    keyboard_zoom,
                    reset_zoom,
                )
                    .in_set(PipelineSet::Input),
            )
            .add_systems(Update, sync_viewport_mesh.in_set(PipelineSet::Render))
            .add_systems(
                Update,
                draw_axis_and_equator_gizmos.in_set(PipelineSet::Render),
            );
    }
}

fn setup_scene(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let material_handle = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.85,
        ..default()
    });

    let river_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.35,
        metallic: 0.05,
        ..default()
    });

    // Используем Mesh3d::default() без аллокации временных сфер/пустых мешей в GPU-буфере
    commands
        .spawn((
            Mesh3d::default(),
            MeshMaterial3d(material_handle),
            Globe,
            GlobeInertia::default(),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d::default(),
                MeshMaterial3d(river_material),
                RiverOverlayMesh,
                Visibility::Hidden,
            ));
        });

    commands.spawn((
        DirectionalLight {
            illuminance: 12000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(10.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((Camera3d::default(), Transform::from_xyz(0.0, 0.0, 9.5)));
}

#[allow(clippy::too_many_arguments)]
fn sync_viewport_mesh(
    mut pipeline: ResMut<WorldPipeline>,
    topology: Res<WorldTopology>,
    world_config: Res<WorldConfig>,
    geology: Res<GeologyLayer>,
    climate: Res<ClimateLayer>,
    hydrology: Res<HydrologyLayer>,
    hydrology_config: Res<HydrologyConfig>,
    civ: Res<CivilizationLayer>,
    civ_config: Res<CivilizationConfig>,
    overlay: Res<MapOverlay>,
    view_config: Res<ViewportViewConfig>,
    mut globe_query: Query<&mut Mesh3d, (With<Globe>, Without<RiverOverlayMesh>)>,
    mut river_query: Query<
        (&mut Mesh3d, &mut Visibility),
        (With<RiverOverlayMesh>, Without<Globe>),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    // Задерживает удаление старого меша на 1 кадр, чтобы slab_allocator не делал free + grow одновременно
    mut deferred_mesh_drops: Local<Vec<Handle<Mesh>>>,
) {
    // Освобождаем старые меши с прошлого кадра, когда расширение GPU-буфера уже завершено
    deferred_mesh_drops.clear();

    let height_scale = world_config.height_scale();

    if let Ok((_, mut river_vis)) = river_query.single_mut() {
        let target_vis = if view_config.show_rivers && !hydrology.flow_accumulation.is_empty() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *river_vis != target_vis {
            *river_vis = target_vis;
        }
    }

    if !pipeline.rebuild_mesh
        && !pipeline.reposition_mesh
        && !pipeline.recolor_mesh
        && !pipeline.rebuild_rivers
    {
        return;
    }
    if geology.elevations.is_empty() || climate.temperatures.is_empty() {
        return;
    }

    // 1. Обновление меша глобуса
    if let Ok(mut mesh_handle) = globe_query.single_mut() {
        let flatten_water = view_config.flat_water;

        if pipeline.rebuild_mesh {
            let colors =
                compute_overlay_colors(&overlay, &geology, &climate, &hydrology, &civ, &civ_config);
            let new_mesh = build_globe_mesh(
                &topology,
                &geology.elevations,
                &geology.is_ocean,
                flatten_water,
                &colors,
                world_config.radius,
                height_scale,
            );
            deferred_mesh_drops.push(mesh_handle.0.clone());
            mesh_handle.0 = meshes.add(new_mesh);
        } else if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
            if pipeline.reposition_mesh {
                let new_positions = reposition_globe_mesh(
                    &topology,
                    &geology.elevations,
                    &geology.is_ocean,
                    flatten_water,
                    world_config.radius,
                    height_scale,
                );
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, new_positions);
            }

            if pipeline.recolor_mesh {
                let colors = compute_overlay_colors(
                    &overlay,
                    &geology,
                    &climate,
                    &hydrology,
                    &civ,
                    &civ_config,
                );
                let vertex_colors = recolor_globe_mesh(&topology, &geology.elevations, &colors);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vertex_colors);
            }
        }
    }

    // 2. Обновление меша речной сети
    if pipeline.rebuild_mesh || pipeline.reposition_mesh || pipeline.rebuild_rivers {
        if let Ok((mut river_mesh_handle, _)) = river_query.single_mut() {
            let new_river_mesh = build_river_mesh(
                &topology,
                &geology,
                &hydrology,
                &hydrology_config,
                view_config.flat_water,
                world_config.radius,
                height_scale,
            );
            deferred_mesh_drops.push(river_mesh_handle.0.clone());
            river_mesh_handle.0 = meshes.add(new_river_mesh);
        }
    }

    pipeline.rebuild_mesh = false;
    pipeline.reposition_mesh = false;
    pipeline.recolor_mesh = false;
    pipeline.rebuild_rivers = false;
}

fn compute_overlay_colors(
    overlay: &MapOverlay,
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &HydrologyLayer,
    civ: &CivilizationLayer,
    civ_config: &CivilizationConfig,
) -> Vec<[f32; 4]> {
    match *overlay {
        MapOverlay::SpaceView => build_space_colors(geology, climate, hydrology),
        MapOverlay::Elevation => build_elevation_colors(geology),
        MapOverlay::TectonicPlates => build_plate_colors(geology),
        MapOverlay::Continentality => build_continentality_colors(climate),
        MapOverlay::Lithology => build_lithology_colors(geology),
        MapOverlay::Ruggedness => build_ruggedness_colors(geology),
        MapOverlay::CrustAge => build_crust_age_colors(geology),
        MapOverlay::TectonicStress => build_tectonic_stress_colors(geology),
        MapOverlay::Temperature => build_temperature_colors(climate),
        MapOverlay::Humidity => build_humidity_colors(climate),
        MapOverlay::Precipitation => build_precipitation_colors(climate),
        MapOverlay::Albedo => build_albedo_colors(climate),
        MapOverlay::Pressure => build_pressure_colors(climate),
        MapOverlay::Winds => build_wind_colors(climate),
        MapOverlay::Soils => build_soil_colors(hydrology),
        MapOverlay::SoilMoisture => build_soil_moisture_colors(geology, hydrology),
        MapOverlay::Biomes => build_biome_colors(hydrology),
        MapOverlay::FoodPotential => build_food_potential_colors(geology, hydrology),
        MapOverlay::Minerals => build_mineral_colors(geology, hydrology),
        MapOverlay::BioResources => build_bio_resource_colors(geology, hydrology),
        MapOverlay::CivSpecies => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Species)
        }
        MapOverlay::CivKinship => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Kinship)
        }
        MapOverlay::CivCulture => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Culture)
        }
        MapOverlay::CivReligion => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Religion)
        }
        MapOverlay::CivEconomy => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Economy)
        }
        MapOverlay::CivPolity => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Polity)
        }
        MapOverlay::CivKnowledge => {
            build_civ_layer_colors(geology, civ, civ_config, SocLayerKind::Knowledge)
        }
    }
}

pub fn draw_axis_and_equator_gizmos(
    mut gizmos: Gizmos,
    view_config: Res<ViewportViewConfig>,
    world_config: Res<crate::world::WorldConfig>,
    globe_query: Query<&Transform, With<Globe>>,
) {
    if !view_config.show_axis_equator {
        return;
    }

    let Ok(globe_transform) = globe_query.single() else {
        return;
    };

    let radius = world_config.radius;

    // 1. Рисуем ось вращения (от Южного полюса к Северному)
    let local_top = Vec3::Y * (radius + 1.0);
    let local_bottom = Vec3::NEG_Y * (radius + 1.0);

    let world_top = globe_transform.transform_point(local_top);
    let world_bottom = globe_transform.transform_point(local_bottom);

    gizmos.arrow(world_bottom, world_top, Color::srgba(1.0, 0.2, 0.2, 0.9));

    // 2. Рисуем Экватор
    let equator_radius = radius + 0.05;

    // Поворачиваем плоскость круга (по умолчанию XY) на -90 градусов по X, чтобы получить экватор (XZ)
    let equator_rotation =
        globe_transform.rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);

    gizmos.circle(
        Isometry3d::new(globe_transform.translation, equator_rotation),
        equator_radius,
        Color::srgba(0.2, 0.8, 1.0, 0.9),
    );
}
