// src/domains/climate/climate_config.rs

use super::climate_data::ClimateSeason;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Resource, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimateConfig {
    /// Базовая температура на экваторе (в градусах Цельсия)
    pub equator_temp: f32,
    /// Базовая температура на полюсах
    pub pole_temp: f32,
    /// Насколько сильно падает температура с высотой (Lapse Rate)
    pub altitude_lapse_rate: f32,

    /// Минимальный размер водоема (в долях от общего числа гексов планеты 0.0..1.0),
    pub min_ocean_size_ratio: f32,
    /// Базовое смягчение температур над самим океаном (dist = 0)
    pub base_ocean_moderation: f32,

    /// Отражательная способность воды (поглощает много тепла)
    pub ocean_albedo: f32,
    /// Отражательная способность обычной суши
    pub land_albedo: f32,
    /// Отражательная способность льда (отражает тепло в космос)
    pub ice_albedo: f32,
    /// Температура, при которой земля покрывается льдом (меняя альбедо)
    pub freezing_temp: f32,

    /// Сила базовых широтных поясов (ячейки Хэдли, Феррела, Полярная)
    pub latitudinal_pressure_weight: f32,
    /// Насколько сильно прогрев/охлаждение материков искажает пояса давления
    pub thermal_pressure_weight: f32,
    /// Множитель скорости ветра от перепада давлений
    pub wind_speed_multiplier: f32,
    /// Сила Кориолиса (максимальный угол отклонения ветра в радианах, ~1.1 рад = ~63°)
    pub coriolis_strength: f32,
    /// Насколько сильно горные хребты тормозят ветер при подъеме
    pub mountain_wind_barrier: f32,

    /// Сила переноса тепла ветрами (0.0 — ветра не влияют на температуру, 1.0 — максимум)
    pub heat_advection_strength: f32,
    /// Количество итераций поиска равновесия (Температура <-> Давление/Ветра)
    pub equilibrium_iterations: usize,

    /// Наклон оси планеты в радианах (0.41 рад ≈ 23.5°)
    pub axial_tilt: f32,
    /// Какой из трех предрассчитанных сезонов сейчас показывать на глобусе
    pub active_season: ClimateSeason,

    pub ocean_continentality_scale: f32,
    pub land_continentality_scale: f32,
    pub marginal_sea_moderation_reduction: f32,

    pub mountain_base_level: f32,
    pub mountain_blend_width: f32,

    /// Базовый множитель скорости течений относительно ветра
    pub ocean_current_speed_multiplier: f32,
    /// Насколько сильно течения ускоряются вдоль берегов (эффект Гольфстрима)
    pub boundary_current_multiplier: f32,
    /// Сила переноса тепла именно толщей воды (высокая термоинерция)
    pub ocean_advection_rate: f32,

    /// Нужно написать что это
    pub geostrophic_multiplier: f32,

    // --- ПАРАМЕТРЫ ВЛАЖНОСТИ ---
    /// Сила и дальность переноса водяного пара ветрами вглубь материков
    pub moisture_advection_strength: f32,
    /// Сила орографического барьера (насколько крутые горы отсекают влагу, создавая дождевую тень)
    pub orographic_rain_shadow: f32,
    /// Интенсивность испарения воды с поверхности океанов
    pub ocean_evaporation_rate: f32,
    /// Скорость фонового иссушения воздуха в антициклонах
    pub subsidence_drying_strength: f32,

    /// Интенсивность выпадения осадков (конвективных и орографических)
    pub precipitation_rate: f32,
    /// Доля выпавших осадков, которая испаряется с почвы/лесов обратно в воздух (рециклинг влаги)
    pub land_moisture_recycling: f32,
}

impl Default for ClimateConfig {
    fn default() -> Self {
        Self {
            equator_temp: 32.0,
            pole_temp: -20.0,
            altitude_lapse_rate: 70.0,

            min_ocean_size_ratio: 0.01,
            base_ocean_moderation: 0.6,

            ocean_albedo: 0.06,
            land_albedo: 0.3,
            ice_albedo: 0.8,
            freezing_temp: -5.0,

            latitudinal_pressure_weight: 0.45,
            thermal_pressure_weight: 0.65,
            wind_speed_multiplier: 12.0,
            coriolis_strength: 3.05,
            mountain_wind_barrier: 8.0,

            heat_advection_strength: 0.75,
            equilibrium_iterations: 2,

            axial_tilt: 23.439281_f32.to_radians(),
            active_season: ClimateSeason::Spring,

            ocean_continentality_scale: 0.35,
            land_continentality_scale: 0.45,
            marginal_sea_moderation_reduction: 0.20,

            mountain_base_level: 0.1,
            mountain_blend_width: 0.02,

            ocean_current_speed_multiplier: 0.45,
            boundary_current_multiplier: 4.0,
            ocean_advection_rate: 0.99,

            geostrophic_multiplier: 2.0,

            moisture_advection_strength: 1.0,
            orographic_rain_shadow: 7.0,
            ocean_evaporation_rate: 1.0,
            subsidence_drying_strength: 1.0,
            precipitation_rate: 1.0,
            land_moisture_recycling: 0.40,
        }
    }
}
