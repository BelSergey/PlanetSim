// src/viewport/camera_3d.rs

use super::StarSystemRoot;
use crate::world::WorldConfig;

#[cfg(target_os = "macos")]
use bevy::input::gestures::PinchGesture;
use bevy::{
    input::mouse::{AccumulatedMouseMotion, MouseWheel},
    prelude::*,
};

#[derive(Component, Default)]
pub struct SystemInertia {
    pub velocity: Vec2,
}

/// Вращает всю звёздную систему мышью с инерцией.
pub fn rotate_system(
    mut query: Query<(&mut Transform, &mut SystemInertia), With<StarSystemRoot>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    accumulated_mouse: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();

    if dt == 0.0 {
        return;
    }

    let Ok((mut transform, mut inertia)) = query.single_mut() else {
        return;
    };

    let sensitivity = 0.005;

    if mouse_button.pressed(MouseButton::Left) {
        let current_velocity = accumulated_mouse.delta / dt;
        inertia.velocity = inertia.velocity.lerp(current_velocity, 15.0 * dt);
    } else {
        let friction = 3.0;
        inertia.velocity *= (-friction * dt).exp();

        if inertia.velocity.length() < 1.0 {
            inertia.velocity = Vec2::ZERO;
        }
    }

    if inertia.velocity != Vec2::ZERO {
        let frame_delta = inertia.velocity * dt;

        let yaw = Quat::from_rotation_y(frame_delta.x * sensitivity);
        let pitch = Quat::from_rotation_x(frame_delta.y * sensitivity);

        transform.rotation = yaw * pitch * transform.rotation;
        transform.rotation = transform.rotation.normalize();
    }
}

/// Возвращает “стандартный” зум для заданного радиуса.
/// Для radius = 5.0 это 20.0 — как в текущей версии.
pub fn default_zoom_distance(radius: f32) -> f32 {
    radius * 4.0
}

/// Минимальный и максимальный зум, масштабируются от радиуса.
/// Для radius = 5.0 дают 4.5 и 20.0 — как в текущей версии.
fn zoom_limits(radius: f32) -> (f32, f32) {
    (radius * 0.9, radius * 4.0)
}

/// Система для установки стандартного зума при старте сцены.
/// Запускать в PostStartup, чтобы камера уже существовала.
pub fn set_default_camera_zoom(
    config: Res<WorldConfig>,
    mut query: Query<&mut Transform, With<Camera3d>>,
) {
    if let Ok(mut transform) = query.single_mut() {
        transform.translation.z = default_zoom_distance(config.radius);
    }
}

/// Зум от колеса мыши и трекпада (MouseWheel), а также от PinchGesture на macOS.
pub fn zoom_camera(
    mut scroll_events: MessageReader<MouseWheel>,
    #[cfg(target_os = "macos")] mut pinch_events: MessageReader<PinchGesture>,
    mut query: Query<&mut Transform, With<Camera3d>>,
    config: Res<WorldConfig>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };

    let zoom_speed = 0.5;
    let (min_zoom, max_zoom) = zoom_limits(config.radius);

    // Трекпад/колесо на Windows, Linux и macOS
    for event in scroll_events.read() {
        // Для зума используем вертикальную составляющую
        let delta = event.y;
        transform.translation.z =
            (transform.translation.z - delta * zoom_speed).clamp(min_zoom, max_zoom);
    }

    // Нативный пинч только на macOS
    #[cfg(target_os = "macos")]
    for event in pinch_events.read() {
        // PinchGesture(f32): положительная delta — приближение
        let delta = event.0;
        transform.translation.z =
            (transform.translation.z - delta * zoom_speed).clamp(min_zoom, max_zoom);
    }
}

/// Зум с клавиатуры: + / - (основная и цифровая клавиатура).
pub fn keyboard_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut Transform, With<Camera3d>>,
    config: Res<WorldConfig>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };

    let step = 1.0;
    let (min_zoom, max_zoom) = zoom_limits(config.radius);

    // Приблизить: Shift + '=' или NumpadAdd
    if (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
        && keys.just_pressed(KeyCode::Equal)
    {
        transform.translation.z = (transform.translation.z - step).clamp(min_zoom, max_zoom);
    }
    if keys.just_pressed(KeyCode::NumpadAdd) {
        transform.translation.z = (transform.translation.z - step).clamp(min_zoom, max_zoom);
    }

    // Отдалить: '-' или NumpadSubtract
    if keys.just_pressed(KeyCode::Minus) {
        transform.translation.z = (transform.translation.z + step).clamp(min_zoom, max_zoom);
    }
    if keys.just_pressed(KeyCode::NumpadSubtract) {
        transform.translation.z = (transform.translation.z + step).clamp(min_zoom, max_zoom);
    }
}

/// Сброс зума к стандартному: клавиша '=' (без Shift) или средняя кнопка мыши.
pub fn reset_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut query: Query<&mut Transform, With<Camera3d>>,
    config: Res<WorldConfig>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };

    let reset =
        // '=' без Shift
        (keys.just_pressed(KeyCode::Equal)
            && !keys.pressed(KeyCode::ShiftLeft)
            && !keys.pressed(KeyCode::ShiftRight))
        // Средняя кнопка мыши
        || mouse.just_pressed(MouseButton::Middle);

    if reset {
        transform.translation.z = default_zoom_distance(config.radius);
    }
}
