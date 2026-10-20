//! Platform services (CONTRACT §5.1): storage, clipboard, HTTP and browser,
//! each behind a trait with native and wasm implementations, isolated from the
//! UI. Nothing here blocks the UI thread: saves go through a worker-thread
//! channel on native, HTTP uses `ehttp` callbacks that request a repaint.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use restrust_core::workspace_doc::WorkspaceDoc;

/// True when compiled to WebAssembly.
pub const IS_WASM: bool = cfg!(target_arch = "wasm32");

// --- storage -----------------------------------------------------------------

/// Persists the [`WorkspaceDoc`].
///
/// Contract §5.1 plus one additive helper, [`StorageService::read_raw`], so the
/// app can distinguish "corrupt file, backed up, starting fresh" (banner) from
/// "no data yet" after a parse failure.
pub trait StorageService: Send + Sync {
    /// Serializes and persists the document (never blocks the UI thread).
    fn save(&self, doc: &WorkspaceDoc);
    /// Loads the document, backing up a corrupt store first.
    fn load(&self) -> Option<WorkspaceDoc>;
    /// Moves a raw unparsable store aside so the next save starts fresh.
    fn backup_corrupt(&self, raw: String);
    /// Reads the raw store contents; `None` when nothing is stored.
    fn read_raw(&self) -> Option<String>;
    /// Human-readable storage location for Settings / the status bar.
    fn describe(&self) -> String;
}

/// The native store: a JSON file at `{data_dir}/restrust/workspace.json`,
/// written atomically (tmp + rename) by a worker thread fed through a channel.
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeStorage {
    path: std::path::PathBuf,
    sender: Mutex<Option<std::sync::mpsc::Sender<String>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for NativeStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeStorage {
    /// Creates the store and spawns the save worker thread.
    pub fn new() -> Self {
        let path = workspace_path();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let worker_path = path.clone();
        let worker = std::thread::Builder::new()
            .name("restrust-storage".into())
            .spawn(move || {
                // Drains every pending save, then exits when the sender drops.
                for json in rx {
                    if let Err(err) = write_atomic(&worker_path, &json) {
                        log::error!("workspace save failed: {err}");
                    }
                }
            })
            .expect("spawn storage worker");
        NativeStorage {
            path,
            sender: Mutex::new(Some(tx)),
            worker: Mutex::new(Some(worker)),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for NativeStorage {
    fn drop(&mut self) {
        // Close the channel so the worker drains remaining saves and exits,
        // then join it: guarantees the last workspace state hits the disk.
        self.sender.lock().unwrap().take();
        if let Some(handle) = self.worker.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl StorageService for NativeStorage {
    fn save(&self, doc: &WorkspaceDoc) {
        match doc.to_json() {
            Ok(json) => {
                if let Some(tx) = self.sender.lock().unwrap().as_ref() {
                    if tx.send(json).is_err() {
                        log::error!("storage worker is gone; save dropped");
                    }
                }
            }
            Err(err) => log::error!("workspace serialization failed: {err}"),
        }
    }

    fn load(&self) -> Option<WorkspaceDoc> {
        let raw = self.read_raw()?;
        match WorkspaceDoc::from_json(&raw) {
            Ok(doc) => Some(doc),
            Err(err) => {
                log::warn!("workspace parse failed ({err}); backing up and starting fresh");
                self.backup_corrupt(raw);
                None
            }
        }
    }

    fn backup_corrupt(&self, raw: String) {
        let ts = chrono::Utc::now().timestamp();
        let corrupt = self
            .path
            .with_file_name(format!("workspace.corrupt-{ts}.json"));
        if let Err(err) = write_atomic(&corrupt, &raw) {
            log::error!("could not back up corrupt workspace: {err}");
        }
        let _ = std::fs::remove_file(&self.path);
    }

    fn read_raw(&self) -> Option<String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .filter(|s| !s.trim().is_empty())
    }

    fn describe(&self) -> String {
        self.path.display().to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn data_root() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("restrust")
}

#[cfg(not(target_arch = "wasm32"))]
fn workspace_path() -> std::path::PathBuf {
    data_root().join("workspace.json")
}

/// Atomic write: tmp file + rename (native only).
#[cfg(not(target_arch = "wasm32"))]
fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Default export/response path on native: `{data_dir}/restrust/exports/{slug}.json`.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_export_path(slug: &str) -> String {
    data_root()
        .join("exports")
        .join(format!("{}.json", sanitize_slug(slug)))
        .display()
        .to_string()
}

/// Keeps `[A-Za-z0-9._-]` and replaces everything else with `-`.
pub fn sanitize_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The wasm store: `localStorage` under `restrust.workspace.v1` (small enough
/// to save synchronously).
#[cfg(target_arch = "wasm32")]
pub struct WasmStorage {
    key: String,
}

#[cfg(target_arch = "wasm32")]
impl WasmStorage {
    pub fn new() -> Self {
        WasmStorage {
            key: "restrust.workspace.v1".to_string(),
        }
    }

    fn local_storage(&self) -> Option<eframe::web_sys::Storage> {
        eframe::web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for WasmStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
impl StorageService for WasmStorage {
    fn save(&self, doc: &WorkspaceDoc) {
        match doc.to_json() {
            Ok(json) => {
                if let Some(storage) = self.local_storage() {
                    if let Err(err) = storage.set_item(&self.key, &json) {
                        log::error!("localStorage save failed: {err:?}");
                    }
                }
            }
            Err(err) => log::error!("workspace serialization failed: {err}"),
        }
    }

    fn load(&self) -> Option<WorkspaceDoc> {
        let raw = self.read_raw()?;
        match WorkspaceDoc::from_json(&raw) {
            Ok(doc) => Some(doc),
            Err(err) => {
                log::warn!("workspace parse failed ({err}); backing up and starting fresh");
                self.backup_corrupt(raw);
                None
            }
        }
    }

    fn backup_corrupt(&self, raw: String) {
        let ts = chrono::Utc::now().timestamp();
        if let Some(storage) = self.local_storage() {
            let _ = storage.set_item(&format!("{}.corrupt-{ts}", self.key), &raw);
            let _ = storage.remove_item(&self.key);
        }
    }

    fn read_raw(&self) -> Option<String> {
        self.local_storage()
            .and_then(|s| s.get_item(&self.key).ok().flatten())
            .filter(|s| !s.trim().is_empty())
    }

    fn describe(&self) -> String {
        "Browser local storage".to_string()
    }
}

// --- clipboard ----------------------------------------------------------------

/// Clipboard access through the egui context — identical on both platforms
/// (CONTRACT §5.1).
pub trait ClipboardService: Send + Sync {
    fn set_text(&self, text: &str);
}

/// `egui::Context`-backed clipboard.
pub struct EguiClipboard {
    ctx: egui::Context,
}

impl EguiClipboard {
    pub fn new(ctx: egui::Context) -> Self {
        EguiClipboard { ctx }
    }
}

impl ClipboardService for EguiClipboard {
    fn set_text(&self, text: &str) {
        self.ctx.copy_text(text.to_string());
    }
}

// --- http ---------------------------------------------------------------------

/// A completed HTTP exchange.
pub type HttpResult = Result<ehttp::Response, String>;

/// Asynchronous HTTP (CONTRACT §5.1). Implementations must never block.
pub trait HttpService: Send + Sync {
    fn fetch(&self, req: ehttp::Request, on_done: Box<dyn FnOnce(HttpResult) + Send>);
}

/// `ehttp`-backed service. Tracked requests carry an id and land in an inbox
/// that the update loop drains; every completion requests a repaint so the UI
/// wakes up immediately (eframe http-example pattern).
pub struct EhttpService {
    ctx: egui::Context,
    inbox: Arc<Mutex<Vec<(u64, HttpResult)>>>,
    next_id: AtomicU64,
}

impl EhttpService {
    pub fn new(ctx: egui::Context) -> Self {
        EhttpService {
            ctx,
            inbox: Arc::new(Mutex::new(Vec::new())),
            next_id: AtomicU64::new(1),
        }
    }

    /// Starts a request; the result is delivered into the inbox keyed by the
    /// returned id.
    pub fn fetch_tracked(&self, req: ehttp::Request) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        ehttp::fetch(req, move |result: ehttp::Result<ehttp::Response>| {
            inbox
                .lock()
                .unwrap()
                .push((id, result.map_err(|err| err.to_string())));
            ctx.request_repaint(); // wake the UI thread
        });
        id
    }

    /// Takes all completed results (called once per frame).
    pub fn drain(&self) -> Vec<(u64, HttpResult)> {
        std::mem::take(&mut *self.inbox.lock().unwrap())
    }
}

impl HttpService for EhttpService {
    fn fetch(&self, req: ehttp::Request, on_done: Box<dyn FnOnce(HttpResult) + Send>) {
        let ctx = self.ctx.clone();
        ehttp::fetch(req, move |result: ehttp::Result<ehttp::Response>| {
            on_done(result.map_err(|err| err.to_string()));
            ctx.request_repaint();
        });
    }
}

// --- browser -------------------------------------------------------------------

/// Opens URLs in the platform browser.
pub trait BrowserService: Send + Sync {
    fn open_url(&self, url: &str);
}

#[cfg(not(target_arch = "wasm32"))]
pub struct NativeBrowser;

#[cfg(not(target_arch = "wasm32"))]
impl BrowserService for NativeBrowser {
    fn open_url(&self, url: &str) {
        if let Err(err) = open::that_detached(url) {
            log::error!("could not open {url}: {err}");
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub struct WasmBrowser;

#[cfg(target_arch = "wasm32")]
impl BrowserService for WasmBrowser {
    fn open_url(&self, url: &str) {
        if let Some(window) = eframe::web_sys::window() {
            let _ = window.open_with_url_and_target(url, "_blank");
        }
    }
}

/// Navigates the current page (used by the wasm OIDC same-origin flow; native
/// falls back to opening the system browser).
pub fn navigate_current_page(url: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = eframe::web_sys::window() {
            let _ = window.location().assign(url);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Err(err) = open::that_detached(url) {
            log::error!("could not open {url}: {err}");
        }
    }
}

/// Reads and clears a `#token=...` hash left by the server's OIDC redirect
/// (wasm only, CONTRACT §4.3).
#[cfg(target_arch = "wasm32")]
pub fn take_location_hash_token() -> Option<String> {
    let window = eframe::web_sys::window()?;
    let hash = window.location().hash().ok()?;
    let (key, value) = hash.trim_start_matches('#').split_once('=')?;
    if key != "token" || value.is_empty() {
        return None;
    }
    let _ = window.location().set_hash("");
    Some(value.to_string())
}

// --- bundle ---------------------------------------------------------------------

/// All platform services in one shareable bundle.
pub struct Services {
    pub storage: Arc<dyn StorageService>,
    pub clipboard: Arc<dyn ClipboardService>,
    pub http: Arc<EhttpService>,
    pub browser: Arc<dyn BrowserService>,
}

impl Services {
    /// Builds the platform-appropriate service set for the given egui context.
    pub fn new(ctx: &egui::Context) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let storage: Arc<dyn StorageService> = Arc::new(NativeStorage::new());
        #[cfg(target_arch = "wasm32")]
        let storage: Arc<dyn StorageService> = Arc::new(WasmStorage::new());

        #[cfg(not(target_arch = "wasm32"))]
        let browser: Arc<dyn BrowserService> = Arc::new(NativeBrowser);
        #[cfg(target_arch = "wasm32")]
        let browser: Arc<dyn BrowserService> = Arc::new(WasmBrowser);

        Services {
            storage,
            clipboard: Arc::new(EguiClipboard::new(ctx.clone())),
            http: Arc::new(EhttpService::new(ctx.clone())),
            browser,
        }
    }

    /// Storage location description for Settings / the status bar.
    pub fn storage_description(&self) -> String {
        self.storage.describe()
    }
}
