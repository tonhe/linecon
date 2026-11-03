//! linecon0: a portable SSH, Telnet and serial client for network engineers.
//!
//! With no arguments this opens the window; with arguments it is a command-line
//! client, which is handy on a jump box or in a script.

// Without this, Windows opens a console window behind the GUI.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod cli;
mod keys;
mod mcp;
mod pane;
mod serial_ports;
mod session_log;
mod sftp_pane;
mod style;
mod terminal;
mod terminal_view;
mod themes;
#[cfg(test)]
mod ui_tests;

fn main() {
    // Older macOS passes a process serial number when launching from Finder.
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("-psn_"))
        .collect();
    if let ["--smoke-test", file] = args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        std::process::exit(run_window(Some(file.into())));
    }
    if !args.is_empty() {
        attach_console();
        std::process::exit(cli::run(args));
    }
    std::process::exit(run_window(None));
}

/// OpenGL is the lighter of the two and the usual default. Windows starts with
/// Direct3D, because it has a software adapter for machines with no GPU, like cloud
/// desktops and CI, where OpenGL may be missing or too old to use.
fn renderers() -> [eframe::Renderer; 2] {
    use eframe::Renderer::{Glow, Wgpu};
    match std::env::var("LINECON_RENDERER").as_deref() {
        Ok("wgpu") => return [Wgpu, Wgpu],
        Ok("glow") => return [Glow, Glow],
        _ => {}
    }
    if cfg!(windows) {
        [Wgpu, Glow]
    } else {
        [Glow, Wgpu]
    }
}

fn run_window(smoke: Option<std::path::PathBuf>) -> i32 {
    let location = linecon_config::ConfigLocation::detect();
    let error_log = location.dir.join("linecon-error.log");
    install_panic_report(error_log.clone(), smoke.clone());
    let mut failures = Vec::new();
    for renderer in renderers() {
        let options = eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_title("linecon0")
                .with_app_id("linecon")
                .with_inner_size([1100.0, 700.0])
                .with_min_inner_size([420.0, 260.0])
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                        .expect("the bundled icon is a valid PNG"),
                ),
            ..Default::default()
        };
        let location = location.clone();
        let smoke = smoke.clone();
        let result = eframe::run_native(
            "linecon",
            options,
            Box::new(move |_cc| {
                let app = app::App::new(location);
                Ok(match smoke {
                    Some(file) => Box::new(SmokeTest {
                        app,
                        file,
                        renderer,
                        frames: 0,
                    }),
                    None => Box::new(app),
                })
            }),
        );
        match result {
            Ok(()) => return 0,
            Err(e) => failures.push(format!("{renderer:?}: {e}")),
        }
    }
    report_fatal(
        &format!(
            "linecon0 could not open its window, because this computer offered no usable graphics.\n\n{}",
            failures.join("\n")
        ),
        &error_log,
        smoke.as_deref(),
    );
    1
}

/// `--smoke-test <file>`: opens the real window, draws a few frames, says how it
/// went in the file and exits. CI uses this to prove the window opens at all.
struct SmokeTest {
    app: app::App,
    file: std::path::PathBuf,
    renderer: eframe::Renderer,
    frames: u32,
}

impl eframe::App for SmokeTest {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, ui, frame);
        self.frames += 1;
        ui.ctx().request_repaint();
        let hold: u32 = std::env::var("LINECON_SMOKE_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        if self.frames == hold {
            let _ = std::fs::write(
                &self.file,
                format!("window opened with {:?}\n", self.renderer),
            );
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// With no console, a failure nobody can see looks like nothing happening. So it
/// goes to a file, and on Windows to a message box as well.
fn report_fatal(message: &str, log: &std::path::Path, smoke: Option<&std::path::Path>) {
    eprintln!("{message}");
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(log, message);
    if let Some(file) = smoke {
        let _ = std::fs::write(file, format!("FAILED: {message}\n"));
    }
    #[cfg(windows)]
    if smoke.is_none() {
        message_box(&format!("{message}\n\nThis is saved in {}", log.display()));
    }
}

fn install_panic_report(log: std::path::PathBuf, smoke: Option<std::path::PathBuf>) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        report_fatal(
            &format!("linecon0 stopped unexpectedly.\n\n{info}"),
            &log,
            smoke.as_deref(),
        );
    }));
}

#[cfg(windows)]
fn message_box(text: &str) {
    extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            title: *const u16,
            kind: u32,
        ) -> i32;
    }
    const MB_ICONERROR: u32 = 0x10;
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (text, title) = (wide(text), wide("linecon0"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR,
        )
    };
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
