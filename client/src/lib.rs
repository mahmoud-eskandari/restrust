//! restrust-client: the cross-platform (desktop + browser) egui app.
//!
//! Native entry: [`run_native`] (called by `main.rs`). Web entry: the
//! `#[wasm_bindgen]` [`start`] function bootstrapped by Trunk via `index.html`
//! (eframe_template pattern), canvas id `the_canvas_id`.

pub mod api;
pub mod app;
pub mod platform;
pub mod send;
pub mod state;
pub mod sync;
pub mod theme;
pub mod time;
pub mod ui;

#[cfg(target_arch = "wasm32")]
use eframe::wasm_bindgen::JsCast;

/// Runs the native desktop app: logging, window options (persisted size —
/// `persist_window` is eframe's default), dark theme default.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("restrust")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "restrust",
        options,
        Box::new(|cc| Ok(Box::new(app::RestrustApp::new(cc)))),
    )
}

/// Web entry (eframe_template pattern): looks up the canvas by id and starts
/// the app on it.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start(app_name: String, canvas_id: String) -> Result<(), eframe::wasm_bindgen::JsValue> {
    // Redirect `log` messages to console.log and friends:
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    if let Some(document) = eframe::web_sys::window().and_then(|w| w.document()) {
        if !app_name.is_empty() {
            document.set_title(&app_name);
        }
    }

    let web_options = eframe::WebOptions::default();

    let canvas = eframe::web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&canvas_id))
        .and_then(|element| {
            element
                .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
                .ok()
        })
        .ok_or_else(|| {
            eframe::wasm_bindgen::JsValue::from_str(&format!("could not find canvas #{canvas_id}"))
        })?;

    // Remove the boot spinner overlay (index.html) once the app is starting.
    if let Some(document) = eframe::web_sys::window().and_then(|w| w.document()) {
        if let Some(spinner) = document.get_element_by_id("loading_text") {
            spinner.remove();
        }
    }

    wasm_bindgen_futures::spawn_local(async move {
        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Ok(Box::new(app::RestrustApp::new(cc)))),
            )
            .await
            .expect("failed to start eframe");
    });

    Ok(())
}
