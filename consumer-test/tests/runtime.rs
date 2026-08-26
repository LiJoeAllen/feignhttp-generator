//! Wire-level integration tests: the generated clients are exercised
//! against a minimal local HTTP/1.1 stub server, verifying URL
//! construction, path/query/header/form/multipart serialization and
//! response decoding exactly as feignhttp performs them at runtime.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use consumer_test::models::{
    DeviceGroupCreate, DeviceLoginForm, DeviceStatus, DeviceStatusState, IndexPatchDeviceGroupsBody,
    StatsDailyQuery,
};
use consumer_test::{device::Device, index::Index, stats::Stats, ApiContext};
use feignhttp::FeignClientBuilder;

/// What the stub server captured from one request.
#[derive(Default, Clone)]
struct Captured {
    method: String,
    path: String,
    query: String,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

type Handler = Arc<dyn Fn(&Captured) -> (u16, String, Vec<u8>) + Send + Sync>;

/// Start a stub server on an ephemeral port; returns its base URL.
fn spawn_server(handler: Handler) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local_addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let handler = handler.clone();
            std::thread::spawn(move || handle_conn(stream, handler));
        }
    });
    format!("http://{addr}")
}

fn handle_conn(stream: TcpStream, handler: Handler) {
    let Ok(write_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut write_half = write_half;

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name.trim().to_string(), value));
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).expect("read body");
    }

    let content_type = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    let captured = Captured {
        method,
        path,
        query,
        content_type,
        headers,
        body,
    };
    let (status, content_type, body) = handler(&captured);

    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let head_only = captured.method == "HEAD";
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = write_half.write_all(response.as_bytes());
    if !head_only {
        let _ = write_half.write_all(&body);
    }
    let _ = write_half.flush();
}

fn client_ctx(base_url: &str) -> ApiContext {
    ApiContext::new(base_url.to_string(), String::new())
}

fn json_response(status: u16, value: serde_json::Value) -> (u16, String, Vec<u8>) {
    (
        status,
        "application/json".to_string(),
        serde_json::to_vec(&value).expect("serialize json"),
    )
}

fn empty_response(status: u16) -> (u16, String, Vec<u8>) {
    (status, String::new(), Vec::new())
}

#[tokio::test]
async fn health_probe_roundtrip() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/health");
        empty_response(200)
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    client.health().await.expect("health() should succeed");
}

#[tokio::test]
async fn get_device_groups_sends_query_and_decodes_json() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/device-groups");
        // Query order is unspecified; both params must be present.
        assert!(req.query.contains("page=2"), "query={}", req.query);
        assert!(req.query.contains("pageSize=20"), "query={}", req.query);
        json_response(
            200,
            serde_json::json!({
                "items": [
                    {"groupId": "g1", "groupName": "Front Row", "capacity": 12},
                    {"groupId": "g2", "groupName": "Back Row"}
                ],
                "total": 2
            }),
        )
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    let resp = client
        .get_device_groups(2, 20)
        .await
        .expect("get_device_groups");
    assert_eq!(resp.total, 2);
    assert_eq!(resp.items.len(), 2);
    assert_eq!(resp.items[0].group_id, "g1");
    assert_eq!(resp.items[0].group_name, "Front Row");
    assert_eq!(resp.items[0].capacity, Some(12));
    assert_eq!(resp.items[1].capacity, None);
}

#[tokio::test]
async fn post_device_groups_serializes_body_with_renames() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/device-groups");
        assert!(req.content_type.starts_with("application/json"));
        let v: serde_json::Value =
            serde_json::from_slice(&req.body).expect("request body must be valid JSON");
        assert_eq!(v["groupName"], "Lobby");
        assert_eq!(v["capacity"], 8);
        json_response(
            200,
            serde_json::json!({"groupId": "g9", "groupName": "Lobby", "capacity": 8}),
        )
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    let created = DeviceGroupCreate {
        group_name: "Lobby".to_string(),
        capacity: Some(8),
    };
    let group = client.post_device_groups(created).await.expect("create");
    assert_eq!(group.group_id, "g9");
    assert_eq!(group.group_name, "Lobby");
    assert_eq!(group.capacity, Some(8));
}

#[tokio::test]
async fn put_status_substitutes_path_and_header() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "PUT");
        assert_eq!(
            req.path,
            "/device-groups/G7/devices/D3/status",
            "path params must be substituted"
        );
        assert_eq!(req.header("x-token"), Some("t0k3n"), "header param missing");
        let v: serde_json::Value =
            serde_json::from_slice(&req.body).expect("body json");
        assert_eq!(v["state"], "online");
        empty_response(200)
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    let status = DeviceStatus {
        state: DeviceStatusState::Online,
        temperature: Some(36.5),
    };
    client
        .put_device_groups("G7".to_string(), "D3".to_string(), "t0k3n".to_string(), status)
        .await
        .expect("put status");
}

#[tokio::test]
async fn patch_status_substitutes_path() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "PATCH");
        assert_eq!(req.path, "/device-groups/GA/devices/DB/status");
        let v: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(v["volume"], 3);
        empty_response(200)
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    client
        .patch_device_groups(
            "GA".to_string(),
            "DB".to_string(),
            IndexPatchDeviceGroupsBody { volume: Some(3) },
        )
        .await
        .expect("patch status");
}

#[tokio::test]
async fn firmware_downloads_octet_stream_bytes() {
    let payload = vec![1u8, 2, 3, 255, 0];
    let payload_clone = payload.clone();
    let base = spawn_server(Arc::new(move |req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/device/firmware/fw-42/download");
        (
            200,
            "application/octet-stream".to_string(),
            payload_clone.clone(),
        )
    }));
    let client = Device::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Device");
    let bytes = client.firmware("fw-42".to_string()).await.expect("firmware");
    assert_eq!(bytes, payload);
}

#[tokio::test]
async fn login_sends_form_and_decodes_renamed_fields() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/device/login");
        assert!(
            req.content_type.starts_with("application/x-www-form-urlencoded"),
            "ct={}",
            req.content_type
        );
        let form = String::from_utf8(req.body.clone()).expect("utf8 form");
        assert!(form.contains("username=admin"), "form={form}");
        assert!(form.contains("password=s3cret"), "form={form}");
        json_response(
            200,
            serde_json::json!({"accessToken": "tok-1", "expiresIn": 3600}),
        )
    }));
    let client = Device::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Device");
    let resp = DeviceLoginForm {
        username: "admin".to_string(),
        password: "s3cret".to_string(),
    };
    let login = client.login(resp).await.expect("login");
    assert_eq!(login.access_token, "tok-1");
    assert_eq!(login.expires_in, Some(3600));
}

#[tokio::test]
async fn avatar_upload_sends_multipart() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/device/dev-1/avatar");
        assert!(
            req.content_type.starts_with("multipart/form-data; boundary="),
            "ct={}",
            req.content_type
        );
        let raw = String::from_utf8_lossy(&req.body).into_owned();
        assert!(raw.contains("name=\"file\""), "missing file part:\n{raw}");
        assert!(raw.contains("name=\"kind\""), "missing kind part:\n{raw}");
        assert!(raw.contains("photo"), "missing kind value:\n{raw}");
        assert!(raw.contains("\u{0}\u{1}PNGDATA"), "binary payload lost:\n{raw}");
        (200, "text/plain".to_string(), b"avatar-ok".to_vec())
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    let png = b"\x00\x01PNGDATA".to_vec();
    let text = client
        .device("dev-1".to_string(), png, "photo".to_string())
        .await
        .expect("upload avatar");
    assert_eq!(text, "avatar-ok");
}

#[tokio::test]
async fn stats_daily_enum_query_and_decode() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/stats/daily/2026-08-01");
        assert!(
            req.query.contains("granularity=day"),
            "enum query not serialized via Display: {}",
            req.query
        );
        json_response(
            200,
            serde_json::json!({
                "date": "2026-08-01",
                "granularity": "day",
                "revenue": 125.5,
                "transactions": [{"count": 2, "slotNo": 7}]
            }),
        )
    }));
    let client = Stats::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Stats");
    let daily = client
        .daily("2026-08-01".to_string(), StatsDailyQuery::Day)
        .await
        .expect("stats daily");
    assert_eq!(daily.date, "2026-08-01");
    assert_eq!(daily.revenue, 125.5);
    let tx = daily.transactions.expect("transactions");
    assert_eq!(tx[0].count, 2);
    assert_eq!(tx[0].slot_no, 7);
}

#[tokio::test]
async fn head_and_options_inventory() {
    for (expected_method, call) in [("HEAD", 0usize), ("OPTIONS", 1usize)] {
        let base = spawn_server(Arc::new(move |req| {
            assert_eq!(req.method, expected_method);
            assert_eq!(req.path, "/inventory/M1/slots/4");
            empty_response(200)
        }));
        let client = Index::builder()
            .context(client_ctx(&base))
            .build()
            .expect("build Index");
        match call {
            0 => client.head_inventory("M1".to_string(), 4).await.expect("head"),
            _ => client.options_inventory("M1".to_string(), 4).await.expect("options"),
        }
    }
}

#[tokio::test]
async fn error_response_parses_into_api_error_payload() {
    let base = spawn_server(Arc::new(|_req| {
        json_response(
            404,
            serde_json::json!({"code": 404, "message": "no such group", "traceId": "tr-9"}),
        )
    }));
    let client = Index::builder()
        .context(client_ctx(&base))
        .build()
        .expect("build Index");
    let err = client
        .get_device_groups(1, 10)
        .await
        .expect_err("expected status error");
    let kind = err.error_kind();
    let feignhttp::ErrorKind::Status(status, body) = kind else {
        panic!("expected ErrorKind::Status, got {kind:?}");
    };
    assert_eq!(status.as_u16(), 404);
    let payload: consumer_test::models::ApiError =
        serde_json::from_str(&body).expect("parse ApiError");
    assert_eq!(payload.code, 404);
    assert_eq!(payload.message, "no such group");
    assert_eq!(payload.trace_id.as_deref(), Some("tr-9"));
}
