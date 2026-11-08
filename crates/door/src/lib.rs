//! The door: what a request is answered with, apart from the board. The firmware hands each request to
//! [`Door::answer`] and the button's presses to [`Door::press`], and sends back what they return; nothing here knows
//! the HTTP server, the network or the pins. What the door needs of the board comes through [`Ports`].

mod guidance;
mod page;
mod signin;

use std::sync::{Mutex, MutexGuard};

use std::collections::BTreeMap;

use serde_json::{json, Value};
use signin::{OAuthError, Refusal, SignIn};

/// The BEST version this door declares.
pub const BEST_VERSION: &str = "0.9.20";

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const FORM: &str = "application/x-www-form-urlencoded";
const JSON: &str = "application/json";
const HTML: &str = "text/html; charset=utf-8";

/// A request as the door reads it.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// What follows the `?`, if anything.
    pub query: &'a str,
    pub authorization: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub body: &'a [u8],
}

/// An answer: its status, its content type, its other headers, and its body, if any.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<String>,
}

/// Milliseconds since the board started.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Random bytes, unguessable: the board's generator.
pub trait Entropy: Send + Sync {
    fn fill(&self, bytes: &mut [u8]);
}

/// Where the durable credentials' hashes outlive a restart.
pub trait Vault: Send + Sync {
    fn read(&self) -> Option<String>;
    fn write(&self, kept: &str) -> Result<(), String>;
}

/// What the robot reads about itself.
pub trait Readings: Send + Sync {
    fn read(&self) -> Status;
}

/// The robot's readings of itself, as `get-status` answers them.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub uptime_ms: u64,
    pub internal_free: u32,
    pub internal_least: u32,
    pub psram_free: u32,
    /// The Wi-Fi signal in dBm, when there is one to read.
    pub wifi_rssi: Option<i8>,
}

/// What the door needs of the board.
pub struct Ports {
    pub clock: Box<dyn Clock>,
    pub entropy: Box<dyn Entropy>,
    pub vault: Box<dyn Vault>,
    pub readings: Box<dyn Readings>,
}

/// The door of one board, served at `endpoint` and named by `name`, the DNS name it is known by.
pub struct Door {
    endpoint: String,
    manifest: String,
    etag: String,
    ports: Ports,
    signin: Mutex<SignIn>,
}

impl Door {
    pub fn new(name: &str, endpoint: &str, ports: Ports) -> Self {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let manifest = manifest(name, &endpoint).to_string();
        let etag = format!("\"{:016x}\"", fnv1a(manifest.as_bytes()));
        let signin = Mutex::new(SignIn::restore(&ports.vault.read().unwrap_or_default()));
        Self {
            endpoint,
            manifest,
            etag,
            ports,
            signin,
        }
    }

    pub fn answer(&self, req: &Request) -> Response {
        match (req.method, req.path) {
            // HEAD is answered as GET; the server sends it without the body.
            ("GET" | "HEAD", "/.well-known/best") => self.manifest(),
            (_, "/.well-known/best") => not_allowed("GET, HEAD", "the manifest is read with GET"),
            ("POST", "/auth/device") => self.register(req),
            ("POST", "/auth/token") => self.redeem(req),
            (_, "/auth/device" | "/auth/token") => not_allowed("POST", "this endpoint takes a form, POSTed"),
            ("GET" | "HEAD", "/approve") => self.page(form(req.query.as_bytes()).get("code").map(String::as_str)),
            ("POST", "/approve") => self.decide(req),
            (_, "/approve") => not_allowed("GET, HEAD, POST", "the page is read with GET, answered with POST"),
            (_, path) if path == "/queries" || path.starts_with("/queries/") => self.queries(req),
            _ => error(
                404,
                "NOT_FOUND",
                "nothing is served here: GET /.well-known/best lists what is",
            ),
        }
    }

    /// The robot's button was pressed: the code armed on the page, if any, is approved. Returns that code.
    pub fn press(&self) -> Option<String> {
        self.signin().press(self.ports.clock.now_ms())
    }

    /// Every credential is forgotten, kept ones included: each agent signs in again. When the vault cannot be
    /// written, every credential is still refused from now on, but the kept ones come back on a restart: the error
    /// says so.
    pub fn forget_credentials(&self) -> Result<(), String> {
        let mut s = self.signin();
        s.forget_credentials();
        self.ports.vault.write(&s.kept()).map_err(|e| {
            format!("every credential is refused until the robot restarts, but the kept ones could not be erased: {e}")
        })
    }

    /// The manifest, whole, with an ETag and a cache life so a client can tell it changed. An `If-None-Match` is
    /// not answered with 304, which HTTP lets a server do: the server would give a 304 a length of 0, where HTTP
    /// allows it none or the manifest's.
    fn manifest(&self) -> Response {
        Response {
            headers: vec![
                ("ETag", self.etag.clone()),
                ("Cache-Control", "public, max-age=60".into()),
            ],
            ..json_ok(self.manifest.clone())
        }
    }

    fn signin(&self) -> MutexGuard<'_, SignIn> {
        self.signin.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn random(&self) -> impl FnMut(&mut [u8]) + '_ {
        |bytes| self.ports.entropy.fill(bytes)
    }

    fn register(&self, req: &Request) -> Response {
        let f = match form_of(req) {
            Ok(f) => f,
            Err(e) => return oauth_error(e),
        };
        let now = self.ports.clock.now_ms();
        let mut s = self.signin();
        let registered = s.register(
            now,
            &mut self.random(),
            f.get("client_id").map(String::as_str),
            f.get("agent_label").map(String::as_str),
            f.get("connection").map(String::as_str),
            f.get("credential_lifetime").map(String::as_str),
        );
        match registered {
            Ok(r) => no_store(json!({
                "device_code": r.device_code,
                "user_code": r.user_code,
                "verification_uri": format!("{}/approve", self.endpoint),
                "verification_uri_complete": format!("{}/approve?code={}", self.endpoint, r.user_code),
                "expires_in": signin::EXPIRES_IN_S,
                "interval": signin::INTERVAL_S,
                "note": guidance::DEVICE,
            })),
            Err(e) => oauth_error(e),
        }
    }

    fn redeem(&self, req: &Request) -> Response {
        let f = match form_of(req) {
            Ok(f) => f,
            Err(e) => return oauth_error(e),
        };
        match f.get("grant_type").map(String::as_str) {
            Some(DEVICE_GRANT) => {}
            Some(_) => {
                return oauth_error(OAuthError {
                    error: "unsupported_grant_type",
                    description: format!("the robot serves the device-code grant alone: grant_type={DEVICE_GRANT}"),
                })
            }
            None => {
                return oauth_error(OAuthError {
                    error: "invalid_request",
                    description: format!("grant_type is required: {DEVICE_GRANT}"),
                })
            }
        }
        let Some(code) = f.get("device_code") else {
            return oauth_error(OAuthError {
                error: "invalid_request",
                description: "device_code is required: the one the device authorization answered".into(),
            });
        };
        let now = self.ports.clock.now_ms();
        let mut s = self.signin();
        let issued = match s.redeem(now, &mut self.random(), code) {
            Ok(issued) => issued,
            Err(e) => return oauth_error(e),
        };
        if issued.expires_in.is_none() {
            if let Err(e) = self.ports.vault.write(&s.kept()) {
                // Not kept, it would end at the next restart, which a durable credential must not: it is withdrawn.
                s.withdraw(&issued.access_token);
                return Response {
                    status: 500,
                    ..no_store(json!({
                        "error": "server_error",
                        "error_description": format!("the robot could not keep the credential ({e}): register again"),
                    }))
                };
            }
        }
        let host = self
            .endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let mut answer = json!({
            "access_token": issued.access_token,
            "token_type": "Bearer",
            "name": issued.name.as_deref().unwrap_or(host),
            "note": guidance::TOKEN,
            "endpoint": self.endpoint,
            "manifest": format!("{}/.well-known/best", self.endpoint),
            "ready_check": format!("{}/queries", self.endpoint),
            "http": {
                "header": "Authorization: Bearer <access_token>",
                "queries": format!("{}/queries", self.endpoint),
            },
        });
        if let Some(s) = issued.expires_in {
            answer["expires_in"] = s.into();
        }
        no_store(answer)
    }

    fn page(&self, code: Option<&str>) -> Response {
        let now = self.ports.clock.now_ms();
        let s = self.signin();
        let code = code.filter(|c| !c.is_empty());
        html(page::approve(code.and_then(|c| s.find(c)), code, now, None))
    }

    fn decide(&self, req: &Request) -> Response {
        let f = form(req.body);
        let code = f.get("code").map(String::as_str).unwrap_or_default();
        let now = self.ports.clock.now_ms();
        let mut s = self.signin();
        let outcome = match f.get("action").map(String::as_str) {
            Some("approve") => s.arm(now, code),
            Some("deny") => s.deny(now, code),
            _ => {
                return html(page::approve(
                    s.find(code),
                    Some(code),
                    now,
                    Some("Choose Approve or Deny."),
                ))
            }
        };
        let notice = outcome.err().map(|refusal| match refusal {
            Refusal::Unknown => "No sign-in waits under this code.".to_string(),
            Refusal::NotWaiting => "This sign-in is no longer waiting.".to_string(),
            Refusal::Full => format!(
                "The robot holds {} credentials already. Hold its BOOT button for 10 seconds to forget them all, then \
                 approve again.",
                signin::CAPACITY
            ),
            Refusal::Busy { seconds } => {
                format!("Another sign-in waits for the button: try again in {seconds} seconds.")
            }
        });
        html(page::approve(s.find(code), Some(code), now, notice.as_deref()))
    }

    fn queries(&self, req: &Request) -> Response {
        // The scheme is matched without regard to case (RFC 9110 §11.1).
        let token = req
            .authorization
            .and_then(|a| a.trim().split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, token)| token.trim());
        let now = self.ports.clock.now_ms();
        if !token.is_some_and(|t| self.signin().admits(now, t)) {
            return Response {
                headers: vec![("WWW-Authenticate", "Bearer".into())],
                ..error(
                    401,
                    "UNAUTHORIZED",
                    "a credential is required: Authorization: Bearer <token>, obtained by the sign-in the manifest's \
                     authentication block declares",
                )
            };
        }
        if !matches!(req.method, "GET" | "HEAD") {
            return not_allowed("GET, HEAD", "queries are read with GET");
        }
        match req.path {
            "/queries" => json_ok(
                json!({ "queries": [{
                    "schema": "get-status",
                    "version": "1.0",
                    "dataschema": format!("{}/queries/get-status/1.0", self.endpoint),
                    "description": STATUS_DESCRIPTION,
                }] })
                .to_string(),
            ),
            "/queries/get-status/1.0" => json_ok(status_schema().to_string()),
            "/queries/get-status" => json_ok(status(&self.ports.readings.read()).to_string()),
            _ => error(
                404,
                "SCHEMA_NOT_FOUND",
                "the robot answers one query, get-status: GET /queries lists it",
            ),
        }
    }
}

const STATUS_DESCRIPTION: &str = "How the robot is: how long it has run, the memory it has free, and the strength of \
                                  its Wi-Fi signal.";

fn status(s: &Status) -> Value {
    json!({
        "uptimeSeconds": s.uptime_ms / 1000,
        "memory": {
            "internalFree": s.internal_free,
            "internalLeast": s.internal_least,
            "psramFree": s.psram_free,
        },
        "wifi": { "rssi": s.wifi_rssi },
    })
}

fn status_schema() -> Value {
    let bytes = |d: &str| json!({ "type": "integer", "minimum": 0, "description": d });
    json!({
        "description": STATUS_DESCRIPTION,
        "response": {
            "type": "object",
            "required": ["uptimeSeconds", "memory", "wifi"],
            "properties": {
                "uptimeSeconds": { "type": "integer", "minimum": 0, "description": "Seconds since the robot started." },
                "memory": {
                    "type": "object",
                    "required": ["internalFree", "internalLeast", "psramFree"],
                    "properties": {
                        "internalFree": bytes("Bytes of internal RAM free now."),
                        "internalLeast": bytes("The fewest bytes of internal RAM free since the robot started."),
                        "psramFree": bytes("Bytes of PSRAM free now."),
                    },
                },
                "wifi": {
                    "type": "object",
                    "required": ["rssi"],
                    "properties": {
                        "rssi": {
                            "type": ["integer", "null"],
                            "description": "The Wi-Fi signal in dBm, null when there is none to read.",
                        },
                    },
                },
            },
        },
    })
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
            "authentication": {
                "type": "bearer",
                "scheme": "Bearer",
                "deviceAuthorizationUrl": format!("{endpoint}/auth/device"),
                "tokenUrl": format!("{endpoint}/auth/token"),
                "note": guidance::MANIFEST,
            },
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
                "schema": "https://behavioralstate.io/v1/schemas/agents/queries.json",
                "service": service,
                "status": "active",
                "endpoints": [
                    { "method": "GET", "path": "/queries" },
                    { "method": "GET", "path": "/queries/{schema}/{version}" },
                    { "method": "GET", "path": "/queries/{schema}" }
                ]
            }]
        }
    })
}

/// A form body, refused as RFC 6749 §5.2 asks when it is not one.
fn form_of(req: &Request) -> Result<BTreeMap<String, String>, OAuthError> {
    let is_form = req
        .content_type
        .is_some_and(|c| c.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case(FORM));
    if !is_form {
        return Err(OAuthError {
            error: "invalid_request",
            description: format!("the request is a form: Content-Type: {FORM}, as RFC 6749 and RFC 8628 ask"),
        });
    }
    Ok(form(req.body))
}

/// `application/x-www-form-urlencoded`, read as WHATWG does, with a name without a value read as omitted (RFC 6749
/// §3.1): of a repeated name, the last with a value wins.
fn form(bytes: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(bytes)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        // A parameter without a value is read as omitted (RFC 6749 §3.1).
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1).and_then(hex_digit), b.get(i + 2).and_then(hex_digit)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(hi << 4 | lo);
                i += 3;
            }
            (b'+', ..) => {
                out.push(b' ');
                i += 1;
            }
            (x, ..) => {
                out.push(x);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(c: &u8) -> Option<u8> {
    (*c as char).to_digit(16).map(|d| d as u8)
}

fn json_ok(body: String) -> Response {
    Response {
        status: 200,
        content_type: JSON,
        headers: vec![],
        body: Some(body),
    }
}

/// RFC 6749 §5.1: an answer that carries or leads to a credential is never cached.
fn no_store(body: Value) -> Response {
    Response {
        headers: vec![("Cache-Control", "no-store".into()), ("Pragma", "no-cache".into())],
        ..json_ok(body.to_string())
    }
}

fn oauth_error(e: OAuthError) -> Response {
    Response {
        status: 400,
        ..no_store(json!({ "error": e.error, "error_description": e.description }))
    }
}

fn html(body: String) -> Response {
    Response {
        status: 200,
        content_type: HTML,
        headers: vec![("Cache-Control", "no-store".into())],
        body: Some(body),
    }
}

fn not_allowed(allow: &str, message: &str) -> Response {
    Response {
        headers: vec![("Allow", allow.into())],
        ..error(405, "METHOD_NOT_ALLOWED", message)
    }
}

fn error(status: u16, code: &str, message: &str) -> Response {
    let body = json!({ "error": { "code": code, "message": message } }).to_string();
    Response {
        status,
        content_type: JSON,
        headers: vec![],
        body: Some(body),
    }
}

/// The answer to a body longer than the board reads, before the door sees it.
pub fn too_large(limit: usize) -> Response {
    error(
        413,
        "PAYLOAD_TOO_LARGE",
        &format!("the body is longer than the {limit} bytes the robot reads: send a shorter one"),
    )
}

/// FNV-1a, 64 bits: enough to tell one manifest from another, with nothing to link.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf29ce484222325, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests;
