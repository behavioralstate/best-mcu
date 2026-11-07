//! The door: what a request is answered with, apart from the board. The firmware hands each request to
//! [`Door::answer`] and sends back what it returns; nothing here knows the HTTP server, the network or the pins.

use serde_json::{json, Value};

/// The BEST version this door declares.
pub const BEST_VERSION: &str = "0.9.20";

/// A request as the door reads it.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// The `If-None-Match` header, when the client sent one.
    pub if_none_match: Option<&'a str>,
}

/// An answer: its status, its headers beyond the content type, and its JSON body, if any.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<String>,
}

/// The door of one board, served at `endpoint` and named by `name`, the DNS name it is known by.
pub struct Door {
    manifest: String,
    etag: String,
}

impl Door {
    pub fn new(name: &str, endpoint: &str) -> Self {
        let manifest = manifest(name, endpoint).to_string();
        let etag = format!("\"{:016x}\"", fnv1a(manifest.as_bytes()));
        Self { manifest, etag }
    }

    pub fn answer(&self, req: &Request) -> Response {
        match (req.method, req.path) {
            // HEAD is refused: the server would give its answer a length of 0, where GET's is the manifest's.
            ("GET", "/.well-known/best") => self.manifest(req),
            (_, "/.well-known/best") => Response {
                headers: vec![("Allow", "GET".into())],
                ..error(405, "METHOD_NOT_ALLOWED", "the manifest is read with GET")
            },
            _ => error(
                404,
                "NOT_FOUND",
                "nothing is served here: GET /.well-known/best lists what is",
            ),
        }
    }

    /// The manifest, with an ETag and a cache life so a client can tell it changed without reading it again.
    fn manifest(&self, req: &Request) -> Response {
        let headers = vec![
            ("ETag", self.etag.clone()),
            ("Cache-Control", "public, max-age=60".into()),
        ];
        // Compared weakly, as HTTP compares If-None-Match: `W/"x"` holds `"x"`, and `*` holds any.
        let held = |t: &str| {
            let t = t.trim();
            t == "*" || t.strip_prefix("W/").unwrap_or(t) == self.etag
        };
        match req.if_none_match.is_some_and(|tags| tags.split(',').any(held)) {
            true => Response {
                status: 304,
                headers,
                body: None,
            },
            false => Response {
                status: 200,
                headers,
                body: Some(self.manifest.clone()),
            },
        }
    }
}

/// The service key: `name` reversed, as the spec asks a service key to sit under the name it resolves from.
fn service_key(name: &str) -> String {
    name.rsplit('.').collect::<Vec<_>>().join(".")
}

fn manifest(name: &str, endpoint: &str) -> Value {
    let service = service_key(name);
    json!({
        "best": {
            "version": BEST_VERSION,
            "services": {
                service.clone(): {
                    "version": "0.0.1",
                    "description": "A robot run by a microcontroller: its skills, its readings and what it did.",
                    "http": { "endpoint": endpoint }
                }
            },
            "capabilities": [{
                "name": "io.best.agents.queries",
                "version": "1.0.0",
                "description": "What the robot reads and reports.",
                "spec": "https://behavioralstate.io/spec#queries--iobestagentsqueries",
                "schema": "https://behavioralstate.io/protocol/v1/schemas/query-catalogue.json",
                "service": service,
                "status": "planned"
            }]
        }
    })
}

fn error(status: u16, code: &str, message: &str) -> Response {
    let body = json!({ "error": { "code": code, "message": message } }).to_string();
    Response {
        status,
        headers: vec![],
        body: Some(body),
    }
}

/// FNV-1a, 64 bits: enough to tell one manifest from another, with nothing to link.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf29ce484222325, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(path: &'a str, if_none_match: Option<&'a str>) -> Request<'a> {
        Request {
            method: "GET",
            path,
            if_none_match,
        }
    }

    fn door() -> Door {
        Door::new("robot.example.com", "https://robot.example.com")
    }

    #[test]
    fn the_manifest_is_public_json_under_the_reversed_name_with_an_etag_and_a_cache_life() {
        let r = door().answer(&get("/.well-known/best", None));
        assert_eq!(r.status, 200);
        let m: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(m["best"]["version"], BEST_VERSION);
        assert_eq!(
            m["best"]["services"]["com.example.robot"]["http"]["endpoint"],
            "https://robot.example.com"
        );
        assert_eq!(m["best"]["capabilities"][0]["service"], "com.example.robot");
        assert!(r.headers.iter().any(|(h, v)| *h == "ETag" && v.starts_with('"')));
        assert!(r
            .headers
            .iter()
            .any(|(h, v)| *h == "Cache-Control" && !v.contains("no-store")));
    }

    #[test]
    fn a_client_holding_the_manifest_is_told_it_has_not_changed() {
        let d = door();
        let etag = d.answer(&get("/.well-known/best", None)).headers[0].1.clone();
        let r = d.answer(&get("/.well-known/best", Some(&format!("\"other\", {etag}"))));
        assert_eq!((r.status, r.body), (304, None));
    }

    #[test]
    fn a_weak_validator_or_any_matches_as_http_compares_if_none_match() {
        let d = door();
        let etag = d.answer(&get("/.well-known/best", None)).headers[0].1.clone();
        for held in [format!("W/{etag}"), "*".to_string()] {
            assert_eq!(d.answer(&get("/.well-known/best", Some(&held))).status, 304, "{held}");
        }
        assert_eq!(d.answer(&get("/.well-known/best", Some("W/\"other\""))).status, 200);
    }

    #[test]
    fn what_is_not_served_is_refused_in_the_best_error_format_with_what_to_do() {
        let r = door().answer(&get("/nothing", None));
        assert_eq!(r.status, 404);
        let e: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(e["error"]["code"], "NOT_FOUND");
        // HEAD too: ESP-IDF would give its answer a length of 0, where GET's is the manifest's.
        for method in ["POST", "HEAD"] {
            let r = door().answer(&Request {
                method,
                path: "/.well-known/best",
                if_none_match: None,
            });
            assert_eq!(r.status, 405, "{method}");
            assert_eq!(r.headers, vec![("Allow", "GET".to_string())]);
            let e: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
            assert_eq!(e["error"]["code"], "METHOD_NOT_ALLOWED");
        }
    }
}
