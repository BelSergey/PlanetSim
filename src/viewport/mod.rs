// src/viewport/mod.rs

pub mod camera_3d;
pub mod hotkeys;
pub mod mesh_globe;
pub mod palettes;

use crate::{
    astronomy::{AxialRotation, Orbit, Sun, Sunlight},
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
                build_crust_age_colors, build_elevation_colors, build_lithology_colors,
                build_plate_colors, build_ruggedness_colors, build_tectonic_stress_colors,
            },
        },
    },
    world::{
        WorldConfig,
        pipeline::{PipelineSet, WorldPipeline},
        topology::WorldTopology,
    },
};
use bevy::{light::NotShadowCaster, prelude::*};
use camera_3d::{
    SystemInertia, keyboard_zoom, reset_zoom, rotate_system, set_default_camera_zoom, zoom_camera,
};
use hotkeys::hotkey_controls;
use mesh_globe::{
    MeshColorLayout, build_globe_mesh, build_river_mesh, compute_color_layout,
    recolor_globe_mesh_range, reposition_globe_mesh,
};
use palettes::build_space_colors;
use std::time::Instant;

/// Визуальная поверхность планеты.
#[derive(Component)]
pub struct Globe;

/// Корневой объект звёздной системы; вращается только пользовательским вводом.
#[derive(Component)]
pub struct StarSystemRoot;

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum ViewMode {
    #[default]
    Globe3D,
    _Flat2D,
}

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
pub struct SunVisualConfig {
    /// Toggles only the visible emissive sphere; DirectionalLight remains active.
    pub visible: bool,
}

impl Default for SunVisualConfig {
    fn default() -> Self {
        Self { visible: true }
    }
}

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

#[derive(Default)]
struct ViewportMeshCache {
    color_layout: Option<MeshColorLayout>,
    /// Quantized colors avoid rewriting vertices for imperceptible float noise.
    last_space_colors_u8: Option<Vec<[u8; 4]>>,
}

pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewMode>()
            .init_resource::<MapOverlay>()
            .init_resource::<ViewportViewConfig>()
            .init_resource::<SunVisualConfig>()
            .add_systems(Startup, setup_scene)
            .add_systems(PostStartup, set_default_camera_zoom)
            .add_systems(
                Update,
                (
                    rotate_system,
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

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let globe_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.85,
        ..default()
    });
    let sun_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.55, 0.12),
        emissive: LinearRgba::rgb(25.0, 10.0, 1.5),
        unlit: true,
        ..default()
    });
    let river_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.35,
        metallic: 0.05,
        ..default()
    });

    let sun_mesh = meshes.add(Sphere::new(1.0).mesh().uv(48, 32));
    let root = commands
        .spawn((
            StarSystemRoot,
            SystemInertia::default(),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();

    commands.entity(root).with_children(|system| {
        system.spawn((
            Mesh3d(sun_mesh),
            MeshMaterial3d(sun_material),
            Sun,
            NotShadowCaster,
            Transform::default(),
            Visibility::Hidden,
        ));

        system
            .spawn((
                Orbit::earth(),
                AxialRotation::earth(),
                Transform::default(),
                Visibility::Inherited,
            ))
            .with_children(|planet| {
                planet
                    .spawn((
                        Mesh3d::default(),
                        MeshMaterial3d(globe_material),
                        Globe,
                        Transform::default(),
                        Visibility::Hidden,
                    ))
                    .with_children(|globe| {
                        globe.spawn((
                            Mesh3d::default(),
                            MeshMaterial3d(river_material),
                            RiverOverlayMesh,
                            Visibility::Hidden,
                        ));
                    });
            });

        system.spawn((
            DirectionalLight {
                illuminance: 12000.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Sunlight,
            Transform::from_xyz(10.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
    });

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
    mut deferred_mesh_drops: Local<Vec<Handle<Mesh>>>,
    mut cache: Local<ViewportMeshCache>,
) {
    deferred_mesh_drops.clear();

    if let Ok((_, mut river_visibility)) = river_query.single_mut() {
        let desired = if view_config.show_rivers && !hydrology.flow_accumulation.is_empty() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *river_visibility != desired {
            *river_visibility = desired;
        }
    }

    if geology.elevations.is_empty() || climate.temperatures.is_empty() {
        return;
    }
    if !pipeline.rebuild_mesh
        && !pipeline.reposition_mesh
        && !pipeline.recolor_mesh
        && !pipeline.rebuild_rivers
    {
        return;
    }

    let height_scale = world_config.height_scale();
    let mut mesh_updated = false;

    if let Ok(mut mesh_handle) = globe_query.single_mut() {
        if pipeline.rebuild_mesh {
            let colors =
                compute_overlay_colors(&overlay, &geology, &climate, &hydrology, &civ, &civ_config);
            let mesh = build_globe_mesh(
                &topology,
                &geology.elevations,
                &geology.is_ocean,
                view_config.flat_water,
                &colors,
                world_config.radius,
                height_scale,
            );
            deferred_mesh_drops.push(mesh_handle.0.clone());
            mesh_handle.0 = meshes.add(mesh);
            cache.color_layout = Some(compute_color_layout(&topology, &geology.elevations));
            cache.last_space_colors_u8 = if *overlay == MapOverlay::SpaceView {
                Some(quantize_colors(&colors))
            } else {
                None
            };
            mesh_updated = true;
        } else {
            // Prepare color data before requesting mutable access to the mesh asset.
            // Assets::get_mut marks an asset as changed, so avoid it when quantized
            // SpaceView colors are identical and there is no position update.
            let mut colors_update: Option<(Vec<[f32; 4]>, Option<Vec<bool>>)> = None;
            let mut current_space_colors: Option<Vec<[u8; 4]>> = None;

            if pipeline.recolor_mesh {
                let colors = compute_overlay_colors(
                    &overlay,
                    &geology,
                    &climate,
                    &hydrology,
                    &civ,
                    &civ_config,
                );

                if *overlay == MapOverlay::SpaceView {
                    let current = quantize_colors(&colors);
                    let dirty = cache.last_space_colors_u8.as_ref().and_then(|previous| {
                        (previous.len() == current.len()).then(|| {
                            previous
                                .iter()
                                .zip(&current)
                                .map(|(a, b)| a != b)
                                .collect::<Vec<_>>()
                        })
                    });
                    let any_dirty = dirty
                        .as_ref()
                        .is_none_or(|mask| mask.iter().any(|&changed| changed));

                    if any_dirty {
                        current_space_colors = Some(current);
                        colors_update = Some((colors, dirty));
                    }
                } else {
                    colors_update = Some((colors, None));
                }
            }

            if pipeline.reposition_mesh || colors_update.is_some() {
                if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
                    if pipeline.reposition_mesh {
                        let positions = reposition_globe_mesh(
                            &topology,
                            &geology.elevations,
                            &geology.is_ocean,
                            view_config.flat_water,
                            world_config.radius,
                            height_scale,
                        );
                        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                        mesh_updated = true;
                    }

                    if let Some((colors, dirty)) = colors_update {
                        if cache.color_layout.is_none() {
                            cache.color_layout =
                                Some(compute_color_layout(&topology, &geology.elevations));
                        }

                        if let Some(layout) = cache.color_layout.as_ref() {
                            recolor_globe_mesh_range(
                                &mut mesh,
                                &topology,
                                &colors,
                                layout,
                                0,
                                topology.cells.len(),
                                dirty.as_deref(),
                            );
                            cache.last_space_colors_u8 = current_space_colors;
                            if *overlay != MapOverlay::SpaceView {
                                cache.last_space_colors_u8 = None;
                            }
                            mesh_updated = true;
                        }
                    }
                }
            }
        }
    }

    if pipeline.rebuild_mesh || pipeline.reposition_mesh || pipeline.rebuild_rivers {
        if let Ok((mut river_handle, _)) = river_query.single_mut() {
            let river_mesh = build_river_mesh(
                &topology,
                &geology,
                &hydrology,
                &hydrology_config,
                view_config.flat_water,
                world_config.radius,
                height_scale,
            );
            deferred_mesh_drops.push(river_handle.0.clone());
            river_handle.0 = meshes.add(river_mesh);
        }
    }

    pipeline.rebuild_mesh = false;
    pipeline.reposition_mesh = false;
    pipeline.recolor_mesh = false;
    pipeline.rebuild_rivers = false;
    let _ = (mesh_updated, Instant::now());
}

fn quantize_colors(colors: &[[f32; 4]]) -> Vec<[u8; 4]> {
    colors
        .iter()
        .map(|c| {
            [
                (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            ]
        })
        .collect()
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
    world_config: Res<WorldConfig>,
    globe_query: Query<&GlobalTransform, With<Globe>>,
) {
    if !view_config.show_axis_equator {
        return;
    }
    let Ok(global_transform) = globe_query.single() else {
        return;
    };
    let transform = global_transform.compute_transform();
    let radius = world_config.radius;
    let top = transform.transform_point(Vec3::Y * (radius + 1.0));
    let bottom = transform.transform_point(Vec3::NEG_Y * (radius + 1.0));
    gizmos.arrow(bottom, top, Color::srgba(1.0, 0.2, 0.2, 0.9));

    let equator_rotation = transform.rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    gizmos.circle(
        Isometry3d::new(transform.translation, equator_rotation),
        radius + 0.05,
        Color::srgba(0.2, 0.8, 1.0, 0.9),
    );
}
