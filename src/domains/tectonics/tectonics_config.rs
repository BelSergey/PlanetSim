// src/domains/tectonics/tectonics_config.rs

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Resource, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TectonicsConfig {
    // === 1. Параметры плит и кластерной генерации кратонов ===
    pub num_plates: usize,
    /// Целевая доля континентальной коры / бюджет суши (0.0..1.0)
    pub continental_ratio: f64,
    /// Доля чисто океанических плит без континентального ядра (аналог Тихоокеанской плиты и Наски)
    pub pure_oceanic_plate_fraction: f32,
    /// Разброс эксцентриситета кратонных ядер и неровности континентальных окраин
    pub craton_eccentricity: f32,
    /// Разброс случайной базовой высоты плит (изостатический подпор)
    pub plate_base_height_variance: f32,
    pub plate_dir_iterations: usize,
    /// Количество макро-кластеров зарождения древних кратонов (срастающихся в крупные материки со швами)
    pub craton_cluster_count: usize,
    /// Общее число первичных кратонных ядер (террейнов), включая одиночные микроконтиненты
    pub craton_kernel_count: usize,

    // === 2. Параметры пошаговой симуляции движения ===
    /// Количество шагов симуляции при генерации
    pub simulation_steps: usize,
    /// Шаг времени одного тика (в условных единицах вращения)
    pub time_step: f32,
    pub angular_vel_min: f32,
    pub angular_vel_max: f32,
    /// Случайное блуждание полюсов Эйлера со временем (создаёт естественные изгибы цепей горячих точек)
    pub euler_pole_wander: f32,

    // === 3. Геодинамические силы (Форсайт — Уэда) ===
    /// Главная сила: тяга тонущего холодного океанического слэба в зонах субдукции
    pub slab_pull_strength: f32,
    /// Толчок от приподнятых молодых срединно-океанических хребтов
    pub ridge_push_strength: f32,
    /// Торможение и отталкивание при лобовом столкновении двух легких континентов
    pub collision_resistance_strength: f32,
    /// Базовое вязкое трение подошвы плиты об астеносферу
    pub mantle_drag_coeff: f32,
    /// Множитель трения под глубокими континентальными корнями (кратонами)
    pub continental_drag_mult: f32,
    /// Сила радиального расталкивания плит от импульсов мантийных плюмов
    pub plume_push_strength: f32,

    // === 4. Физический цикл Уилсона (раскол по натяжению, швы и слияние) ===
    /// Минимальная доля площади планеты, ниже которой остаток океанической плиты поглощается соседом
    pub plate_min_area_ratio: f32,
    /// Опорная доля площади крупной плиты для расчёта теплоизоляции мантии
    pub plate_split_area_ratio: f32,
    /// Критический порог внутреннего растягивающего напряжения для раскола плиты по швам
    pub plate_fracture_threshold: f32,
    /// Скорость накопления тепла под крупными континентальными щитами (ослабляет кору суперконтинента)
    pub supercontinent_heat_rate: f32,
    /// Во сколько раз древние сутуры (швы слияния кратонов) снижают локальную прочность на разрыв
    pub suture_weakness_mult: f32,

    // === 5. Параметры коры и изостазии Эйри ===
    pub cont_base_height: f32,
    pub ocean_base_height: f32,
    pub cont_crust_thickness_km: f32,
    pub ocean_crust_thickness_km: f32,
    pub max_crust_thickness_km: f32,
    pub cont_crust_density: f32,
    pub young_ocean_density: f32,
    pub old_ocean_density: f32,
    pub mantle_density: f32,
    /// Возраст (в усл. млн лет), за который океаническая литосфера полностью остывает и уплотняется
    pub oceanic_mature_age: f32,

    // === 6. Взаимодействие на границах (в радианах) ===
    /// Физическая ширина активной зоны деформации у границы плит (рад)
    pub edge_threshold: f32,
    /// Скорость утолщения коры при коллизии и субдукции
    pub convergence_thickening_rate: f32,
    /// Скорость утончения коры при континентальном рифтогенезе
    pub rift_thinning_rate: f32,
    /// Глубина прогиба океанического желоба при субдукции
    pub trench_depth_amp: f32,
    /// Угловой отступ вулканической дуги вглубь верхней плиты от желоба (рад)
    pub arc_inland_offset_rad: f32,
    /// Амплитуда ступенчатых трансформных разломов на сдвиговых участках СОХ
    pub transform_fault_amp: f32,

    // === 7. Нелинейная эрозия, память орогенеза и осадконакопление ===
    pub step_erosion_rate: f32,
    pub sediment_deposition_efficiency: f32,
    pub diffusion_strength: f32,
    pub slope_erosion_factor: f32,
    pub diffusion_max_strength: f32,
    /// Скорость затухания памяти древних орогенов (малое значение сохраняет Урал и Аппалачи надолго)
    pub orogen_memory_decay: f32,
    /// Показатель нелинейности склоновой эрозии (`|grad h|^k`, сохраняет острые гребни молодых гор)
    pub nonlinear_erosion_pow: f32,

    // === 8. Мантийные плюмы, траппы (LIP) и вулканы ===
    pub hotspot_count: usize,
    /// Минимальная угловая дистанция дрейфа плиты над плюмом для рождения нового вулкана цепи (рад)
    pub hotspot_spacing_rad: f32,
    pub hotspot_height_min: f32,
    pub hotspot_height_max: f32,
    pub hotspot_radius_base: f32,
    pub hotspot_radius_growth: f32,
    pub volcano_erosion_rate: f32,
    pub volcano_profile_pow: i32,
    pub volcano_radius_per_height: f32,
    pub volcano_radius_min: f32,
    pub volcano_noise_scale: f64,
    pub volcano_noise_base: f32,
    pub volcano_noise_mult: f32,

    // === 9. Шум границ и финальной детализации ===
    pub warp_strength: f32,
    pub warp_scale: f64,
    pub warp_offsets: [[f64; 3]; 3],
    pub boundary_noise_scale: f64,
    pub boundary_noise_base: f32,
    pub boundary_noise_amp: f32,
    pub detail_noise_scale: f64,
    pub detail_noise_amp: f32,
    pub elevation_min_clamp: f32,

    // === 10. Определение океанов, засыпание замкнутых впадин и континентальность ===
    /// Минимальная доля клеток планеты, чтобы водный бассейн считался океаном (например, 0.015 = 1.5%)
    pub min_ocean_size_ratio: f32,
    /// Порог глубины: в бассейне должна быть хотя бы одна абиссальная клетка ниже этого уровня
    pub ocean_abyssal_threshold: f32,
    /// До какой высоты осадки подтягивают запертые внутри материков не-океанические глубокие котловины
    pub trapped_basin_floor_elev: f32,
    /// Масштаб океанической континентальности в радианах
    pub ocean_continentality_scale: f32,
    /// Масштаб сухопутной континентальности в радианах
    pub land_continentality_scale: f32,
}

impl Default for TectonicsConfig {
    fn default() -> Self {
        Self {
            num_plates: 15,
            continental_ratio: 0.36,
            pure_oceanic_plate_fraction: 0.28,
            craton_eccentricity: 0.85,
            plate_base_height_variance: 0.035,
            plate_dir_iterations: 18,
            craton_cluster_count: 4,
            craton_kernel_count: 28,

            simulation_steps: 35,
            time_step: 0.028,
            angular_vel_min: 0.45,
            angular_vel_max: 1.85,
            euler_pole_wander: 0.14,

            slab_pull_strength: 2.6,
            ridge_push_strength: 1.1,
            collision_resistance_strength: 3.4,
            mantle_drag_coeff: 0.85,
            continental_drag_mult: 2.1,
            plume_push_strength: 1.4,

            plate_min_area_ratio: 0.006,
            plate_split_area_ratio: 0.16,
            plate_fracture_threshold: 1.35,
            supercontinent_heat_rate: 0.42,
            suture_weakness_mult: 2.2,

            cont_base_height: 0.12,
            ocean_base_height: -0.35,
            cont_crust_thickness_km: 35.0,
            ocean_crust_thickness_km: 7.0,
            max_crust_thickness_km: 72.0,
            cont_crust_density: 2700.0,
            young_ocean_density: 2890.0,
            old_ocean_density: 3130.0,
            mantle_density: 3300.0,
            oceanic_mature_age: 1.2,

            edge_threshold: 0.38,
            convergence_thickening_rate: 26.0,
            rift_thinning_rate: 19.0,
            trench_depth_amp: 0.24,
            arc_inland_offset_rad: 0.055,
            transform_fault_amp: 0.045,

            step_erosion_rate: 0.06,
            sediment_deposition_efficiency: 0.75,
            diffusion_strength: 0.20,
            slope_erosion_factor: 2.2,
            diffusion_max_strength: 0.35,
            orogen_memory_decay: 0.09,
            nonlinear_erosion_pow: 1.65,

            hotspot_count: 12,
            hotspot_spacing_rad: 0.048,
            hotspot_height_min: 0.26,
            hotspot_height_max: 0.72,
            hotspot_radius_base: 0.045,
            hotspot_radius_growth: 0.035,
            volcano_erosion_rate: 0.32,
            volcano_profile_pow: 2,
            volcano_radius_per_height: 0.045,
            volcano_radius_min: 0.018,
            volcano_noise_scale: 24.0,
            volcano_noise_base: 0.82,
            volcano_noise_mult: 0.22,

            warp_strength: 0.64,
            warp_scale: 0.8,
            warp_offsets: [[12.3, 45.6, 78.9], [98.7, 65.4, 32.1], [55.5, 11.1, 99.9]],
            boundary_noise_scale: 14.0,
            boundary_noise_base: 0.65,
            boundary_noise_amp: 0.75,
            detail_noise_scale: 4.5,
            detail_noise_amp: 1.2,
            elevation_min_clamp: -0.90,

            min_ocean_size_ratio: 0.015,
            ocean_abyssal_threshold: -0.20,
            trapped_basin_floor_elev: -0.025,
            ocean_continentality_scale: 0.28,
            land_continentality_scale: 0.45,
        }
    }
}
