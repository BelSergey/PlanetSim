// src/viewport/mod.rs

pub mod camera_3d;
pub mod hotkeys;
pub mod mesh_globe;

use crate::astronomy::{AxialRotation, Orbit, Sun, Sunlight};

use crate::{
    domains::{
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
            hydrology_data::{Biome, HydrologyLayer},
            hydrology_palettes::{
                biome_satellite_rgb, build_biome_colors, build_soil_colors,
                build_soil_moisture_colors,
            },
        },
        tectonics::{
            tectonics_data::GeologyLayer,
            tectonics_palettes::{
                build_elevation_colors, build_lithology_colors, build_plate_colors,
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
    SystemInertia, keyboard_zoom, reset_zoom, rotate_system, set_default_camera_zoom, zoom_camera,
};
use hotkeys::hotkey_controls;
use mesh_globe::{build_globe_mesh, build_river_mesh, recolor_globe_mesh, reposition_globe_mesh};

/// Визуальная поверхность планеты.
#[derive(Component)]
pub struct Globe;
/// Корневой объект всей звёздной системы.
/// Вращение этого объекта поворачивает Солнце, Землю и освещение вместе.
#[derive(Component)]
pub struct StarSystemRoot;

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
    Lithology,
    Temperature,
    Humidity,
    Precipitation,
    Albedo,
    Continentality,
    Pressure,
    Winds,
    Soils,
    SoilMoisture,
    Biomes,
}

#[derive(Component)]
pub struct RiverOverlayMesh;

#[derive(Resource)]
pub struct ViewportViewConfig {
    pub flat_water: bool,
    pub show_rivers: bool,
    pub show_wind_arrows: bool,
    pub show_ocean_currents: bool,
    pub show_axis_equator: bool,
}

impl Default for ViewportViewConfig {
    fn default() -> Self {
        Self {
            flat_water: true,
            show_rivers: true,
            show_wind_arrows: false,
            show_ocean_currents: false,
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
    let material_handle = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.85,
        ..default()
    });

    // Геометрия Солнца
    let sun_mesh = meshes.add(Sphere::new(1.0).mesh().uv(48, 32));

    // Светящаяся поверхность Солнца
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

    // Общий родитель всей звёздной системы.
    let system_root = commands
        .spawn((
            StarSystemRoot,
            SystemInertia::default(),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();

    // Все объекты системы становятся дочерними одного корня.
    commands.entity(system_root).with_children(|system| {
        // Солнце в центре системы.
        system.spawn((
            Mesh3d(sun_mesh),
            MeshMaterial3d(sun_material),
            Sun,
            Transform::default(),
            Visibility::Hidden,
        ));

        // Физическая система координат Земли.
        // Орбита и осевое вращение рассчитываются независимо
        // от ручного поворота всей системы.
        system
            .spawn((
                Orbit::earth(),
                AxialRotation::earth(),
                Transform::default(),
                Visibility::Inherited,
            ))
            .with_children(|parent| {
                // Поверхность Земли.
                parent
                    .spawn((
                        Mesh3d::default(),
                        MeshMaterial3d(material_handle),
                        Globe,
                        Transform::default(),
                        Visibility::Hidden,
                    ))
                    .with_children(|globe| {
                        // Реки остаются дочерним объектом поверхности.
                        globe.spawn((
                            Mesh3d::default(),
                            MeshMaterial3d(river_material),
                            RiverOverlayMesh,
                            Visibility::Hidden,
                        ));
                    });
            });

        // Солнечный свет также принадлежит общей системе.
        system.spawn((
            DirectionalLight {
                illuminance: 12000.0,
                shadow_maps_enabled: true,
                ..default()
            },
            Sunlight,
            Transform::from_xyz(10.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
    });

    // Камера остаётся вне системы:
    // вращение объектов не вращает саму камеру.
    commands.spawn((Camera3d::default(), Transform::from_xyz(0.0, 0.0, 9.5)));
}

pub fn build_space_colors(
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &HydrologyLayer,
) -> Vec<[f32; 4]> {
    geology
        .elevations
        .iter()
        .enumerate()
        .map(|(i, &h)| {
            let albedo = climate.albedo[i];
            let biome = hydrology.biomes.get(i).copied().unwrap_or(Biome::DeepOcean);

            let (base_r, base_g, base_b, base_albedo) = if h <= 0.0 {
                if biome == Biome::SeaIce {
                    (0.88, 0.94, 0.99, 0.75)
                } else {
                    let depth = (h / -0.35).clamp(0.0, 1.0);
                    let r = 0.02 + (1.0 - depth) * 0.04;
                    let g = 0.10 + (1.0 - depth) * 0.18;
                    let b = 0.28 + (1.0 - depth) * 0.25;
                    (r, g, b, 0.06)
                }
            } else {
                let (r, g, b, b_alb) = biome_satellite_rgb(biome);
                // Легкое высотное тонирование рельефа
                let elev_shade = (1.0 - (h - 0.10).max(0.0) * 0.35).clamp(0.75, 1.0);
                (r * elev_shade, g * elev_shade, b * elev_shade, b_alb)
            };

            // Сезонный снежный покров и морской лед из текущего сезона климата
            let snow_cover =
                ((albedo - base_albedo) / (0.80 - base_albedo).max(0.1)).clamp(0.0, 1.0);

            let r = base_r * (1.0 - snow_cover) + 0.92 * snow_cover;
            let g = base_g * (1.0 - snow_cover) + 0.96 * snow_cover;
            let b = base_b * (1.0 - snow_cover) + 1.00 * snow_cover;

            Color::srgb(r, g, b).to_linear().to_f32_array()
        })
        .collect()
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
            let colors = compute_overlay_colors(&overlay, &geology, &climate, &hydrology);
            let new_mesh = build_globe_mesh(
                &topology,
                &geology.elevations,
                flatten_water,
                &colors,
                world_config.radius,
            );
            deferred_mesh_drops.push(mesh_handle.0.clone());
            mesh_handle.0 = meshes.add(new_mesh);
        } else if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
            if pipeline.reposition_mesh {
                let new_positions = reposition_globe_mesh(
                    &topology,
                    &geology.elevations,
                    flatten_water,
                    world_config.radius,
                );
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, new_positions);
            }

            if pipeline.recolor_mesh {
                let colors = compute_overlay_colors(&overlay, &geology, &climate, &hydrology);
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
) -> Vec<[f32; 4]> {
    match *overlay {
        MapOverlay::SpaceView => build_space_colors(geology, climate, hydrology),
        MapOverlay::Elevation => build_elevation_colors(geology),
        MapOverlay::TectonicPlates => build_plate_colors(geology),
        MapOverlay::Lithology => build_lithology_colors(geology),
        MapOverlay::Temperature => build_temperature_colors(climate),
        MapOverlay::Humidity => build_humidity_colors(climate),
        MapOverlay::Precipitation => build_precipitation_colors(climate),
        MapOverlay::Albedo => build_albedo_colors(climate),
        MapOverlay::Continentality => build_continentality_colors(climate),
        MapOverlay::Pressure => build_pressure_colors(climate),
        MapOverlay::Winds => build_wind_colors(climate),
        MapOverlay::Soils => build_soil_colors(hydrology),
        MapOverlay::SoilMoisture => build_soil_moisture_colors(geology, hydrology),
        MapOverlay::Biomes => build_biome_colors(hydrology),
    }
}

pub fn draw_axis_and_equator_gizmos(
    mut gizmos: Gizmos,
    view_config: Res<ViewportViewConfig>,
    world_config: Res<crate::world::WorldConfig>,
    globe_query: Query<&GlobalTransform, With<Globe>>,
) {
    if !view_config.show_axis_equator {
        return;
    }

    let Ok(global_transform) = globe_query.single() else {
        return;
    };

    let globe_transform = global_transform.compute_transform();

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
