use gtk4::prelude::*;
use libadwaita::{ApplicationWindow, Toast, ToastOverlay};

use crate::database::RuntimeBridge;

pub fn bind_window_shutdown(
    window: &ApplicationWindow,
    bridge: RuntimeBridge,
    overlay: &ToastOverlay,
) {
    let overlay = overlay.clone();
    window.connect_close_request(move |window| {
        use dzban_core::application::ApplicationPhase;
        let context = bridge.application_context();
        let phase = *context.subscribe_phase().borrow();
        match phase {
            ApplicationPhase::Closed => glib::Propagation::Proceed,
            ApplicationPhase::Draining => glib::Propagation::Stop,
            ApplicationPhase::Running => {
                match bridge.start_shutdown() {
                    Ok(completion) => {
                        let window = window.downgrade();
                        let overlay = overlay.clone();
                        glib::spawn_future_local(async move {
                            match completion.await {
                                Ok(Ok(())) => {
                                    if let Some(window) = window.upgrade() {
                                        window.close();
                                    }
                                }
                                Ok(Err(error)) => overlay.add_toast(Toast::new(&format!(
                                    "Cannot close Dzban: {error}. Finish or cancel running work and try again."
                                ))),
                                Err(error) => overlay.add_toast(Toast::new(&format!(
                                    "Shutdown did not complete: {error}"
                                ))),
                            }
                        });
                    }
                    Err(error) => overlay.add_toast(Toast::new(&error.to_string())),
                }
                glib::Propagation::Stop
            }
        }
    });
}
