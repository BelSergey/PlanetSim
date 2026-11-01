// src/domains/climate/climate_data.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub enum ClimateSeason {
    #[default]
    Spring, // Весна / Осень (Равноденствие, смещение 0.0) — по умолчанию
    Summer, // Лето в Северном полушарии / Зима в Южном (+axial_tilt)
    Winter, // Зима в Северном полушарии / Лето в Южном (-axial_tilt)
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SeasonalSnapshot {
    pub temperatures: Vec<f32>,
    pub humidity: Vec<f32>,
    pub precipitation: Vec<f32>,
    pub albedo: Vec<f32>,
    pub pressure: Vec<f32>,
    pub winds: Vec<Vec2>,
    pub winds_3d: Vec<Vec3>,
    pub ocean_currents: Vec<Vec3>,
    pub wind_arrows_cache: Vec<(usize, f32)>,
    pub current_arrows_cache: Vec<(usize, f32)>,
}

impl SeasonalSnapshot {
    pub fn resize(&mut self, num_cells: usize) {
        self.temperatures.resize(num_cells, 0.0);
        self.humidity.resize(num_cells, 0.0);
        self.precipitation.resize(num_cells, 0.0);
        self.albedo.resize(num_cells, 0.0);
        self.pressure.resize(num_cells, 0.0);
        self.winds.resize(num_cells, Vec2::ZERO);
        self.winds_3d.resize(num_cells, Vec3::ZERO);
        self.ocean_currents.resize(num_cells, Vec3::ZERO);
    }
}

#[derive(Resource, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimateLayer {
    pub temperatures: Vec<f32>,
    pub humidity: Vec<f32>,
    /// Накопленные за сезон осадки [0.0 = сухая пустыня .. 1.0 = тропический ливень]
    pub precipitation: Vec<f32>,
    pub albedo: Vec<f32>,
    pub continentality: Vec<f32>,

    /// Бесснежное альбедо поверхности суши (от биомов, почв и рек)
    pub surface_albedo: Vec<f32>,
    /// Доступная для испарения и транспирации влага поверхности [0.0 = сухая скала/пустыня .. 1.0 = река/болото/джунгли]
    pub surface_wetness: Vec<f32>,

    /// Атмосферное давление (в условных единицах отклонения от нормы: < 0 — циклон/низкое, > 0 — антициклон/высокое)
    pub pressure: Vec<f32>,
    /// Локальный 2D-вектор ветра: X — с запада на восток, Y — с юга на север
    pub winds: Vec<Vec2>,
    /// 3D-вектор ветра, лежащий в касательной плоскости к сфере (удобно для 3D-стрелок и адвекции)
    pub winds_3d: Vec<Vec3>,
    /// Векторы океанических течений в активном слое
    pub ocean_currents: Vec<Vec3>,

    pub wind_arrows_cache: Vec<(usize, f32)>,
    pub current_arrows_cache: Vec<(usize, f32)>,

    /// Теплоёмкость подстилающей поверхности [0..1].
    /// 1.0 — открытая вода (макс. инерция), 0.1 — скалы/сухая пустыня.
    /// Заполняется hydrology-solver'ом в apply_surface_feedback_to_climate.
    pub surface_heat_capacity: Vec<f32>,

    // Предрассчитанные снимки трех сезонов для биомов, рек и мгновенного переключения в UI
    pub spring: SeasonalSnapshot,
    pub summer: SeasonalSnapshot,
    pub winter: SeasonalSnapshot,
}

impl ClimateLayer {
    pub fn resize(&mut self, num_cells: usize) {
        self.temperatures.resize(num_cells, 0.0);
        self.humidity.resize(num_cells, 0.0);
        self.precipitation.resize(num_cells, 0.0);
        self.albedo.resize(num_cells, 0.0);
        self.continentality.resize(num_cells, 0.0);

        self.surface_albedo.resize(num_cells, 0.30);
        self.surface_wetness.resize(num_cells, 0.35);

        self.pressure.resize(num_cells, 0.0);
        self.winds.resize(num_cells, Vec2::ZERO);
        self.winds_3d.resize(num_cells, Vec3::ZERO);
        self.ocean_currents.resize(num_cells, Vec3::ZERO);

        self.surface_heat_capacity.resize(num_cells, 0.35); // дефолт суши

        self.spring.resize(num_cells);
        self.summer.resize(num_cells);
        self.winter.resize(num_cells);
    }

    /// Сбрасывает свойства поверхности к нейтральным константам перед 1-м проходом климата
    pub fn reset_surface_defaults(
        &mut self,
        num_cells: usize,
        land_albedo: f32,
        base_wetness: f32,
    ) {
        self.surface_albedo.clear();
        self.surface_albedo.resize(num_cells, land_albedo);
        self.surface_wetness.clear();
        self.surface_wetness.resize(num_cells, base_wetness);
    }

    /// Копирует предрассчитанный сезон в активные буферы рендера без пересчета физики
    pub fn apply_active_season(&mut self, season: ClimateSeason) {
        let snap = match season {
            ClimateSeason::Spring => &self.spring,
            ClimateSeason::Summer => &self.summer,
            ClimateSeason::Winter => &self.winter,
        };
        self.temperatures.clone_from(&snap.temperatures);
        self.humidity.clone_from(&snap.humidity);
        self.precipitation.clone_from(&snap.precipitation);
        self.albedo.clone_from(&snap.albedo);
        self.pressure.clone_from(&snap.pressure);
        self.winds.clone_from(&snap.winds);
        self.winds_3d.clone_from(&snap.winds_3d);
        self.ocean_currents.clone_from(&snap.ocean_currents);
        self.wind_arrows_cache = snap.wind_arrows_cache.clone();
        self.current_arrows_cache = snap.current_arrows_cache.clone();
    }

    /// Доля твердых осадков (снега) при данной температуре [0.0 = дождь, 1.0 = снег].
    /// Используется для визуализации снежных фронтов и накопления снега для весеннего половодья рек.
    #[inline]
    pub fn snow_fraction(temp_c: f32) -> f32 {
        let t = ((2.0 - temp_c) / 6.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}
