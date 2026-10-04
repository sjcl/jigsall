use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    window::{PrimaryWindow, WindowCreated},
    winit::WINIT_WINDOWS,
};
use winit::window::Icon;

pub struct WindowIconPlugin;

impl Plugin for WindowIconPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, set_window_icon);
    }
}

fn set_window_icon(
    mut created: MessageReader<WindowCreated>,
    primary: Query<(), With<PrimaryWindow>>,
    mut icon: Local<Option<Icon>>,
    _main_thread: NonSendMarker,
) {
    for event in created.read() {
        if !primary.contains(event.window) {
            continue;
        }
        WINIT_WINDOWS.with_borrow(|windows| {
            let Some(window) = windows.get_window(event.window) else {
                return;
            };
            let icon = icon.get_or_insert_with(|| {
                let image = image::load_from_memory(include_bytes!("../../assets/menu-icon.png"))
                    .expect("embedded window icon is a valid PNG")
                    .to_rgba8();
                let (width, height) = image.dimensions();
                Icon::from_rgba(image.into_raw(), width, height)
                    .expect("failed to create the window icon")
            });
            window.set_window_icon(Some(icon.clone()));
            #[cfg(target_os = "windows")]
            {
                use winit::platform::windows::WindowExtWindows;
                window.set_taskbar_icon(Some(icon.clone()));
            }
        });
    }
}
