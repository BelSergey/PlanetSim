// src/domains/climate/solver/thermodynamics.rs

use super::super::climate_config::ClimateConfig;
use super::{SolverPrecalc, smoothstep};
use crate::domains::tectonics::tectonics_data::GeologyLayer;

/// Высотное охлаждение с мягким порогом.
/// До base_level — 0, после (base_level + blend_width) — линейно.
pub(super) fn elevation_cooling(elev: f32, base_level: f32, blend_width: f32) -> f32 {
    let x = (elev - base_level).max(0.0);
    let k = blend_width.max(1e-6);
    if x < k {
        x * x / (2.0 * k)
    } else {
        x - k * 0.5
    }
}

#[inline]
pub(super) fn calc_radiative_sea_temp(
    i: usize,
    albedo: f32,
    base_insolation: f32,
    precalc: &SolverPrecalc,
    config: &ClimateConfig,
    geology: &GeologyLayer,
) -> f32 {
    let is_ocean = geology.is_ocean_cell(i);

    let albedo_modifier = if is_ocean && albedo < config.land_albedo {
        1.0
    } else if !is_ocean && albedo <= 0.45 {
        1.0 - (albedo - config.land_albedo) * 0.35
    } else {
        (1.0 - albedo) / (1.0 - config.land_albedo)
    };
    let effective_insolation = base_insolation * albedo_modifier;

    let mut raw_temp =
        config.pole_temp + (config.equator_temp - config.pole_temp) * effective_insolation;

    // Континентальный экстремум: суша сильнее раскаляется под высоким солнцем и сильнее стынет у полюсов
    if !is_ocean {
        let cont_factor =
            1.0 - (precalc.moderation[i] / config.base_ocean_moderation.max(0.01)).clamp(0.0, 1.0);
        raw_temp += cont_factor * (base_insolation - 0.42) * 10.5;
    }

    let moderation = precalc.moderation[i];
    let ocean_equilibrium = precalc.ocean_equilibrium[i];

    raw_temp * (1.0 - moderation) + ocean_equilibrium * moderation + precalc.bathymetry_shift[i]
}

#[inline]
pub(super) fn calc_albedo(
    temp: f32,
    is_ocean: bool,
    base_land_albedo: f32,
    config: &ClimateConfig,
) -> f32 {
    let base_albedo = if is_ocean {
        base_land_albedo.max(config.ocean_albedo)
    } else {
        base_land_albedo
    };
    let t = (temp - (config.freezing_temp - 4.0)) / 6.0;
    let ice_blend = smoothstep(t);

    let effective_ice_albedo = if !is_ocean && base_land_albedo < 0.17 {
        let mask = smoothstep((base_land_albedo - 0.10) / 0.25);
        config.ice_albedo * (0.62 + 0.38 * mask)
    } else {
        config.ice_albedo
    };

    effective_ice_albedo * (1.0 - ice_blend) + base_albedo * ice_blend
}