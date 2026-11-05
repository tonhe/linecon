//! linecon0: a portable SSH, Telnet and serial client for network engineers.
//!
//! With no arguments this opens the window; with arguments it is a command-line
//! client, which is handy on a jump box or in a script.

mod app;
mod cli;
#[cfg(not(target_os = "macos"))]
mod cpu_paint;
#[cfg(not(target_os = "macos"))]
mod cpu_window;
mod keys;
mod mcp;
mod native_menu;
mod openssh_keys;
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
mod update;

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
        std::process::exit(cli::run(args));
    }
    hide_own_console();
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

fn viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("linecon0")
        .with_app_id("linecon")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([420.0, 260.0])
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                .expect("the bundled icon is a valid PNG"),
        )
}

/// Whether to paint with the processor. A GPU emulated in software costs far more than doing
/// it directly, so that is what a machine with nothing else gets. macOS always has a GPU.
#[cfg(not(target_os = "macos"))]
fn use_processor() -> bool {
    match std::env::var("LINECON_RENDERER").as_deref() {
        Ok("cpu") => return true,
        Ok("wgpu" | "glow") => return false,
        _ => {}
    }
    if !cfg!(windows) {
        return false;
    }
    let instance = eframe::wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(eframe::wgpu::Backends::PRIMARY));
    !adapters
        .iter()
        .any(|a| a.get_info().device_type != eframe::wgpu::DeviceType::Cpu)
}

#[cfg(not(target_os = "macos"))]
fn run_with_processor(
    location: &linecon_config::ConfigLocation,
    smoke: Option<&std::path::Path>,
) -> Result<(), String> {
    update::clean_up_after_update();
    let hold = std::env::var("LINECON_SMOKE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    cpu_window::run(
        app::App::new(location.clone()),
        viewport(),
        smoke.map(|file| cpu_window::Smoke {
            file: file.to_path_buf(),
            frames: hold,
        }),
    )
}

fn run_window(smoke: Option<std::path::PathBuf>) -> i32 {
    let location = linecon_config::ConfigLocation::detect();
    let error_log = location.dir.join("linecon-error.log");
    install_panic_report(error_log.clone(), smoke.clone());
    let mut failures = Vec::new();
    #[cfg(not(target_os = "macos"))]
    if use_processor() {
        match run_with_processor(&location, smoke.as_deref()) {
            Ok(()) => return 0,
            Err(e) => failures.push(format!("processor: {e}")),
        }
    }
    for renderer in renderers() {
        let options = eframe::NativeOptions {
            renderer,
            // Flat colors have no banding to hide, and it costs every pixel on every frame.
            dithering: false,
            // Each queued frame is a frame of delay between a key and its echo.
            wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
                surface: eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY,
                ..Default::default()
            },
            viewport: viewport(),
            ..Default::default()
        };
        let location = location.clone();
        let smoke = smoke.clone();
        let result = eframe::run_native(
            "linecon",
            options,
            Box::new(move |cc| {
                update::clean_up_after_update();
                let wake = cc.egui_ctx.clone();
                native_menu::install(move || wake.request_repaint());
                let mut app = app::App::new(location);
                app.set_renderer_info(match &cc.wgpu_render_state {
                    Some(state) => {
                        let info = state.adapter.get_info();
                        format!(
                            "Drawn by {:?} on {} ({:?})",
                            info.backend, info.name, info.device_type
                        )
                    }
                    None => "Drawn by OpenGL".to_string(),
                });
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
    // Nothing else worked, and this asks for nothing but a window.
    #[cfg(not(target_os = "macos"))]
    if !failures.iter().any(|f| f.starts_with("processor")) {
        match run_with_processor(&location, smoke.as_deref()) {
            Ok(()) => return 0,
            Err(e) => failures.push(format!("processor: {e}")),
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

/// This is a console program, because the command line has to work properly from a
/// shell: the shell waits for it, and they do not compete for the keyboard. Started
/// from Explorer, Windows gives it a console of its own, which the window does not
/// want. A console with only this process in it is that case. Started from a shell
/// it stays attached, so any errors are visible there.
#[cfg(windows)]
fn hide_own_console() {
    extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    let mut list = [0u32; 4];
    unsafe {
        if GetConsoleProcessList(list.as_mut_ptr(), list.len() as u32) == 1 {
            FreeConsole();
        }
    }
}

#[cfg(not(windows))]
fn hide_own_console() {}
