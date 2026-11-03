//! linecon0: a portable SSH, Telnet and serial client for network engineers.
//!
//! With no arguments this opens the window; with arguments it is a command-line
//! client, which is handy on a jump box or in a script.

// Without this, Windows opens a console window behind the GUI.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod cli;
mod keys;
mod pane;
mod session_log;
mod style;
mod terminal;
mod terminal_view;
mod themes;
#[cfg(test)]
mod ui_tests;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        attach_console();
        std::process::exit(cli::run(args));
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("linecon0")
            .with_app_id("linecon")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([420.0, 260.0]),
        ..Default::default()
    };
    let location = linecon_config::ConfigLocation::detect();
    let result = eframe::run_native(
        "linecon",
        options,
        Box::new(move |_cc| Ok(Box::new(app::App::new(location)))),
    );
    if let Err(e) = result {
        eprintln!("linecon: could not open a window: {e}");
        std::process::exit(1);
    }
}

/// The GUI subsystem has no console, so borrow the parent's for command-line use.
#[cfg(windows)]
fn attach_console() {
    extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_console() {}
