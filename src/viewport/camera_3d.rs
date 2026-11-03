// src/viewport/camera_3d.rs

use super::StarSystemRoot;
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

/// Вращает всю звёздную систему вручную, не затрагивая ход орбитальной симуляции.
pub fn rotate_system(
    mut query: Query<(&mut Transform, &mut SystemInertia), With<StarSystemRoot>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    accumulated_mouse: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    layout: Res<ShellLayout>,
    mut egui_contexts: EguiContexts,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    let Ok((mut transform, mut inertia)) = query.single_mut() else {
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

    if inertia.velocity != Vec2::ZERO {
        let frame_delta = inertia.velocity * dt;
        let yaw = Quat::from_rotation_y(frame_delta.x * 0.005);
        let pitch = Quat::from_rotation_x(frame_delta.y * 0.005);
        transform.rotation = (yaw * pitch * transform.rotation).normalize();
    }
}

pub fn default_zoom_distance(radius: f32) -> f32 {
    radius * 4.0
}

fn zoom_limits(radius: f32) -> (f32, f32) {
    (radius * 0.9, radius * 4.0)
}

pub fn set_default_camera_zoom(
    config: Res<WorldConfig>,
    mut query: Query<&mut Transform, With<Camera3d>>,
) {
    if let Ok(mut transform) = query.single_mut() {
        transform.translation.z = default_zoom_distance(config.radius);
    }
}

pub fn zoom_camera(
    mut scroll_events: MessageReader<MouseWheel>,
    #[cfg(target_os = "macos")] mut pinch_events: MessageReader<PinchGesture>,
    mut query: Query<&mut Transform, With<Camera3d>>,
    config: Res<WorldConfig>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let (min_zoom, max_zoom) = zoom_limits(config.radius);
    for event in scroll_events.read() {
        transform.translation.z =
            (transform.translation.z - event.y * 0.5).clamp(min_zoom, max_zoom);
    }
    #[cfg(target_os = "macos")]
    for event in pinch_events.read() {
        transform.translation.z =
            (transform.translation.z - event.0 * 0.5).clamp(min_zoom, max_zoom);
    }
}

pub fn keyboard_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut Transform, With<Camera3d>>,
    config: Res<WorldConfig>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let (min_zoom, max_zoom) = zoom_limits(config.radius);
    if (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
        && keys.just_pressed(KeyCode::Equal)
    {
        transform.translation.z = (transform.translation.z - 1.0).clamp(min_zoom, max_zoom);
    }
    if keys.just_pressed(KeyCode::NumpadAdd) {
        transform.translation.z = (transform.translation.z - 1.0).clamp(min_zoom, max_zoom);
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        transform.translation.z = (transform.translation.z + 1.0).clamp(min_zoom, max_zoom);
    }
}

pub fn reset_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut query: Query<&mut Transform, With<Camera3d>>,
    config: Res<WorldConfig>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let reset = (keys.just_pressed(KeyCode::Equal)
        && !keys.pressed(KeyCode::ShiftLeft)
        && !keys.pressed(KeyCode::ShiftRight))
        || mouse.just_pressed(MouseButton::Middle);
    if reset {
        transform.translation.z = default_zoom_distance(config.radius);
    }
}
