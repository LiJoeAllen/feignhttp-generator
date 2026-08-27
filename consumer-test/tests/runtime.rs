//! Wire-level integration tests for code generated from the public
//! Swagger Petstore 3.0 spec (fetched at build time via URL).
//!
//! The generated clients are pointed at a local stub HTTP/1.1 server so
//! every assertion is deterministic and offline; the server impersonates
//! the exact routes declared in the spec.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use consumer_test::models::{
    ApiResponse, Category, Order, OrderStatus, Pet, PetFindByStatusQuery, PetStatus, Tag, User,
};
use consumer_test::{index::Index, pet::Pet as PetApi, store::Store, user::User as UserApi, ApiContext};
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
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
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
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
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

    let captured = Captured {
        method,
        path,
        query,
        content_type: headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default(),
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
    (status, "application/json".to_string(), serde_json::to_vec(&value).expect("serialize json"))
}

fn empty_response(status: u16) -> (u16, String, Vec<u8>) {
    (status, String::new(), Vec::new())
}

fn sample_pet_json(id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": "Rex",
        "category": {"id": 1, "name": "Dogs"},
        "photoUrls": ["https://example.com/rex.png"],
        "tags": [],
        "status": "available"
    })
}

#[tokio::test]
async fn get_pet_decodes_nested_models_and_enum() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/pet/42", "path param substitution");
        json_response(200, sample_pet_json(42))
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    let pet = client.get_pet(42).await.expect("get_pet");
    assert_eq!(pet.id, Some(42));
    assert_eq!(pet.name, "Rex");
    let category = pet.category.expect("category");
    assert_eq!(category.name.as_deref(), Some("Dogs"));
    assert_eq!(pet.photo_urls, vec!["https://example.com/rex.png".to_string()]);
    assert!(matches!(pet.status, Some(PetStatus::Available)));
}

#[tokio::test]
async fn post_pet_serializes_body_with_renames() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/pet");
        assert!(req.content_type.starts_with("application/json"));
        let v: serde_json::Value = serde_json::from_slice(&req.body).expect("request body must be valid JSON");
        assert_eq!(v["name"], "Rex");
        assert_eq!(v["photoUrls"], serde_json::json!(["a.png"]));
        json_response(200, sample_pet_json(7))
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    let pet = Pet {
        category: None,
        id: None,
        name: "Rex".to_string(),
        photo_urls: vec!["a.png".to_string()],
        status: Some(PetStatus::Pending),
        tags: None,
    };
    let created = client.post_pet(pet).await.expect("post_pet");
    assert_eq!(created.id, Some(7));
}

#[tokio::test]
async fn put_pet_roundtrip() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "PUT");
        assert_eq!(req.path, "/pet");
        json_response(
            200,
            serde_json::json!({
                "id": 42,
                "name": "Rex",
                "category": {"id": 1, "name": "Dogs"},
                "photoUrls": [],
                "tags": [],
                "status": "sold"
            }),
        )
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    let pet = Pet {
        category: Some(Category { id: Some(1), name: Some("Dogs".to_string()) }),
        id: Some(42),
        name: "Rex".to_string(),
        photo_urls: vec![],
        status: Some(PetStatus::Sold),
        tags: Some(vec![]),
    };
    let updated = client.put_pet(pet).await.expect("put_pet");
    assert!(matches!(updated.status, Some(PetStatus::Sold)));
}

#[tokio::test]
async fn delete_pet_sends_header_and_returns_unit() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "DELETE");
        assert_eq!(req.path, "/pet/42");
        assert_eq!(req.header("api_key"), Some("secret-key"), "header param missing");
        empty_response(200)
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    client.delete_pet(42, "secret-key".to_string()).await.expect("delete_pet");
}

#[tokio::test]
async fn find_by_status_sends_enum_query_and_decodes_vec() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/pet/findByStatus");
        assert!(req.query.contains("status=available"), "enum query not serialized correctly: {}", req.query);
        json_response(200, serde_json::json!([sample_pet_json(1), sample_pet_json(2)]))
    }));
    let client = PetApi::builder().context(client_ctx(&base)).build().expect("build Pet");
    let pets = client.find_by_status(PetFindByStatusQuery::Available).await.expect("find_by_status");
    assert_eq!(pets.len(), 2);
    assert_eq!(pets[1].id, Some(2));
}

#[tokio::test]
async fn find_by_tags_sends_repeated_array_query() {
    let base = spawn_server(Arc::new(|req| {
        assert!(
            req.query.contains("tags=alpha") && req.query.contains("tags=beta"),
            "array query must repeat the key: {}",
            req.query
        );
        json_response(200, serde_json::json!([]))
    }));
    let client = PetApi::builder().context(client_ctx(&base)).build().expect("build Pet");
    let pets = client.find_by_tags(vec!["alpha".to_string(), "beta".to_string()]).await.expect("find_by_tags");
    assert!(pets.is_empty());
}

#[tokio::test]
async fn upload_image_sends_octet_stream_and_decodes_apiresponse() {
    let payload = b"\x00\x01PNGDATA".to_vec();
    let expected = payload.clone();
    let base = spawn_server(Arc::new(move |req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/pet/42/uploadImage");
        assert!(req.query.contains("additionalMetadata=meta"), "query param missing: {}", req.query);
        assert!(req.content_type.starts_with("application/octet-stream"), "ct={}", req.content_type);
        assert_eq!(req.body, expected, "binary body lost");
        json_response(200, serde_json::json!({"code": 200, "type": "ok", "message": "uploaded"}))
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    let resp = client.post_pet_3(42, "meta".to_string(), payload).await.expect("upload image");
    assert_eq!(resp.code, Some(200));
    assert_eq!(resp.r#type.as_deref(), Some("ok"));
    assert_eq!(resp.message.as_deref(), Some("uploaded"));
}

#[tokio::test]
async fn user_login_sends_query_and_decodes_text() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/user/login");
        assert!(req.query.contains("username=u1"), "query={}", req.query);
        assert!(req.query.contains("password=p1"), "query={}", req.query);
        (200, "text/plain".to_string(), b"tok-9".to_vec())
    }));
    let client = UserApi::builder().context(client_ctx(&base)).build().expect("build User");
    let token = client.login("u1".to_string(), "p1".to_string()).await.expect("login");
    assert_eq!(token, "tok-9");
}

#[tokio::test]
async fn create_with_list_posts_json_array_body() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/user/createWithList");
        let v: serde_json::Value = serde_json::from_slice(&req.body).expect("array body");
        assert_eq!(v[0]["firstName"], "Ada");
        assert_eq!(v[0]["userStatus"], 1);
        json_response(
            200,
            serde_json::json!({"id": 9, "username": "ada", "firstName": "Ada", "lastName": "L", "email": null, "password": null, "phone": null, "userStatus": 1}),
        )
    }));
    let client = UserApi::builder().context(client_ctx(&base)).build().expect("build User");
    let users = vec![User {
        email: None,
        first_name: Some("Ada".to_string()),
        id: None,
        last_name: None,
        password: None,
        phone: None,
        user_status: Some(1),
        username: None,
    }];
    let created = client.create_with_list(users).await.expect("createWithList");
    assert_eq!(created.id, Some(9));
}

#[tokio::test]
async fn get_user_decodes_renamed_fields() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.path, "/user/u1");
        json_response(
            200,
            serde_json::json!({"id": 5, "username": "u1", "firstName": "F", "lastName": "L", "email": "e@x", "password": "p", "phone": "+1", "userStatus": 2}),
        )
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    let user = client.get_user("u1".to_string()).await.expect("get_user");
    assert_eq!(user.first_name.as_deref(), Some("F"));
    assert_eq!(user.user_status, Some(2));
}

#[tokio::test]
async fn store_order_enum_body_field_roundtrip() {
    let base = spawn_server(Arc::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/store/order");
        let v: serde_json::Value = serde_json::from_slice(&req.body).expect("order body");
        assert_eq!(v["petId"], 42);
        assert_eq!(v["shipDate"], "2026-08-26T00:00:00Z");
        assert_eq!(v["status"], "placed");
        json_response(
            200,
            serde_json::json!({"id": 3, "petId": 42, "quantity": 1, "shipDate": "2026-08-26T00:00:00Z", "status": "placed", "complete": false}),
        )
    }));
    let client = Store::builder().context(client_ctx(&base)).build().expect("build Store");
    let order = Order {
        complete: Some(false),
        id: Some(3),
        pet_id: Some(42),
        quantity: Some(1),
        ship_date: Some("2026-08-26T00:00:00Z".to_string()),
        status: Some(OrderStatus::Placed),
    };
    let placed = client.post_order(order).await.expect("place order");
    assert!(matches!(placed.status, Some(OrderStatus::Placed)));
    assert_eq!(placed.complete, Some(false));
}

#[tokio::test]
async fn error_response_parses_into_typed_payload() {
    let base = spawn_server(Arc::new(|_req| {
        json_response(404, serde_json::json!({"code": 404, "type": "error", "message": "Pet not found"}))
    }));
    let client = Index::builder().context(client_ctx(&base)).build().expect("build Index");
    let err = client.get_pet(999_999).await.expect_err("expected status error");
    let kind = err.error_kind();
    let feignhttp::ErrorKind::Status(status, body) = kind else {
        panic!("expected ErrorKind::Status");
    };
    assert_eq!(status.as_u16(), 404);
    let payload: ApiResponse = serde_json::from_str(&body).expect("parse ApiResponse");
    assert_eq!(payload.code, Some(404));
    assert_eq!(payload.message.as_deref(), Some("Pet not found"));
}

/// `Tag` is a free-form object in the spec; ensure the generated type at
/// least round-trips through `serde_json::Value`.
#[test]
fn tag_newtype_holds_arbitrary_json() {
    let tag = Tag(serde_json::json!({"id": 1, "name": "friendly"}));
    let encoded = serde_json::to_value(&tag).expect("serialize tag");
    assert_eq!(encoded["name"], "friendly");
}
