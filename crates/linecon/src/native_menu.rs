//! The macOS menu bar. Other platforms show their menus inside the window.

use crate::app::Command;

/// What each native menu item runs, by index.
#[cfg(target_os = "macos")]
pub(crate) static COMMANDS: std::sync::Mutex<Vec<Command>> = std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
mod platform {
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

    use super::*;

    /// Installs the menu bar. It has to be called on the main thread, once the app exists.
    pub fn install() {
        let columns = crate::app::native_menu_items(crate::app::has_update_key());
        let menu = Menu::new();
        for (index, (title, entries)) in columns.into_iter().enumerate() {
            let submenu = Submenu::new(title, true);
            for entry in entries {
                let _ = match entry {
                    None => submenu.append(&PredefinedMenuItem::separator()),
                    Some((label, id)) => {
                        // No accelerators: shortcuts are handled by the window, and a
                        // menu that also took them would run each one twice or lose it.
                        submenu.append(&MenuItem::with_id(id.to_string(), label, true, None))
                    }
                };
            }
            if index == 0 {
                let _ = submenu.append(&PredefinedMenuItem::separator());
                let _ = submenu.append(&PredefinedMenuItem::hide(None));
                let _ = submenu.append(&PredefinedMenuItem::hide_others(None));
                let _ = submenu.append(&PredefinedMenuItem::show_all(None));
                let _ = submenu.append(&PredefinedMenuItem::separator());
                let _ = submenu.append(&PredefinedMenuItem::quit(None));
            }
            let _ = menu.append(&submenu);
        }
        menu.init_for_nsapp();
        // The bar lasts as long as the program.
        std::mem::forget(menu);
    }

    pub fn take_chosen() -> Vec<Command> {
        let commands = COMMANDS.lock().unwrap();
        let mut chosen = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let index = event.id().0.parse::<usize>().ok();
            if let Some(command) = index.and_then(|i| commands.get(i)) {
                chosen.push(command.clone());
            }
        }
        chosen
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub fn install() {}

    pub fn take_chosen() -> Vec<Command> {
        Vec::new()
    }
}

pub use platform::{install, take_chosen};
