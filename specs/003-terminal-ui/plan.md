# Implementation Plan: Terminal Window

egui via eframe's glow backend. One pump thread per pane feeds the emulator and the log.
Settings live in a right-hand inspector shown on request, and commands in a palette, so no
toolbar is needed. Tests use egui_kittest; screenshots need a Vulkan driver and are optional.
