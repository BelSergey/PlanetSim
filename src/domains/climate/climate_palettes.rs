// src/domains/climate/climate_palettes.rs
use bevy::prelude::*;
use rayon::prelude::*;

use super::climate_data::ClimateLayer;
use crate::{
    domains::{
        palettes::{LegendEntry, MapLegend, rgb_u8},
        tectonics::tectonics_data::GeologyLayer,
    },
    viewport::{Globe, ViewMode, ViewportViewConfig},
    world::{WorldConfig, topology::WorldTopology},
};

/// Палитра относительной влажности [0.0 .. 1.0]:
/// Пустынная охра (сухо) -> Салатово-зеленый (умеренно) -> Бирюзовый -> Глубокий синий (100% влажность)
pub fn build_humidity_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    climate
        .humidity
        .par_iter()
        .map(|&h| humidity_to_color(h).to_linear().to_f32_array())
        .collect()
}

fn humidity_to_color(h: f32) -> Color {
    const STOPS: &[(f32, [f32; 3])] = &[
        (0.00, [0.82, 0.58, 0.32]), // Сухая пустыня (песочно-охристый)
        (0.25, [0.88, 0.82, 0.45]), // Полупустыня / степь (светло-желтый)
        (0.50, [0.42, 0.78, 0.44]), // Умеренная влажность (зелень)
        (0.75, [0.15, 0.62, 0.78]), // Влажный морской / муссонный (бирюзовый)
        (1.00, [0.06, 0.22, 0.68]), // Тропический ливень / насыщенный пар (глубокий синий)
    ];

    let h = h.clamp(0.0, 1.0);
    for window in STOPS.windows(2) {
        let (h0, c0) = window[0];
        let (h1, c1) = window[1];
        if h <= h1 {
            let f = (h - h0) / (h1 - h0);
            return Color::srgb(
                c0[0] + (c1[0] - c0[0]) * f,
                c0[1] + (c1[1] - c0[1]) * f,
                c0[2] + (c1[2] - c0[2]) * f,
            );
        }
    }
    let [r, g, b] = STOPS.last().unwrap().1;
    Color::srgb(r, g, b)
}

pub fn build_temperature_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    climate
        .temperatures
        .par_iter()
        .map(|&t| temperature_to_color(t).to_linear().to_f32_array())
        .collect()
}

fn temperature_to_color(t: f32) -> Color {
    // Узловые точки: (Температура °C, R, G, B)
    const STOPS: &[(f32, [f32; 3])] = &[
        (-30.0, [0.92, 0.94, 1.00]), // Белый / Арктический лед
        (-15.0, [0.60, 0.20, 0.80]), // Фиолетовый (суровый мороз)
        (0.0, [0.05, 0.35, 0.85]),   // Синий (точка замерзания)
        (12.0, [0.20, 0.72, 0.40]),  // Зеленый (умеренный климат)
        (22.0, [0.92, 0.82, 0.20]),  // Желтый (субтропики)
        (30.0, [0.92, 0.40, 0.10]),  // Оранжевый (жара)
        (42.0, [0.65, 0.08, 0.08]),  // Темно-красный (пустынный зной)
    ];

    if t <= STOPS[0].0 {
        let [r, g, b] = STOPS[0].1;
        return Color::srgb(r, g, b);
    }
    for window in STOPS.windows(2) {
        let (t0, c0) = window[0];
        let (t1, c1) = window[1];
        if t <= t1 {
            let f = (t - t0) / (t1 - t0);
            return Color::srgb(
                c0[0] + (c1[0] - c0[0]) * f,
                c0[1] + (c1[1] - c0[1]) * f,
                c0[2] + (c1[2] - c0[2]) * f,
            );
        }
    }
    let [r, g, b] = STOPS.last().unwrap().1;
    Color::srgb(r, g, b)
}

pub fn build_albedo_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    climate
        .albedo
        .par_iter()
        .map(|&a| Color::srgb(a, a, a).to_linear().to_f32_array())
        .collect()
}

pub fn build_continentality_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    climate
        .continentality
        .par_iter()
        .map(|&c| {
            let color = if c < 0.0 {
                // ОКЕАН: c ∈ [-1.0 (открытый океан) .. 0.0 (берег/маргинальное море)]
                let t = (-c).clamp(0.0, 1.0);
                Color::srgb(0.45 - 0.40 * t, 0.78 - 0.63 * t, 0.88 - 0.43 * t)
            } else {
                // СУША: c ∈ [0.0 (побережье) .. +1.0 (глубь материка)]
                let t = c.clamp(0.0, 1.0);
                if t < 0.5 {
                    let s = t * 2.0;
                    Color::srgb(0.92 - 0.07 * s, 0.85 - 0.30 * s, 0.68 - 0.38 * s)
                } else {
                    let s = (t - 0.5) * 2.0;
                    Color::srgb(0.85 - 0.37 * s, 0.55 - 0.40 * s, 0.30 - 0.20 * s)
                }
            };
            color.to_linear().to_f32_array()
        })
        .collect()
}

/// Палитра атмосферного давления: Синий = Низкое (Циклоны/ITCZ), Белый = Норма, Красный = Высокое (Антициклоны)
pub fn build_pressure_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    climate
        .pressure
        .par_iter()
        .map(|&p| {
            let norm = p.clamp(-1.0, 1.0);
            let color = if norm < 0.0 {
                let t = norm + 1.0;
                Color::srgb(0.1 + 0.85 * t, 0.25 + 0.7 * t, 0.95)
            } else {
                let t = norm;
                Color::srgb(0.95, 0.95 - 0.75 * t, 0.95 - 0.85 * t)
            };
            color.to_linear().to_f32_array()
        })
        .collect()
}

/// Палитра силы ветра (как на метеокартах): Синий (штиль) -> Бирюзовый -> Желтый -> Красный (шторм)
pub fn build_wind_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    let max_speed = climate
        .winds
        .par_iter()
        .map(|w| w.length())
        .reduce(|| 0.0001_f32, |a, b| a.max(b));

    climate
        .winds
        .iter()
        .map(|&w| {
            let s = (w.length() / max_speed).clamp(0.0, 1.0).sqrt();
            wind_speed_to_color(s).to_linear().to_f32_array()
        })
        .collect()
}

/// Цвет ветра по нормированной силе [0..1]. Общий для меша и 3D-стрелок.
fn wind_speed_to_color(s: f32) -> Color {
    let s = s.clamp(0.0, 1.0);
    match s {
        x if x < 0.25 => {
            let t = x / 0.25;
            Color::srgb(0.08, 0.12 + 0.48 * t, 0.35 + 0.45 * t) // тёмно-синий → голубой
        }
        x if x < 0.50 => {
            let t = (x - 0.25) / 0.25;
            Color::srgb(0.08 + 0.22 * t, 0.60 + 0.25 * t, 0.80 - 0.40 * t) // голубой → зелёный
        }
        x if x < 0.75 => {
            let t = (x - 0.50) / 0.25;
            Color::srgb(0.30 + 0.65 * t, 0.85, 0.40 - 0.30 * t) // зелёный → жёлтый
        }
        _ => {
            let t = (s - 0.75) / 0.25;
            Color::srgb(0.95, 0.85 - 0.70 * t, 0.10 + 0.30 * t) // жёлтый → красный
        }
    }
}

/// Отрисовка 3D-стрелок ветра (берет уже готовый кэш из ClimateLayer, рассчитанный в фоновом потоке!)
pub(super) fn draw_wind_arrows_gizmos(
    mut gizmos: Gizmos,
    topology: Res<WorldTopology>,
    geology: Res<GeologyLayer>,
    climate: Res<ClimateLayer>,
    view_mode: Res<ViewMode>,
    view_config: Res<ViewportViewConfig>,
    world_config: Res<WorldConfig>,
    globe_query: Query<&GlobalTransform, With<Globe>>,
) {
    if !view_config.show_wind_arrows
        || climate.wind_arrows_cache.is_empty()
        || geology.elevations.is_empty()
    {
        return;
    }
    let Ok(global_transform) = globe_query.single() else {
        return;
    };
    let globe_transform = global_transform.compute_transform();
    let radius = world_config.radius;
    let height_scale = world_config.height_scale();
    let base_len = radius * 0.08;
    let is_flat_map = *view_mode == ViewMode::Map3D;

    for &(i, strength) in &climate.wind_arrows_cache {
        let Some(cell) = topology.cells.get(i) else {
            continue;
        };
        let Some(&elevation) = geology.elevations.get(i) else {
            continue;
        };
        let Some(&wind_3d) = climate.winds_3d.get(i) else {
            continue;
        };
        let cell_dir = cell.center;
        let arrow_len = base_len * (0.35 + 0.65 * strength);

        let (local_start, local_end) = if is_flat_map {
            let latitude = cell_dir.y.clamp(-1.0, 1.0).asin();
            let longitude = cell_dir.x.atan2(cell_dir.z);
            let cos_lat = latitude.cos().abs().max(0.18);
            let wind = climate.winds.get(i).copied().unwrap_or(Vec2::ZERO);
            let direction = Vec3::new(wind.x / cos_lat, wind.y, 0.0).normalize_or_zero();
            let start = Vec3::new(
                longitude * radius,
                latitude * radius,
                elevation.max(0.0) * height_scale + radius * 0.012,
            );
            (start, start + direction * arrow_len)
        } else {
            let start = cell_dir * (radius + elevation.max(0.0) * height_scale + radius * 0.015);
            let direction = wind_3d.normalize_or_zero();
            (start, start + direction * arrow_len)
        };

        let world_start = globe_transform.transform_point(local_start);
        let world_end = globe_transform.transform_point(local_end);
        let color = wind_speed_to_color(strength).with_alpha(0.45 + 0.55 * strength);
        gizmos.arrow(world_start, world_end, color);
    }
}

/// Отрисовка 3D-стрелок океанических течений на сфере или на плоской проекции.
pub(super) fn draw_ocean_currents_gizmos(
    mut gizmos: Gizmos,
    topology: Res<WorldTopology>,
    geology: Res<GeologyLayer>,
    climate: Res<ClimateLayer>,
    view_mode: Res<ViewMode>,
    view_config: Res<ViewportViewConfig>,
    world_config: Res<WorldConfig>,
    globe_query: Query<&GlobalTransform, With<Globe>>,
) {
    if !view_config.show_ocean_currents
        || climate.current_arrows_cache.is_empty()
        || geology.elevations.is_empty()
    {
        return;
    }
    let Ok(global_transform) = globe_query.single() else {
        return;
    };
    let globe_transform = global_transform.compute_transform();
    let radius = world_config.radius;
    let base_len = radius * 0.075;
    let is_flat_map = *view_mode == ViewMode::Map3D;

    for &(i, strength) in &climate.current_arrows_cache {
        let Some(cell) = topology.cells.get(i) else {
            continue;
        };
        let cell_dir = cell.center;
        let Some(&current) = climate.ocean_currents.get(i) else {
            continue;
        };
        let arrow_len = base_len * (0.35 + 0.65 * strength);

        let (local_start, local_end) = if is_flat_map {
            let latitude = cell_dir.y.clamp(-1.0, 1.0).asin();
            let longitude = cell_dir.x.atan2(cell_dir.z);
            let east = Vec3::new(longitude.cos(), 0.0, -longitude.sin());
            let north = cell_dir.cross(east).normalize_or_zero();
            let cos_lat = latitude.cos().abs().max(0.18);
            let tangent = Vec2::new(current.dot(east), current.dot(north));
            let direction = Vec3::new(tangent.x / cos_lat, tangent.y, 0.0).normalize_or_zero();
            let start = Vec3::new(longitude * radius, latitude * radius, radius * 0.012);
            (start, start + direction * arrow_len)
        } else {
            let terrain_height = geology.elevations.get(i).copied().unwrap_or(0.0) * height_scale;
            let start = cell_dir * (radius + terrain_height + radius * 0.012);
            let end = start + current.normalize_or_zero() * arrow_len;
            (start, end)
        };

        let world_start = globe_transform.transform_point(local_start);
        let world_end = globe_transform.transform_point(local_end);
        let color = Color::srgba(0.15, 0.85, 0.95, 0.5 + 0.5 * strength);
        gizmos.arrow(world_start, world_end, color);
    }
}

/// Палитра осадков [0.0 .. 1.0] с автоматическим разделением на ДОЖДЬ (T > 0°C) и СНЕГ (T < 0°C).
pub fn build_precipitation_colors(climate: &ClimateLayer) -> Vec<[f32; 4]> {
    climate
        .precipitation
        .iter()
        .zip(&climate.temperatures)
        .map(|(&p, &t)| precipitation_to_color(p, t).to_linear().to_f32_array())
        .collect()
}

fn precipitation_to_color(p: f32, temp_c: f32) -> Color {
    // 1. Шкала жидких осадков (Дождь):
    // Пустыня -> Степь -> Умеренные дожди -> Сильные дожди -> Тропический ливень
    const RAIN_STOPS: &[(f32, [f32; 3])] = &[
        (0.00, [0.85, 0.68, 0.42]), // Аридная пустыня (песочный)
        (0.12, [0.86, 0.84, 0.44]), // Сухая степь / полупустыня (желтый)
        (0.30, [0.38, 0.76, 0.42]), // Умеренные осадки (зеленый)
        (0.55, [0.12, 0.56, 0.80]), // Влажный климат (сине-голубой)
        (0.78, [0.16, 0.22, 0.74]), // Муссонный ливень (глубокий индиго)
        (1.00, [0.52, 0.12, 0.68]), // Экстремальный тропический/горный ливень (пурпурный)
    ];

    // 2. Шкала твердых осадков (Снег при T < 0°C):
    // Сухая мерзлота (мало снега) -> Поземка -> Снегопад -> Мощная метель/ледник
    const SNOW_STOPS: &[(f32, [f32; 3])] = &[
        (0.00, [0.52, 0.55, 0.60]), // Сухая морозная пустыня (холодный серо-стальной)
        (0.15, [0.58, 0.72, 0.82]), // Малоснежная тундра (сизо-голубой)
        (0.40, [0.72, 0.88, 0.96]), // Умеренный снегопад (морозно-голубой)
        (0.70, [0.90, 0.96, 1.00]), // Сильный снегопад (белоснежный)
        (1.00, [0.82, 0.76, 0.98]), // Экстремальная снежная буря в горах (лавандово-белый)
    ];

    let p = p.clamp(0.0, 1.0);
    let rain_rgb = sample_gradient(RAIN_STOPS, p);
    let snow_rgb = sample_gradient(SNOW_STOPS, p);

    let snow_frac = ClimateLayer::snow_fraction(temp_c);
    Color::srgb(
        rain_rgb[0] * (1.0 - snow_frac) + snow_rgb[0] * snow_frac,
        rain_rgb[1] * (1.0 - snow_frac) + snow_rgb[1] * snow_frac,
        rain_rgb[2] * (1.0 - snow_frac) + snow_rgb[2] * snow_frac,
    )
}

#[inline]
fn sample_gradient(stops: &[(f32, [f32; 3])], x: f32) -> [f32; 3] {
    for window in stops.windows(2) {
        let (x0, c0) = window[0];
        let (x1, c1) = window[1];
        if x <= x1 {
            let f = (x - x0) / (x1 - x0).max(1e-5);
            return [
                c0[0] + (c1[0] - c0[0]) * f,
                c0[1] + (c1[1] - c0[1]) * f,
                c0[2] + (c1[2] - c0[2]) * f,
            ];
        }
    }
    stops.last().unwrap().1
}

// ==== ЛЕГЕНДЫ КЛИМАТА ====

pub fn humidity_legend() -> MapLegend {
    MapLegend {
        title: "Относительная влажность",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.82, 0.58, 0.32),
                label: "Сухая пустыня  (0.00)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.88, 0.82, 0.45),
                label: "Полупустыня / степь  (0.25)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.42, 0.78, 0.44),
                label: "Умеренная влажность  (0.50)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.15, 0.62, 0.78),
                label: "Влажно / муссон  (0.75)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.06, 0.22, 0.68),
                label: "Насыщение  (1.00)".into(),
            },
        ],
    }
}

pub fn temperature_legend() -> MapLegend {
    MapLegend {
        title: "Температура (°C)",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.92, 0.94, 1.00),
                label: "Арктический лёд  (<= -30)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.60, 0.20, 0.80),
                label: "Суровый мороз  (-15)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.05, 0.35, 0.85),
                label: "Точка замерзания  (0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.20, 0.72, 0.40),
                label: "Умеренный климат  (12)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.92, 0.82, 0.20),
                label: "Субтропики  (22)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.92, 0.40, 0.10),
                label: "Жара  (30)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.65, 0.08, 0.08),
                label: "Пустынный зной  (>= 42)".into(),
            },
        ],
    }
}

pub fn albedo_legend() -> MapLegend {
    MapLegend {
        title: "Альбедо",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.05, 0.05, 0.05),
                label: "Тёмная поверхность  (0.05)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.30, 0.30, 0.30),
                label: "Умеренно тёмная  (0.30)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.60, 0.60, 0.60),
                label: "Средняя  (0.60)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.85, 0.85, 0.85),
                label: "Светлая  (0.85)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(1.00, 1.00, 1.00),
                label: "Снег / лёд  (1.00)".into(),
            },
        ],
    }
}

pub fn continentality_legend() -> MapLegend {
    MapLegend {
        title: "Континентальность",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.45, 0.78, 0.88),
                label: "Открытый океан  (-1.0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.20, 0.50, 0.70),
                label: "Берег / море  (~0.0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.92, 0.85, 0.68),
                label: "Побережье суши  (0.0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.85, 0.55, 0.30),
                label: "Внутриматериковый  (0.5)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.48, 0.15, 0.10),
                label: "Глубь материка  (1.0)".into(),
            },
        ],
    }
}

pub fn pressure_legend() -> MapLegend {
    MapLegend {
        title: "Атмосферное давление",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.10, 0.25, 0.95),
                label: "Низкое — циклоны / ITCZ  (-1)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.95, 0.95, 0.95),
                label: "Норма  (0)".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.95, 0.20, 0.10),
                label: "Высокое — антициклоны  (+1)".into(),
            },
        ],
    }
}

pub fn wind_legend() -> MapLegend {
    MapLegend {
        title: "Скорость ветра",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.08, 0.12, 0.35),
                label: "Штиль".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.08, 0.60, 0.80),
                label: "Слабый".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.30, 0.85, 0.40),
                label: "Умеренный".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.95, 0.85, 0.10),
                label: "Сильный".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.95, 0.15, 0.40),
                label: "Шторм".into(),
            },
        ],
    }
}

pub fn precipitation_legend() -> MapLegend {
    // Цвет точки = комбинация кол-ва осадков и температуры.
    // Показываем ключевые цвета обеих шкал.
    MapLegend {
        title: "Осадки (цвет = кол-во + T)",
        entries: vec![
            LegendEntry {
                rgb: rgb_u8(0.85, 0.68, 0.42),
                label: "Пустыня — дождь".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.38, 0.76, 0.42),
                label: "Умеренные дожди".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.16, 0.22, 0.74),
                label: "Муссонный ливень".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.52, 0.12, 0.68),
                label: "Экстремальный ливень".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.52, 0.55, 0.60),
                label: "Сухая мерзлота — снег".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.72, 0.88, 0.96),
                label: "Умеренный снегопад".into(),
            },
            LegendEntry {
                rgb: rgb_u8(0.90, 0.96, 1.00),
                label: "Сильная метель".into(),
            },
        ],
    }
}
