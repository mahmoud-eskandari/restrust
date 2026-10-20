//! Entry points: native desktop window, and the wasm bootstrap used by Trunk.

// Hide the console window on Windows in release:
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    restrust_client::run_native()
}

// When compiling to web using trunk:
#[cfg(target_arch = "wasm32")]
fn main() {
    // Canvas id is hardcoded in index.html (eframe_template pattern).
    let _ = restrust_client::start("restrust".to_string(), "the_canvas_id".to_string());
}
