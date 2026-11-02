// src/viewport/mod.rs
use rayon::prelude::*;
use std::time::Instant;
pub mod camera_3d;
pub mod hotkeys;
pub mod mesh_globe;

use crate::astronomy::{AxialRotation, Orbit, Sun, Sunlight};
use bevy::light::NotShadowCaster;

use mesh_globe::{
    MeshColorLayout, build_globe_mesh, build_river_mesh, compute_color_layout,
    recolor_globe_mesh_range, reposition_globe_mesh,
};

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
                build_ruggedness_colors,
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

/// Визуальная поверхность планеты.
#[derive(Component)]
pub struct Globe;

/// Корневой объект всей звёздной системы.
#[derive(Component)]
pub struct StarSystemRoot;

/// Режим проекции мира (задел под переключение между 3D-сферой и 2D-картой).
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum ViewMode {
    #[default]
    Globe3D,
    _Flat2D,
}

/// Активный слой раскраски карты.
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
    Ruggedness,
}

/// Кэш разметки цветов (offsets + маска стенок).
/// Перекраска выполняется целиком за 1 кадр — визуальных «скачков» нет.
#[derive(Default)]
struct ViewportMeshCache {
    cached_layout: Option<MeshColorLayout>,
    /// Последние отправленные в меш цвета, квантованные в u8.
    /// Квантование важно: Float32 сравнение ловит шум в 1e-7,
    /// который глаз не видит, но recolor_globe_mesh_range честно перепишет.
    /// u8 = ровно то, что видно на экране (после тональной компрессии).
    last_space_colors_u8: Option<Vec<[u8; 4]>>,
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

    let sun_mesh = meshes.add(Sphere::new(1.0).mesh().uv(48, 32));

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

    let system_root = commands
        .spawn((
            StarSystemRoot,
            SystemInertia::default(),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();

    commands.entity(system_root).with_children(|system| {
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
            .with_children(|parent| {
                parent
                    .spawn((
                        Mesh3d::default(),
                        MeshMaterial3d(material_handle),
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

pub fn build_space_colors(
    geology: &GeologyLayer,
    climate: &ClimateLayer,
    hydrology: &HydrologyLayer,
) -> Vec<[f32; 4]> {
    let num_cells = geology.elevations.len();
    if num_cells == 0 {
        return Vec::new();
    }

    geology
        .elevations
        .par_iter()
        .enumerate()
        .map(|(i, &h)| {
            let temp = climate.temperatures.get(i).copied().unwrap_or(15.0);
            let humidity = climate.humidity.get(i).copied().unwrap_or(0.5);
            let precip = climate.precipitation.get(i).copied().unwrap_or(0.2);
            let biome = hydrology.biomes.get(i).copied().unwrap_or(Biome::DeepOcean);

            if h <= 0.0 {
                render_ocean_pixel(h, temp, biome)
            } else {
                render_land_pixel(h, temp, humidity, precip, biome)
            }
        })
        .collect()
}

#[inline]
fn render_ocean_pixel(h: f32, temp: f32, biome: Biome) -> [f32; 4] {
    let depth = (h / -0.35).clamp(0.0, 1.0);
    let water_r = 0.015 + (1.0 - depth) * 0.04;
    let water_g = 0.080 + (1.0 - depth) * 0.16;
    let water_b = 0.220 + (1.0 - depth) * 0.22;

    let freeze_factor = ((-1.5 - temp) / 6.5).clamp(0.0, 1.0);

    let sea_ice_amount = if biome == Biome::SeaIce {
        if temp > 2.0 {
            (1.0 - (temp - 2.0) * 0.25).clamp(0.15, 1.0)
        } else {
            1.0
        }
    } else {
        freeze_factor * freeze_factor * (3.0 - 2.0 * freeze_factor)
    };

    if sea_ice_amount <= 0.001 {
        Color::srgb(water_r, water_g, water_b)
            .to_linear()
            .to_f32_array()
    } else {
        let ice_r = 0.82 + 0.12 * sea_ice_amount;
        let ice_g = 0.90 + 0.07 * sea_ice_amount;
        let ice_b = 0.96 + 0.04 * sea_ice_amount;

        let r = water_r * (1.0 - sea_ice_amount) + ice_r * sea_ice_amount;
        let g = water_g * (1.0 - sea_ice_amount) + ice_g * sea_ice_amount;
        let b = water_b * (1.0 - sea_ice_amount) + ice_b * sea_ice_amount;

        Color::srgb(r, g, b).to_linear().to_f32_array()
    }
}

#[inline]
fn render_land_pixel(h: f32, temp: f32, humidity: f32, precip: f32, biome: Biome) -> [f32; 4] {
    let (mut base_r, mut base_g, mut base_b, _) = biome_satellite_rgb(biome);

    match biome {
        Biome::TemperateDeciduousForest => {
            if temp < 12.0 {
                let autumn = ((12.0 - temp) / 9.0).clamp(0.0, 1.0);
                let fall_r = 0.42 * (1.0 - autumn * 0.4) + 0.28 * (autumn * 0.4);
                let fall_g = 0.32 * (1.0 - autumn * 0.7) + 0.18 * (autumn * 0.7);
                let fall_b = 0.12 * (1.0 - autumn * 0.5) + 0.14 * (autumn * 0.5);

                base_r = base_r * (1.0 - autumn) + fall_r * autumn;
                base_g = base_g * (1.0 - autumn) + fall_g * autumn;
                base_b = base_b * (1.0 - autumn) + fall_b * autumn;
            }
        }
        Biome::TemperateGrassland | Biome::Savanna => {
            let dryness = (1.0 - humidity).clamp(0.0, 1.0);
            let coldness = ((10.0 - temp) / 12.0).clamp(0.0, 1.0);
            let dry_factor = dryness.max(coldness) * 0.65;

            let straw_r = 0.65;
            let straw_g = 0.58;
            let straw_b = 0.32;

            base_r = base_r * (1.0 - dry_factor) + straw_r * dry_factor;
            base_g = base_g * (1.0 - dry_factor) + straw_g * dry_factor;
            base_b = base_b * (1.0 - dry_factor) + straw_b * dry_factor;
        }
        _ => {}
    }

    let snow_cover = calculate_snow_cover(temp, humidity, precip, biome);

    let max_visual_snow = match biome {
        Biome::BorealTaiga => 0.62,
        Biome::TemperateRainforest => 0.55,
        Biome::TropicalRainforest => 0.0,
        Biome::TropicalSeasonalForest => 0.0,
        Biome::TemperateDeciduousForest => 0.85,
        _ => 0.98,
    };

    let effective_snow = (snow_cover * max_visual_snow).clamp(0.0, 1.0);

    let snow_r = 0.94;
    let snow_g = 0.97;
    let snow_b = 1.00;

    let mut r = base_r * (1.0 - effective_snow) + snow_r * effective_snow;
    let mut g = base_g * (1.0 - effective_snow) + snow_g * effective_snow;
    let mut b = base_b * (1.0 - effective_snow) + snow_b * effective_snow;

    let elev_shade = (1.0 - (h - 0.10).max(0.0) * 0.35).clamp(0.75, 1.0);
    r *= elev_shade;
    g *= elev_shade;
    b *= elev_shade;

    Color::srgb(r, g, b).to_linear().to_f32_array()
}

#[inline]
fn calculate_snow_cover(temp: f32, humidity: f32, precip: f32, biome: Biome) -> f32 {
    if biome == Biome::IceSheet {
        return 1.0;
    }

    let temp_snow_potential = ClimateLayer::snow_fraction(temp);

    if temp_snow_potential <= 0.001 {
        return 0.0;
    }

    let moisture_availability = (humidity * 0.65 + precip * 1.8).clamp(0.12, 1.0);

    let subzero_persistence = if temp < -6.0 {
        ((-6.0 - temp) / 10.0).clamp(0.0, 0.45)
    } else {
        0.0
    };

    (temp_snow_potential * moisture_availability + subzero_persistence).clamp(0.0, 1.0)
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
    mut deferred_mesh_drops: Local<Vec<Handle<Mesh>>>,
    mut viewport_cache: Local<ViewportMeshCache>,
) {
    deferred_mesh_drops.clear();

    // Видимость рек.
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

    let perf_total = Instant::now();
    let mut perf_colors = std::time::Duration::ZERO;
    let mut perf_recolor = std::time::Duration::ZERO;
    let mut perf_mesh_build = std::time::Duration::ZERO;
    let mut perf_reposition = std::time::Duration::ZERO;
    let mut perf_insert = std::time::Duration::ZERO;
    let mut perf_rivers = std::time::Duration::ZERO;

    let perf_flags = (
        pipeline.rebuild_mesh,
        pipeline.reposition_mesh,
        pipeline.recolor_mesh,
        pipeline.rebuild_rivers,
    );

    // ── 1. Обновление меша глобуса ─────────────────────────────────────
    if let Ok(mut mesh_handle) = globe_query.single_mut() {
        if pipeline.rebuild_mesh {
    let timer = Instant::now();
    let colors = compute_overlay_colors(&overlay, &geology, &climate, &hydrology);
    perf_colors += timer.elapsed();

    let new_mesh = build_globe_mesh(
        &topology,
        &geology.elevations,
        view_config.flat_water,
        &colors,
        world_config.radius,
        world_config.system_scale(),
    );
    perf_mesh_build += timer.elapsed();

    deferred_mesh_drops.push(mesh_handle.0.clone());
    mesh_handle.0 = meshes.add(new_mesh);

    viewport_cache.cached_layout =
        Some(compute_color_layout(&topology, &geology.elevations));

    pipeline.recolor_mesh = false;
} else {
            // Быстрое обновление позиций (изменение формы воды / радиуса).
            if pipeline.reposition_mesh {
                let timer = Instant::now();
                let new_positions = reposition_globe_mesh(
                    &topology,
                    &geology.elevations,
                    view_config.flat_water,
                    world_config.radius,
                    world_config.system_scale(),
                );
                perf_reposition += timer.elapsed();

                if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
                    let timer = Instant::now();
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, new_positions);
                    perf_insert += timer.elapsed();
                }

                // Разметка вершин не меняется при reposition — cached_layout остаётся валидным.
            }

            // Мгновенная перекраска за ОДИН кадр.
            let overlay_needs_recolor = matches!(
                *overlay,
                MapOverlay::SpaceView
                    | MapOverlay::Temperature
                    | MapOverlay::Humidity
                    | MapOverlay::Precipitation
                    | MapOverlay::Albedo
                    | MapOverlay::Pressure
                    | MapOverlay::Winds
            );

if pipeline.recolor_mesh && overlay_needs_recolor {
    let t0 = Instant::now();
    let new_colors_f32 = compute_overlay_colors(&overlay, &geology, &climate, &hydrology);
    let colors_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();

    // Спец‑путь: только для SpaceView, потому что только он меняется от сезона
    if *overlay == MapOverlay::SpaceView {
        let total = topology.cells.len();
        let new_u8: Vec<[u8; 4]> = new_colors_f32
            .iter()
            .map(|c| [
                (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            ])
            .collect();

        if let Some(layout) = &viewport_cache.cached_layout
            && let Some(mut mesh) = meshes.get_mut(&mesh_handle.0)
        {
            match viewport_cache.last_space_colors_u8.as_ref() {
                Some(old_u8) if old_u8.len() == total => {
                    // Сравниваем и строим маску
                    let mut dirty = vec![false; total];
                    let mut dirty_count = 0usize;
                    for i in 0..total {
                        if old_u8[i] != new_u8[i] {
                            dirty[i] = true;
                            dirty_count += 1;
                        }
                    }

                    if dirty_count > 0 {
                        recolor_globe_mesh_range(
                            &mut mesh,
                            &topology,
                            &new_colors_f32,
                            layout,
                            0,
                            total,
                            Some(&dirty),
                        );
                    }

                    info!(
                        "[RECOLOR DIRTY] {}/{} cells changed ({:.1}%)",
                        dirty_count,
                        total,
                        100.0 * dirty_count as f32 / total as f32
                    );
                }
                _ => {
                    // Первый раз или размер сетки сменился — перекрашиваем всё
                    recolor_globe_mesh_range(
                        &mut mesh,
                        &topology,
                        &new_colors_f32,
                        layout,
                        0,
                        total,
                        None,
                    );
                }
            }
        }

        viewport_cache.last_space_colors_u8 = Some(new_u8);
    } else {
        // Остальные слои меняются только явно (клик в UI/хоткей) — перекрашиваем всё
        if let Some(layout) = &viewport_cache.cached_layout
            && let Some(mut mesh) = meshes.get_mut(&mesh_handle.0)
        {
            let total = topology.cells.len();
            recolor_globe_mesh_range(
                &mut mesh,
                &topology,
                &new_colors_f32,
                layout,
                0,
                total,
                None,
            );
        }
        // Инвалидируем кэш SpaceView, чтобы следующий перекрас был полным
        viewport_cache.last_space_colors_u8 = None;
    }

    let write_ms = t1.elapsed().as_secs_f64() * 1000.0;
info!(
    "[RECOLOR SPLIT] colors={:.2}ms, write={:.2}ms",
    colors_ms, write_ms
);

    pipeline.recolor_mesh = false;
}
}
    }

    // ── 2. Пересборка меша рек ─────────────────────────────────────────
    if pipeline.rebuild_mesh || pipeline.reposition_mesh || pipeline.rebuild_rivers {
        if let Ok((mut river_mesh_handle, _)) = river_query.single_mut() {
            let timer = Instant::now();
            let new_river_mesh = build_river_mesh(
                &topology,
                &geology,
                &hydrology,
                &hydrology_config,
                view_config.flat_water,
                world_config.radius,
                world_config.system_scale(),
            );
            perf_rivers += timer.elapsed();

            deferred_mesh_drops.push(river_mesh_handle.0.clone());
            river_mesh_handle.0 = meshes.add(new_river_mesh);
        }
    }

    // Сбрасываем только те флаги, что гарантированно отработаны в этом кадре.
    pipeline.rebuild_mesh = false;
    pipeline.reposition_mesh = false;
    pipeline.rebuild_rivers = false;
    // recolor_mesh уже сброшен внутри своей секции при успешной обработке.

    info!(
        "[VIEWPORT PERF] total={:.2}ms | colors={:.2}ms | recolor={:.2}ms | insert={:.2}ms | mesh_build={:.2}ms | reposition={:.2}ms | rivers={:.2}ms | flags={:?}",
        perf_total.elapsed().as_secs_f64() * 1000.0,
        perf_colors.as_secs_f64() * 1000.0,
        perf_recolor.as_secs_f64() * 1000.0,
        perf_insert.as_secs_f64() * 1000.0,
        perf_mesh_build.as_secs_f64() * 1000.0,
        perf_reposition.as_secs_f64() * 1000.0,
        perf_rivers.as_secs_f64() * 1000.0,
        perf_flags,
    );
    // в конце sync_viewport_mesh — вернуть безусловный сброс:
    pipeline.recolor_mesh = false;
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
        MapOverlay::Ruggedness => build_ruggedness_colors(geology),
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

    let local_top = Vec3::Y * (radius + 1.0);
    let local_bottom = Vec3::NEG_Y * (radius + 1.0);

    let world_top = globe_transform.transform_point(local_top);
    let world_bottom = globe_transform.transform_point(local_bottom);

    gizmos.arrow(world_bottom, world_top, Color::srgba(1.0, 0.2, 0.2, 0.9));

    let equator_radius = radius + 0.05;
    let equator_rotation =
        globe_transform.rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);

    gizmos.circle(
        Isometry3d::new(globe_transform.translation, equator_rotation),
        equator_radius,
        Color::srgba(0.2, 0.8, 1.0, 0.9),
    );
}
