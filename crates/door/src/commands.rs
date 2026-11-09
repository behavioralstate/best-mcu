//! The robot's skills: commands, and the events that say how each one went. The catalogue names a skill, never a
//! motor or a joint: where the head points for "left" is the board's to know.

use std::collections::{BTreeMap, VecDeque};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::camera::{parse_rfc3339, rfc3339};
use crate::signin::hex;
use crate::{error, form, json_ok, not_allowed, unauthorized, Door, Request, Response, Vault, JSON};

const SCHEMA_JSON: &str = "application/schema+json";
const ENVELOPE: &str = "application/cloudevents+json";

/// How many commands the robot remembers, for replay protection, correlation and its event log: a board's flash is
/// small and every command rewrites it.
pub(crate) const REMEMBERED: usize = 8;
/// The longest `id` and `correlationid` taken: each is kept in flash.
const LONGEST_ID: usize = 64;
/// The deepest JSON read: parsing recurses, and the stack is a board's.
const DEEPEST: usize = 8;
/// The shortest time between two commands of one credential.
const PACE_MS: u64 = 500;

const LOOK_DESCRIPTION: &str = "Turn the robot's head to look left, ahead or right. Looked says it was told to, or \
                                LookFailed says why not; the head does not report where it arrived.";

/// Where the robot can look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Toward {
    Left,
    Ahead,
    Right,
}

impl Toward {
    fn named(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "ahead" => Some(Self::Ahead),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// The robot's head.
pub trait Head: Send + Sync {
    /// Turns to look that way, or says why it cannot.
    fn look(&self, toward: Toward) -> Result<(), String>;
    /// Lets go: it holds nothing until told to look again.
    fn rest(&self);
}

/// A command accepted and how it went.
struct Record {
    owner: String,
    id: String,
    fingerprint: String,
    correlation: String,
    status: u16,
    event: Value,
}

impl Record {
    fn kept(&self) -> Value {
        json!({
            "o": self.owner, "i": self.id, "f": self.fingerprint, "c": self.correlation, "s": self.status, "e": self.event,
        })
    }

    fn restored(v: &Value) -> Option<Self> {
        let text = |k: &str| v[k].as_str().map(str::to_string);
        Some(Self {
            owner: text("o")?,
            id: text("i")?,
            fingerprint: text("f")?,
            correlation: text("c")?,
            status: v["s"].as_u64()? as u16,
            event: v.get("e")?.clone(),
        })
    }

    fn answer(&self) -> Response {
        Response {
            status: self.status,
            ..json_ok(json!({ "id": self.id, "correlationId": self.correlation }).to_string())
        }
    }
}

/// The commands the robot remembers, the latest last, and what holds it back.
#[derive(Default)]
pub(crate) struct Ledger {
    records: VecDeque<Record>,
    last_sent: BTreeMap<String, u64>,
    stopped: bool,
}

impl Ledger {
    pub(crate) fn restore(kept: &str) -> Self {
        let records = serde_json::from_str::<Vec<Value>>(kept)
            .unwrap_or_default()
            .iter()
            .filter_map(Record::restored)
            .collect();
        Self {
            records,
            ..Self::default()
        }
    }

    /// Whether this credential sent a command in the last moment: if not, it is counted as having sent one now.
    fn too_soon(&mut self, owner: &str, now: u64) -> bool {
        self.last_sent.retain(|_, at| now - *at < PACE_MS);
        if self.last_sent.contains_key(owner) {
            return true;
        }
        self.last_sent.insert(owner.to_string(), now);
        false
    }

    fn add(&mut self, record: Record) {
        if self.records.len() == REMEMBERED {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }
}

fn refused(status: u16, code: &str, message: &str, details: Value) -> Response {
    let body = json!({ "error": { "code": code, "message": message, "details": details } }).to_string();
    Response {
        status,
        content_type: JSON,
        headers: vec![],
        body: Some(body),
    }
}

fn invalid(field: &str, message: &str) -> Response {
    refused(400, "INVALID_COMMAND", message, json!({ "field": field }))
}

fn invalid_parameter(parameter: &str, message: &str) -> Response {
    refused(400, "INVALID_PARAMETER", message, json!({ "parameter": parameter }))
}

/// Whether the JSON nests deeper than the robot reads: strings do not count.
fn too_deep(body: &[u8]) -> bool {
    let (mut depth, mut in_text, mut escaped) = (0usize, false, false);
    for b in body {
        match (in_text, escaped, b) {
            (true, true, _) => escaped = false,
            (true, false, b'\\') => escaped = true,
            (true, false, b'"') => in_text = false,
            (false, _, b'"') => in_text = true,
            (false, _, b'{' | b'[') => {
                depth += 1;
                if depth > DEEPEST {
                    return true;
                }
            }
            (false, _, b'}' | b']') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// A required attribute of the envelope, as text.
fn text<'a>(envelope: &'a Value, field: &str) -> Result<&'a str, Response> {
    envelope[field].as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
        invalid(
            field,
            &format!("{field} is required, as a string of at least one character"),
        )
    })
}

fn short(field: &str, value: &str) -> Result<(), Response> {
    if value.len() > LONGEST_ID {
        return Err(invalid(
            field,
            &format!("{field} is longer than {LONGEST_ID} characters: send a shorter one"),
        ));
    }
    Ok(())
}

/// The command's data, as `look` takes it.
fn toward_of(data: &Value) -> Result<(Toward, &str), Response> {
    let data = data
        .as_object()
        .ok_or_else(|| invalid("data", "data is required, as an object: { \"toward\": \"left\" }"))?;
    if let Some(other) = data.keys().find(|k| *k != "toward") {
        return Err(invalid(
            &format!("data.{other}"),
            "look takes toward alone: remove this field",
        ));
    }
    let name = data.get("toward").and_then(Value::as_str);
    name.and_then(|n| Toward::named(n).map(|t| (t, n)))
        .ok_or_else(|| invalid("data.toward", "toward is required: left, ahead or right"))
}

pub(crate) fn capabilities(service: &str) -> [Value; 2] {
    let capability = |name: &str, anchor: &str, schema: &str, description: &str, endpoints: Value| {
        json!({
            "name": name,
            "version": "1.0.0",
            "description": description,
            "spec": format!("https://behavioralstate.io/spec#{anchor}"),
            "schema": format!("https://behavioralstate.io/v1/schemas/agents/{schema}.json"),
            "service": service,
            "status": "active",
            "endpoints": endpoints,
        })
    };
    [
        capability(
            "io.best.agents.commands",
            "commands--iobestagentscommands",
            "commands",
            "What the robot can be asked to do.",
            json!([
                { "method": "GET", "path": "/commands" },
                { "method": "POST", "path": "/commands" },
                { "method": "GET", "path": "/commands/{schema}/{version}" }
            ]),
        ),
        capability(
            "io.best.agents.events",
            "events--iobestagentsevents",
            "events",
            "What the robot did.",
            json!([{ "method": "GET", "path": "/events" }]),
        ),
    ]
}

impl Door {
    /// The robot's button, with no sign-in waiting: stops the head and refuses it commands, or lets them through
    /// again. Whether it is now stopped; none on a robot with no head.
    pub fn stop_or_release(&self) -> Option<bool> {
        let head = self.ports.head.as_deref()?;
        let mut ledger = self.ledger();
        ledger.stopped = !ledger.stopped;
        if ledger.stopped {
            head.rest();
        }
        Some(ledger.stopped)
    }

    pub(crate) fn commands(&self, req: &Request) -> Response {
        let now = self.ports.clock.now_ms();
        let Some(owner) = self.credential_of(req, now) else {
            return self.credential_required();
        };
        match (req.method, req.path) {
            ("GET" | "HEAD", "/commands") => json_ok(json!({ "commands": [self.look_entry()] }).to_string()),
            ("POST", "/commands") => self.ingest(req, &hex(&owner), now).unwrap_or_else(|refusal| refusal),
            (_, "/commands") => not_allowed("GET, HEAD, POST", "the catalogue is read with GET, a command is POSTed"),
            ("GET" | "HEAD", "/commands/look/1.0") => Response {
                content_type: SCHEMA_JSON,
                ..json_ok(look_schema().to_string())
            },
            _ => error(
                404,
                "SCHEMA_NOT_FOUND",
                "the robot takes no such command: GET /commands lists what it takes",
            ),
        }
    }

    pub(crate) fn events(&self, req: &Request) -> Response {
        let now = self.ports.clock.now_ms();
        let Some(owner) = self.credential_of(req, now) else {
            return self.credential_required();
        };
        if !matches!(req.method, "GET" | "HEAD") {
            return not_allowed("GET, HEAD", "the event log is read with GET");
        }
        self.read_events(req, &hex(&owner)).unwrap_or_else(|refusal| refusal)
    }

    fn credential_required(&self) -> Response {
        unauthorized(
            "a credential is required: Authorization: Bearer <token>, obtained by the sign-in the manifest's \
             authentication block declares",
        )
    }

    fn look_entry(&self) -> Value {
        json!({
            "schema": "look",
            "version": "1.0",
            "commandType": "Look",
            "dataschema": self.look_uri(),
            "description": LOOK_DESCRIPTION,
        })
    }

    fn look_uri(&self) -> String {
        format!("{}/commands/look/1.0", self.endpoint)
    }

    /// `POST /commands`, in the order the spec's Ingestion Semantics gives.
    fn ingest(&self, req: &Request, owner: &str, now: u64) -> Result<Response, Response> {
        let media = req.content_type.and_then(|c| c.split(';').next()).unwrap_or("").trim();
        if !media.eq_ignore_ascii_case(ENVELOPE) && !media.eq_ignore_ascii_case(JSON) {
            return Err(refused(
                415,
                "UNSUPPORTED_MEDIA_TYPE",
                &format!("a command is a BEST envelope: Content-Type: {ENVELOPE}, or {JSON}"),
                json!({ "field": "Content-Type" }),
            ));
        }
        if too_deep(req.body) {
            return Err(invalid(
                "body",
                &format!("the JSON nests deeper than {DEEPEST} levels: send a flatter one"),
            ));
        }
        let envelope: Value =
            serde_json::from_slice(req.body).map_err(|e| invalid("body", &format!("the body is not JSON: {e}")))?;
        if envelope["specversion"] != "1.0" {
            return Err(invalid("specversion", "specversion is required, and is \"1.0\""));
        }
        if envelope["datacontenttype"] != JSON {
            return Err(invalid(
                "datacontenttype",
                "datacontenttype is required, and is \"application/json\"",
            ));
        }
        let id = text(&envelope, "id")?;
        short("id", id)?;
        text(&envelope, "source")?;
        if parse_rfc3339(text(&envelope, "time")?).is_none() {
            return Err(invalid("time", "time is an RFC 3339 timestamp: 2026-11-09T12:46:15Z"));
        }
        let correlation = match envelope.get("correlationid") {
            None => id,
            Some(c) => {
                let c = c.as_str().filter(|c| !c.is_empty()).ok_or_else(|| {
                    invalid(
                        "correlationid",
                        "correlationid, when sent, is a string of at least one character",
                    )
                })?;
                short("correlationid", c)?;
                c
            }
        };
        let kind = text(&envelope, "type")?;
        if kind != "Look" {
            return Err(invalid(
                "type",
                "the robot takes no such command: GET /commands lists what it takes",
            ));
        }
        if text(&envelope, "dataschema")? != self.look_uri() {
            return Err(invalid(
                "dataschema",
                &format!("dataschema is the catalogue's: {}", self.look_uri()),
            ));
        }
        let (toward, named) = toward_of(&envelope["data"])?;

        let material = format!("{kind}\n{correlation}\n{}", envelope["data"]);
        let fingerprint = hex(&Sha256::digest(material.as_bytes()));
        let mut ledger = self.ledger();
        if let Some(sent) = ledger.records.iter().find(|r| r.owner == owner && r.id == id) {
            if sent.fingerprint != fingerprint {
                return Err(refused(
                    409,
                    "DUPLICATE_ID",
                    &format!("the id {id} was sent with another payload: send this one under a new id"),
                    json!({ "id": id }),
                ));
            }
            return Ok(sent.answer());
        }
        if ledger
            .records
            .iter()
            .any(|r| r.correlation == correlation && r.owner != owner)
        {
            return Err(invalid(
                "correlationid",
                "this correlation id is another sender's: use one of your own",
            ));
        }
        if ledger.stopped {
            return Err(refused(
                400,
                "STOPPED",
                "someone at the robot stopped it with its button: it takes commands again once the button is pressed again",
                json!({}),
            ));
        }
        let Some(utc) = self.ports.clock.utc_ms() else {
            return Err(refused(
                400,
                "TIME_UNKNOWN",
                "the robot does not know the time yet, and every event carries it: try again in a moment",
                json!({}),
            ));
        };
        if ledger.too_soon(owner, now) {
            return Err(Response {
                headers: vec![("Retry-After", "1".into())],
                ..error(
                    429,
                    "TOO_MANY_COMMANDS",
                    "commands are taken at two a second: send it again in a second",
                )
            });
        }

        let head = self
            .ports
            .head
            .as_deref()
            .expect("commands are routed here only with a head");
        let (kind, data) = match head.look(toward) {
            Ok(()) => ("Looked", json!({ "toward": named })),
            Err(reason) => ("LookFailed", json!({ "toward": named, "reason": reason })),
        };
        let mut event_id = [0u8; 16];
        self.ports.entropy.fill(&mut event_id);
        let event = json!({
            "specversion": "1.0",
            "id": hex(&event_id),
            "source": self.endpoint,
            "type": kind,
            "datacontenttype": JSON,
            "correlationid": correlation,
            "time": rfc3339(utc),
            "data": data,
        });
        let mut record = Record {
            owner: owner.to_string(),
            id: id.to_string(),
            fingerprint,
            correlation: correlation.to_string(),
            status: 201,
            event,
        };
        // What the head did cannot be undone: a command the flash did not take is said to be accepted, not durable.
        if self.remember(&mut ledger, &record).is_err() {
            record.status = 202;
        }
        let answer = record.answer();
        ledger.add(record);
        Ok(answer)
    }

    /// Writes what the ledger will hold once this record is in it, before it is.
    fn remember(&self, ledger: &mut Ledger, record: &Record) -> Result<(), String> {
        let kept: Vec<Value> = ledger
            .records
            .iter()
            .skip((ledger.records.len() + 1).saturating_sub(REMEMBERED))
            .chain(std::iter::once(record))
            .map(Record::kept)
            .collect();
        self.ports.log.write(&Value::Array(kept).to_string())
    }

    /// `GET /events`: what this credential's commands did. Oldest first.
    fn read_events(&self, req: &Request, owner: &str) -> Result<Response, Response> {
        let query = form(req.query.as_bytes());
        let get = |name: &str| query.get(name).map(String::as_str);
        let instant = |name: &str| match get(name) {
            None => Ok(None),
            Some(t) => parse_rfc3339(t).map(Some).ok_or_else(|| {
                invalid_parameter(name, &format!("{name} is an RFC 3339 timestamp: 2026-11-09T12:46:15Z"))
            }),
        };
        let (from, to) = (instant("from")?, instant("to")?);
        let limit = match get("limit") {
            None => REMEMBERED,
            Some(l) => l
                .parse::<usize>()
                .ok()
                .filter(|l| *l > 0)
                .ok_or_else(|| invalid_parameter("limit", "limit is a whole number of at least 1"))?,
        };
        let matches = |event: &Value, field: &str, parameter: &str| {
            get(parameter).is_none_or(|wanted| event[field].as_str() == Some(wanted))
        };
        let ledger = self.ledger();
        let events: Vec<&Value> = ledger
            .records
            .iter()
            .filter(|r| r.owner == owner)
            .map(|r| &r.event)
            .filter(|e| {
                matches(e, "type", "type")
                    && matches(e, "correlationid", "correlationId")
                    && matches(e, "source", "source")
            })
            .filter(|e| {
                let at = e["time"].as_str().and_then(parse_rfc3339);
                from.is_none_or(|f| at >= Some(f)) && to.is_none_or(|t| at <= Some(t))
            })
            .collect();
        let start = match get("after") {
            None => 0,
            Some(cursor) => {
                1 + events
                    .iter()
                    .position(|e| e["id"].as_str() == Some(cursor))
                    .ok_or_else(|| {
                        invalid_parameter(
                            "after",
                            "after is a nextCursor this log gave, and that event is gone: read from the start",
                        )
                    })?
            }
        };
        let page: Vec<&&Value> = events[start..].iter().take(limit).collect();
        let mut list = json!({ "events": page });
        if start + page.len() < events.len() {
            list["nextCursor"] = page[page.len() - 1]["id"].clone();
        }
        Ok(json_ok(list.to_string()))
    }
}

fn look_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "commandType": "Look",
        "description": LOOK_DESCRIPTION,
        "produces": ["Looked", "LookFailed"],
        "type": "object",
        "required": ["toward"],
        "additionalProperties": false,
        "properties": {
            "toward": { "type": "string", "enum": ["left", "ahead", "right"], "description": "Where to look." },
        },
    })
}

/// What the flash holds of the ledger when a door starts.
pub(crate) fn restore(log: &dyn Vault) -> Ledger {
    Ledger::restore(&log.read().unwrap_or_default())
}
