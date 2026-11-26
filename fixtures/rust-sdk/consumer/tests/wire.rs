//! The generated module tree, driven over a Unix socket by the hand-written transport, against a
//! relay played by a `UnixListener` that checks each request and answers with a canned reply.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread::JoinHandle;

use notes_consumer::api::models;
use notes_consumer::api::{Client, Error, ListNotesParams, MultipartFile};
use notes_consumer::unix::UnixSocketTransport;

/// What the relay received for one exchange.
struct Received {
    head: String,
    body: Vec<u8>,
}

/// A relay that serves one connection per reply, in order, and returns what it received.
fn relay(name: &str, replies: Vec<String>) -> (PathBuf, JoinHandle<Vec<Received>>) {
    let dir = std::env::temp_dir().join(format!("notes-relay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("relay.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let handle = std::thread::spawn(move || {
        let mut received = Vec::new();
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 4096];
            let (head, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                raw.extend_from_slice(&buffer[..read]);
                if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8(raw[..end].to_vec()).unwrap();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .map_or(0, |value| value.parse::<usize>().unwrap());
                    raw.drain(..end + 4);
                    break (head, length);
                }
            };
            while raw.len() < length {
                let read = stream.read(&mut buffer).unwrap();
                raw.extend_from_slice(&buffer[..read]);
            }
            stream.write_all(reply.as_bytes()).unwrap();
            received.push(Received { head, body: raw });
        }
        received
    });
    (path, handle)
}

fn json_reply(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.0 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn client(path: PathBuf) -> Client {
    Client::builder("http://notes.internal", UnixSocketTransport::new(path))
        .transport_auth()
        .build()
}

const NOTE: &str =
    r#"{"id":"n1","title":"Groceries","visibility":"private","tags":["home"],"score":null}"#;

#[test]
fn a_listing_sends_its_query_and_header_and_pages_through_the_cursor() {
    let (path, relay) = relay(
        "list",
        vec![
            json_reply(
                "200 OK",
                &format!(r#"{{"items":[{NOTE}],"next_cursor":"c2"}}"#),
            ),
            json_reply("200 OK", r#"{"items":[]}"#),
        ],
    );
    let mut params = ListNotesParams::new();
    params.limit = Some(1);
    params.tag = Some(vec!["home".into(), "work".into()]);
    params.x_trace = Some("t-1".into());
    let notes: Vec<models::Note> = client(path)
        .iter_list_notes(&params)
        .map(Result::unwrap)
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].visibility, models::Visibility::Private);
    let received = relay.join().unwrap();
    assert!(
        received[0]
            .head
            .starts_with("GET /v1/notes?limit=1&tag=home&tag=work HTTP/1.0\r\n"),
        "{}",
        received[0].head
    );
    assert!(
        received[0].head.contains("\r\nX-Trace: t-1"),
        "{}",
        received[0].head
    );
    assert!(
        received[0].head.contains("\r\nConnection: close"),
        "{}",
        received[0].head
    );
    assert!(
        !received[0].head.contains("X-API-Key"),
        "the relay adds the credential"
    );
    assert!(
        received[1]
            .head
            .starts_with("GET /v1/notes?cursor=c2&limit=1&tag=home&tag=work "),
        "{}",
        received[1].head
    );
}

#[test]
fn a_create_sends_json_and_a_rejection_is_an_api_error() {
    let (path, relay) = relay(
        "create",
        vec![
            json_reply("201 Created", NOTE),
            json_reply(
                "400 Bad Request",
                r#"{"message":"title is required","slug":"invalid"}"#,
            ),
        ],
    );
    let client = client(path);
    let body = models::NewNote::new("Groceries".into(), models::Visibility::Private);
    let note = client.create_note(&body).unwrap();
    assert_eq!(note.title, "Groceries");
    let Error::Api(error) = client.create_note(&body).unwrap_err() else {
        panic!("expected an API error");
    };
    assert_eq!(error.status, 400);
    assert_eq!(error.slug().as_deref(), Some("invalid"));
    let problem: models::Problem = error.decode().unwrap();
    assert_eq!(problem.message, "title is required");
    let received = relay.join().unwrap();
    assert!(received[0].head.starts_with("POST /v1/notes HTTP/1.0\r\n"));
    let sent: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({"title": "Groceries", "visibility": "private"})
    );
}

#[test]
fn a_file_goes_as_multipart_and_an_attachment_comes_back_as_bytes() {
    let (path, relay) = relay(
        "files",
        vec![
            "HTTP/1.0 204 No Content\r\n\r\n".to_string(),
            "HTTP/1.0 200 OK\r\nContent-Type: application/octet-stream\r\n\r\n\u{0}\u{1}"
                .to_string(),
        ],
    );
    let client = client(path);
    let mut form =
        models::UploadFileFormRequest::new(MultipartFile::new("list.txt", b"milk".to_vec()));
    form.caption = Some("weekly".into());
    client.upload_file("n 1", &form).unwrap();
    assert_eq!(client.attachment("n1").unwrap(), b"\x00\x01");
    let received = relay.join().unwrap();
    assert!(
        received[0]
            .head
            .starts_with("POST /v1/notes/n%201/files HTTP/1.0\r\n"),
        "{}",
        received[0].head
    );
    assert!(received[0]
        .head
        .contains("Content-Type: multipart/form-data; boundary=gnr8-"));
    let body = String::from_utf8(received[0].body.clone()).unwrap();
    assert!(body.contains("name=\"file\"; filename=\"list.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nmilk\r\n"), "{body}");
    assert!(
        body.contains("name=\"caption\"\r\n\r\nweekly\r\n"),
        "{body}"
    );
}
