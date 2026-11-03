//! src/domains/palettes.rs
//!
//! Общие типы для легенд карт. Живут отдельно от конкретных палитр,
//! чтобы любая палитра (tectonics / climate / hydrology / …) могла
//! строить легенды в одном формате.

/// Один пункт легенды: цвет в sRGB (u8) + подпись.
#[derive(Clone)]
pub struct LegendEntry {
    pub rgb: [u8; 3],
    pub label: String,
}

/// Готовая легенда слоя.
#[derive(Clone)]
pub struct MapLegend {
    pub title: &'static str,
    pub entries: Vec<LegendEntry>,
}

/// Утилита: [0.0..=1.0]³ → [u8;3]. Используется всеми палитрами.
pub fn rgb_u8(r: f32, g: f32, b: f32) -> [u8; 3] {
    [
        (r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (b.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}
