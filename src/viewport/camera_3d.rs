// src/viewport/camera_3d.rs

use super::{Globe, MapCameraAngle, StarSystemRoot, ViewMode};
use crate::{ui::ShellLayout, world::WorldConfig};
#[cfg(target_os = "macos")]
use bevy::input::gestures::PinchGesture;
use bevy::{
    input::mouse::{AccumulatedMouseMotion, MouseWheel},
    prelude::*,
};
use bevy_egui::EguiContexts;

#[derive(Component, Default)]
pub struct SystemInertia {
    pub velocity: Vec2,
}

pub fn default_transform_for_mode(_view_mode: ViewMode, _map_angle: MapCameraAngle) -> Transform {
    Transform::default()
}

pub fn default_zoom_distance_for_mode(radius: f32, view_mode: ViewMode) -> f32 {
    match view_mode {
        ViewMode::StarSystem => radius * 4.0,
        ViewMode::GlobeSolo => radius * 3.0,
        ViewMode::Map3D => radius * 3.7,
    }
}

pub fn default_camera_transform(
    radius: f32,
    view_mode: ViewMode,
    map_angle: MapCameraAngle,
) -> Transform {
    let distance = default_zoom_distance_for_mode(radius, view_mode);
    if view_mode == ViewMode::Map3D && map_angle == MapCameraAngle::Isometric {
        Transform::from_xyz(0.0, -distance * 0.58, distance * 0.815).looking_at(Vec3::ZERO, Vec3::Y)
    } else {
        Transform::from_xyz(0.0, 0.0, distance)
    }
}

fn zoom_limits(radius: f32, view_mode: ViewMode) -> (f32, f32) {
    match view_mode {
        ViewMode::StarSystem => (radius * 0.9, radius * 6.0),
        ViewMode::GlobeSolo => (radius * 1.1, radius * 5.0),
        ViewMode::Map3D => (radius * 1.5, radius * 12.0), // Позволяем отдаляться дальше для карты
    }
}

fn set_camera_distance(transform: &mut Transform, distance: f32) {
    let direction = transform.translation.normalize_or_zero();
    transform.translation = if direction == Vec3::ZERO {
        Vec3::Z * distance
    } else {
        direction * distance
    };
}

/// В системе вращается корневой узел; в отдельном режиме вращается только меш глобуса,
/// а в режиме карты управление работает по принципу Tabletop Simulator.
pub fn rotate_system(
    view_mode: Res<ViewMode>,
    _map_angle: Res<MapCameraAngle>,
    world_config: Res<WorldConfig>,
    mut root_query: Query<
        (&mut Transform, &mut SystemInertia),
        (With<StarSystemRoot>, Without<Globe>),
    >,
    mut globe_query: Query<&mut Transform, (With<Globe>, Without<StarSystemRoot>)>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    accumulated_mouse: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    layout: Res<ShellLayout>,
    mut egui_contexts: EguiContexts,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let Ok((mut root_transform, mut inertia)) = root_query.single_mut() else {
        return;
    };

    let pointer_over_ui = egui_contexts.ctx_mut().map_or(true, |ctx| {
        let over_shell = ctx
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| {
                layout.top.contains(pos)
                    || layout.bottom.contains(pos)
                    || layout.left.contains(pos)
                    || layout.right.contains(pos)
            });
        over_shell || ctx.is_pointer_over_egui()
    });

    if *view_mode == ViewMode::Map3D {
        // --- РЕЖИМ 3D-КАРТЫ (Стиль Tabletop Simulator) ---
        // Без инерции для мгновенного отклика (как в RTS)
        inertia.velocity = Vec2::ZERO;
        let mouse_delta = if !pointer_over_ui {
            accumulated_mouse.delta
        } else {
            Vec2::ZERO
        };

        let is_rotate = mouse_button.pressed(MouseButton::Right);
        let is_pan =
            mouse_button.pressed(MouseButton::Left) || mouse_button.pressed(MouseButton::Middle);

        if mouse_delta != Vec2::ZERO {
            if is_rotate {
                // ПКМ: Вращение карты
                let yaw = Quat::from_rotation_y(-mouse_delta.x * 0.005);
                let pitch = Quat::from_rotation_x(-mouse_delta.y * 0.005);
                root_transform.rotation = (yaw * pitch * root_transform.rotation).normalize();
            } else if is_pan {
                // ЛКМ: Перемещение (панорамирование)
                let pan_scale = world_config.radius * 0.006;
                let max_x = std::f32::consts::PI * world_config.radius;
                let max_y = std::f32::consts::FRAC_PI_2 * world_config.radius;
                root_transform.translation.x =
                    (root_transform.translation.x - mouse_delta.x * pan_scale).clamp(-max_x, max_x);
                root_transform.translation.y =
                    (root_transform.translation.y + mouse_delta.y * pan_scale).clamp(-max_y, max_y);
            }
        }

        // Перемещение на клавиатуре (WASD / Стрелки)
        if !pointer_over_ui {
            let speed = world_config.radius * 1.5 * dt;
            let mut move_vec = Vec3::ZERO;
            if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
                move_vec.y += speed;
            }
            if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
                move_vec.y -= speed;
            }
            if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
                move_vec.x -= speed;
            }
            if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
                move_vec.x += speed;
            }

            if move_vec != Vec3::ZERO {
                // Двигаем относительно текущего поворота
                let rotated_move = root_transform.rotation * move_vec;
                root_transform.translation += rotated_move;
                let max_x = std::f32::consts::PI * world_config.radius;
                let max_y = std::f32::consts::FRAC_PI_2 * world_config.radius;
                root_transform.translation.x = root_transform.translation.x.clamp(-max_x, max_x);
                root_transform.translation.y = root_transform.translation.y.clamp(-max_y, max_y);
            }
        }
        return;
    }

    // --- РЕЖИМ ОРБИТЫ / ГЛОБУСА (С инерцией) ---
    let dragging = mouse_button.pressed(MouseButton::Left) && !pointer_over_ui;
    if dragging {
        let current_velocity = accumulated_mouse.delta / dt;
        inertia.velocity = inertia
            .velocity
            .lerp(current_velocity, (15.0 * dt).clamp(0.0, 1.0));
    } else {
        inertia.velocity *= (-3.0 * dt).exp();
        if inertia.velocity.length() < 1.0 {
            inertia.velocity = Vec2::ZERO;
        }
    }

    if inertia.velocity == Vec2::ZERO {
        return;
    }

    let frame_delta = inertia.velocity * dt;
    match *view_mode {
        ViewMode::StarSystem => {
            let yaw = Quat::from_rotation_y(frame_delta.x * 0.005);
            let pitch = Quat::from_rotation_x(frame_delta.y * 0.005);
            root_transform.rotation = (yaw * pitch * root_transform.rotation).normalize();
        }
        ViewMode::GlobeSolo => {
            if let Ok(mut globe_transform) = globe_query.single_mut() {
                let yaw = Quat::from_rotation_y(frame_delta.x * 0.005);
                let pitch = Quat::from_rotation_x(frame_delta.y * 0.005);
                globe_transform.rotation = (yaw * pitch * globe_transform.rotation).normalize();
            }
        }
        _ => {}
    }
}

/// Возвращает камеру к центру и корректному ракурсу после смены режима/проекции.
pub fn apply_view_mode_camera(
    config: Res<WorldConfig>,
    view_mode: Res<ViewMode>,
    map_angle: Res<MapCameraAngle>,
    mut last_mode: Local<Option<(ViewMode, MapCameraAngle)>>,
    mut root_query: Query<
        (&mut Transform, &mut SystemInertia),
        (With<StarSystemRoot>, Without<Camera3d>, Without<Globe>),
    >,
    mut globe_query: Query<
        &mut Transform,
        (With<Globe>, Without<Camera3d>, Without<StarSystemRoot>),
    >,
    mut camera_query: Query<
        &mut Transform,
        (With<Camera3d>, Without<StarSystemRoot>, Without<Globe>),
    >,
) {
    let current = (*view_mode, *map_angle);
    if *last_mode == Some(current) {
        return;
    }
    *last_mode = Some(current);

    if let Ok((mut root_transform, mut inertia)) = root_query.single_mut() {
        *root_transform = default_transform_for_mode(*view_mode, *map_angle);
        inertia.velocity = Vec2::ZERO;
    }
    if let Ok(mut globe_transform) = globe_query.single_mut() {
        globe_transform.rotation = Quat::IDENTITY;
    }
    if let Ok(mut camera_transform) = camera_query.single_mut() {
        *camera_transform = default_camera_transform(config.radius, *view_mode, *map_angle);
    }
}

pub fn set_default_camera_zoom(
    config: Res<WorldConfig>,
    view_mode: Res<ViewMode>,
    map_angle: Res<MapCameraAngle>,
    mut query: Query<&mut Transform, With<Camera3d>>,
) {
    if let Ok(mut transform) = query.single_mut() {
        *transform = default_camera_transform(config.radius, *view_mode, *map_angle);
    }
}

pub fn zoom_camera(
    config: Res<WorldConfig>,
    view_mode: Res<ViewMode>,
    mut scroll_events: MessageReader<MouseWheel>,
    #[cfg(target_os = "macos")] mut pinch_events: MessageReader<PinchGesture>,
    mut query: Query<&mut Transform, With<Camera3d>>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let mut delta = 0.0;
    for event in scroll_events.read() {
        delta += event.y * 0.6;
    }
    #[cfg(target_os = "macos")]
    for event in pinch_events.read() {
        delta += event.0 * 3.0;
    }
    if delta.abs() > 1e-5 {
        let (min_zoom, max_zoom) = zoom_limits(config.radius, *view_mode);
        let distance = (transform.translation.length() - delta).clamp(min_zoom, max_zoom);
        set_camera_distance(&mut transform, distance);
    }
}

pub fn keyboard_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    config: Res<WorldConfig>,
    view_mode: Res<ViewMode>,
    mut query: Query<&mut Transform, With<Camera3d>>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let zoom_in = (keys.pressed(KeyCode::Equal) && shift) || keys.pressed(KeyCode::NumpadAdd);
    let zoom_out = keys.pressed(KeyCode::Minus) || keys.pressed(KeyCode::NumpadSubtract);
    if zoom_in || zoom_out {
        let direction = if zoom_in { -1.0 } else { 1.0 };
        let speed = config.radius * 2.0;
        let (min_zoom, max_zoom) = zoom_limits(config.radius, *view_mode);
        let distance = (transform.translation.length() + direction * speed * time.delta_secs())
            .clamp(min_zoom, max_zoom);
        set_camera_distance(&mut transform, distance);
    }
}

pub fn reset_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    config: Res<WorldConfig>,
    view_mode: Res<ViewMode>,
    map_angle: Res<MapCameraAngle>,
    mut query: Query<&mut Transform, With<Camera3d>>,
) {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let reset =
        (keys.just_pressed(KeyCode::Equal) && !shift) || mouse.just_pressed(MouseButton::Middle);
    if reset {
        if let Ok(mut transform) = query.single_mut() {
            *transform = default_camera_transform(config.radius, *view_mode, *map_angle);
        }
    }
}
