//! End-to-end send-path test: builds a real request with the same code the UI
//! uses, executes it over the network with ehttp against a live local server,
//! and verifies response summarization + history persistence.
//!
//! Owns a loopback HTTP fixture on an ephemeral port. No prebuilt sibling
//! executable, fixed port, or existing user server is required.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use restrust_core::model::{AuthKind, BodyKind, HttpMethod, KeyValue, RequestBody, RequestDef};
use restrust_core::workspace_doc::WorkspaceDoc;

fn start_target() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test target");
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "test target received no request");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept request: {err}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut headers = Vec::new();
        let mut byte = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            assert!(headers.len() < 8192, "request headers exceed fixture limit");
            stream.read_exact(&mut byte).expect("read request headers");
            headers.push(byte[0]);
        }
        let request = String::from_utf8(headers).unwrap();
        assert!(request.starts_with("GET /api/v1/healthz?x-trace=e2e HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("\r\naccept: application/json\r\n"));
        let body = r#"{"status":"ok"}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    (format!("http://{address}"), worker)
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
    let (server, worker) = start_target();

    // Same construction the request editor uses.
    let def = RequestDef {
        id: "test-request-1".into(),
        name: "Health check".into(),
        description: None,
        method: HttpMethod::Get,
        url: format!("{server}/api/v1/healthz"),
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

    let result = fetch_blocking(req);
    worker
        .join()
        .expect("test target must receive the expected request");
    let resp = result.expect("test target must be reachable");
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
