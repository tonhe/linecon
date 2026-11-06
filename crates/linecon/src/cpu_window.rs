//! The window on a machine with no GPU: the program's own event loop, painting with the
//! processor and handing the pixels straight to the system.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ViewportCommand, ViewportId, ViewportInfo};
use egui_winit::{WindowGeometry, WindowSettings};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::app::App;
use crate::cpu_paint::Painter;

/// What `--smoke-test` needs: how many frames to draw before writing this file and leaving.
pub struct Smoke {
    pub file: std::path::PathBuf,
    pub frames: u32,
}

struct Open {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    state: egui_winit::State,
    info: ViewportInfo,
}

struct Runner {
    app: App,
    ctx: egui::Context,
    builder: egui::ViewportBuilder,
    proxy: EventLoopProxy<Duration>,
    open: Option<Open>,
    painter: Painter,
    /// What is on screen, to send only what changed: this may be going over remote desktop.
    shown: Vec<u32>,
    shown_size: (usize, usize),
    next_frame: Option<Instant>,
    smoke: Option<Smoke>,
    frames: u32,
    error: Option<String>,
    /// Where the window's size and place are kept between runs.
    place_file: Option<std::path::PathBuf>,
    placed: Option<WindowSettings>,
    /// Size and place before maximizing, to come back to.
    last_normal: Option<WindowGeometry>,
}

/// The largest monitor in points, so a saved size never asks for more than there is.
fn largest_monitor(event_loop: &ActiveEventLoop) -> Option<egui::Vec2> {
    event_loop
        .available_monitors()
        .map(|m| {
            let size = m.size().to_logical::<f32>(m.scale_factor());
            egui::vec2(size.width, size.height)
        })
        .reduce(|a, b| a.max(b))
}

fn load_place(file: &std::path::Path) -> Option<WindowSettings> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

impl Runner {
    fn remember_place(&self) {
        let (Some(file), Some(open)) = (&self.place_file, &self.open) else {
            return;
        };
        let settings =
            WindowSettings::from_window(self.ctx.zoom_factor(), &open.window, self.last_normal);
        if let Ok(text) = serde_json::to_vec(&settings) {
            if let Some(dir) = file.parent() {
                let _ = linecon_engine::private_files::create_dir(dir);
            }
            let tmp = file.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, file);
            }
        }
    }

    fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let zoom = self.ctx.zoom_factor();
        let mut builder = self.builder.clone();
        self.placed = self.place_file.as_deref().and_then(load_place);
        if let Some(placed) = &mut self.placed {
            if let Some(largest) = largest_monitor(event_loop) {
                placed.clamp_size_to_sane_values(largest);
            }
            // A monitor that was unplugged since would leave the window out of sight.
            placed.clamp_position_to_monitors(zoom, event_loop);
            builder = placed.initialize_viewport_builder(zoom, builder);
        }
        let window = egui_winit::create_window(&self.ctx, event_loop, &builder)
            .map_err(|e| e.to_string())?;
        if let Some(placed) = &self.placed {
            placed.initialize_window(&window);
            self.last_normal = Some(placed.normal_geometry());
        }
        let window = Arc::new(window);
        let context = softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
        let surface =
            softbuffer::Surface::new(&context, window.clone()).map_err(|e| e.to_string())?;
        let state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(8192),
        );
        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &self.ctx, &window, true);
        if let Some(placed) = &self.placed {
            placed.before_first_show(&window);
        }
        window.set_visible(true);
        if let Some(placed) = &self.placed {
            placed.after_first_show(&window);
        }
        window.request_redraw();
        self.open = Some(Open {
            window,
            surface,
            state,
            info,
        });
        Ok(())
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let size = open.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        egui_winit::update_viewport_info(&mut open.info, &self.ctx, &open.window, false);
        let mut input = open.state.take_egui_input(&open.window);
        input.viewports.insert(ViewportId::ROOT, open.info.clone());
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| app.show(ui));
        open.state
            .handle_platform_output(&open.window, output.platform_output);

        let mut close = false;
        if let Some(viewport) = output.viewport_output.get(&ViewportId::ROOT) {
            close = viewport
                .commands
                .iter()
                .any(|c| matches!(c, ViewportCommand::Close));
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut open.info,
                viewport.commands.iter().cloned(),
                &open.window,
                &mut actions,
            );
            // These come back as what the window would have sent had the keys been pressed.
            for action in actions {
                let event = match action {
                    egui_winit::ActionRequested::Paste => {
                        open.state.clipboard_text().map(egui::Event::Paste)
                    }
                    egui_winit::ActionRequested::Copy => Some(egui::Event::Copy),
                    egui_winit::ActionRequested::Cut => Some(egui::Event::Cut),
                    egui_winit::ActionRequested::Screenshot(_)
                    | egui_winit::ActionRequested::PaintWhileHidden => None,
                };
                if let Some(event) = event {
                    open.state.egui_input_mut().events.push(event);
                    self.next_frame = Some(Instant::now());
                }
            }
            if viewport.repaint_delay != Duration::MAX {
                let at = Instant::now() + viewport.repaint_delay;
                self.next_frame = Some(self.next_frame.map_or(at, |t| t.min(at)));
            }
        }

        let scale = output.pixels_per_point;
        let primitives = self.ctx.tessellate(output.shapes, scale);
        let (w, h) = (width.get() as usize, height.get() as usize);
        if self.painter.width != w || self.painter.height != h {
            self.painter.resize(w, h);
        }
        self.painter.update_textures(&output.textures_delta);
        self.painter.paint(egui::Color32::BLACK, &primitives, scale);
        self.painter.free_textures(&output.textures_delta);
        output.textures_delta.clear();

        if self.shown_size != (w, h) {
            if let Err(e) = open.surface.resize(width, height) {
                self.error = Some(e.to_string());
                event_loop.exit();
                return;
            }
            self.shown_size = (w, h);
            self.shown.clear();
            self.shown.resize(w * h, u32::MAX);
        }
        // Rows that changed, top to bottom. An unchanged frame costs nothing further.
        let changed = |row: &usize| {
            self.shown[row * w..(row + 1) * w] != self.painter.pixels[row * w..(row + 1) * w]
        };
        let first = (0..h).find(changed);
        let last = (0..h).rev().find(changed);
        if let (Some(first), Some(last)) = (first, last) {
            match open.surface.buffer_mut() {
                Ok(mut buffer) => {
                    // The buffer may hold an older frame, so all of it is written.
                    buffer.copy_from_slice(&self.painter.pixels);
                    let damage = softbuffer::Rect {
                        x: 0,
                        y: first as u32,
                        width,
                        height: NonZeroU32::new((last - first + 1) as u32).unwrap_or(height),
                    };
                    if let Err(e) = buffer.present_with_damage(&[damage]) {
                        self.error = Some(e.to_string());
                        event_loop.exit();
                        return;
                    }
                    self.shown[first * w..(last + 1) * w]
                        .copy_from_slice(&self.painter.pixels[first * w..(last + 1) * w]);
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                    event_loop.exit();
                    return;
                }
            }
        }

        self.frames += 1;
        if let Some(smoke) = &self.smoke {
            self.ctx.request_repaint();
            if self.frames == smoke.frames {
                let _ = std::fs::write(&smoke.file, "window opened with the processor\n");
                close = true;
            }
        }
        if close {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<Duration> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.open.is_none() {
            if let Err(e) = self.open_window(event_loop) {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, delay: Duration) {
        let at = Instant::now() + delay;
        self.next_frame = Some(self.next_frame.map_or(at, |t| t.min(at)));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match event {
            WindowEvent::RedrawRequested => {
                self.next_frame = None;
                self.frame(event_loop);
            }
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                self.remember_place();
                event_loop.exit();
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_)
                if !open.window.is_maximized() && open.window.fullscreen().is_none() =>
            {
                self.last_normal = Some(WindowGeometry::from_window(
                    self.ctx.zoom_factor(),
                    &open.window,
                ));
                if let WindowEvent::Resized(_) = event {
                    open.window.request_redraw();
                }
                let _ = open.state.on_window_event(&open.window, &event);
            }
            other => {
                if open.state.on_window_event(&open.window, &other).repaint {
                    open.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.next_frame {
            Some(at) if at <= Instant::now() => {
                if let Some(open) = &self.open {
                    open.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

pub fn run(
    mut app: App,
    builder: egui::ViewportBuilder,
    smoke: Option<Smoke>,
    place_file: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let event_loop = EventLoop::<Duration>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let ctx = egui::Context::default();
    // One window: the session manager shows inside this one.
    ctx.set_embed_viewports(true);
    let proxy = event_loop.create_proxy();
    let wake = proxy.clone();
    ctx.set_request_repaint_callback(move |info| {
        let _ = wake.send_event(info.delay);
    });
    app.set_software_rendering(&ctx);
    app.set_renderer_info("Drawn by the processor, as this computer has no GPU".to_string());
    let mut runner = Runner {
        app,
        ctx,
        builder,
        proxy,
        open: None,
        painter: Painter::default(),
        shown: Vec::new(),
        shown_size: (0, 0),
        next_frame: None,
        smoke,
        frames: 0,
        error: None,
        place_file,
        placed: None,
        last_normal: None,
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    runner.app.save_before_exit();
    let _ = &runner.proxy;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
