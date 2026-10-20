//! End-to-end send-path test: builds a real request with the same code the UI
//! uses, executes it over the network with ehttp against a live local server,
//! and verifies response summarization + history persistence.
//!
//! Reuses an already-running server on 127.0.0.1:18080 when one answers;
//! otherwise starts the locally built `restrust-server` with detached stdio
//! (so it can never hang the test harness pipes) and stops it on drop.

use restrust_core::model::{AuthKind, BodyKind, HttpMethod, KeyValue, RequestBody, RequestDef};
use restrust_core::workspace_doc::WorkspaceDoc;

const SERVER: &str = "http://127.0.0.1:18080";

/// Kills the child when dropped, even on assertion panic.
struct ServerGuard(Option<std::process::Child>);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_server_if_needed() -> ServerGuard {
    if std::net::TcpStream::connect(("127.0.0.1", 18080)).is_ok() {
        return ServerGuard(None);
    }
    let exe = std::env::current_dir()
        .ok()
        .map(|d| d.join("../target/debug/restrust-server"))
        .filter(|p| p.exists());
    let child = match exe {
        Some(path) => std::process::Command::new(path)
            .env("RESTRUST_HTTP_PORT", "18080")
            .env(
                "RESTRUST_DATA_DIR",
                std::env::temp_dir().join("restrust-send-e2e"),
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok(),
        None => None,
    };
    // Wait for the listener to come up.
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", 18080)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    ServerGuard(child)
}

fn fetch_blocking(req: ehttp::Request) -> Result<ehttp::Response, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    ehttp::fetch(req, move |result| {
        tx.send(result).unwrap();
    });
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
}

#[test]
fn send_healthz_and_record_history() {
    let _server = spawn_server_if_needed();

    // Same construction the request editor uses.
    let def = RequestDef {
        id: "test-request-1".into(),
        name: "Health check".into(),
        description: None,
        method: HttpMethod::Get,
        url: format!("{SERVER}/api/v1/healthz"),
        query: vec![KeyValue {
            key: "x-trace".into(),
            value: "e2e".into(),
            enabled: true,
            description: None,
        }],
        headers: vec![KeyValue {
            key: "accept".into(),
            value: "application/json".into(),
            enabled: true,
            description: None,
        }],
        body: RequestBody {
            kind: BodyKind::None,
            raw: None,
            rows: Vec::new(),
        },
        auth: None,
    };

    let req =
        restrust_client::send::build_request(&def, &AuthKind::None, &[]).expect("request builds");
    assert_eq!(req.method, ehttp::Method::GET);
    assert!(req.url.contains("/api/v1/healthz?x-trace=e2e"));
    let accept = req.headers.get("accept").unwrap_or_default();
    assert_eq!(accept, "application/json");

    let resp = fetch_blocking(req).expect("server must be reachable for the e2e send test");
    assert_eq!(
        resp.status,
        200,
        "body: {}",
        resp.text().unwrap_or_default()
    );
    let text = resp.text().unwrap_or_default();
    assert!(text.contains("\"status\""), "healthz body: {text}");

    // Response summary + history recording exactly as the UI does on completion.
    let summary = restrust_client::send::summary_from_response(&resp, 12);
    assert_eq!(summary.status, 200);
    assert!(summary.size > 0);

    let mut doc = WorkspaceDoc::new();
    doc.add_history(def, Some(summary));
    let json = doc.to_json().unwrap();
    assert!(
        json.contains("Health check"),
        "history must persist: {json}"
    );
    let reloaded = WorkspaceDoc::from_json(&json).unwrap();
    assert_eq!(reloaded.history.len(), 1);
    assert_eq!(reloaded.history[0].response.as_ref().unwrap().status, 200);
}
