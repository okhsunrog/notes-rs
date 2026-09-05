//! Reusable mobile system integration that is independent from application features.

use tauri::{
    Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(mobile)]
use tauri::Manager;

mod error;
#[cfg(mobile)]
mod mobile;
mod models;

pub use error::{Error, Result};
#[cfg(mobile)]
pub use mobile::MobileSystem;
pub use models::{SafeAreaInsets, StylusCapabilities};

#[cfg(mobile)]
pub trait MobileSystemExt<R: Runtime> {
    fn mobile_system(&self) -> &MobileSystem<R>;
}

#[cfg(mobile)]
impl<R: Runtime, T: Manager<R>> MobileSystemExt<R> for T {
    fn mobile_system(&self) -> &MobileSystem<R> {
        self.state::<MobileSystem<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("mobile-system")
        .setup(|app, api| {
            #[cfg(mobile)]
            {
                let integration = mobile::init(app, api)?;
                app.manage(integration);
            }
            #[cfg(not(mobile))]
            {
                let _ = (app, api);
            }
            Ok(())
        })
        .build()
}
