use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

const ENDPOINT: &str = "https://robot.example.com";

#[derive(Clone, Default)]
struct Board {
    now: Arc<AtomicU64>,
    next: Arc<AtomicU8>,
    kept: Arc<Mutex<Option<String>>>,
    vault_fails: Arc<Mutex<bool>>,
    eye: Arc<Mutex<Option<Box<dyn Camera>>>>,
    neck: Arc<Mutex<Option<Box<dyn Head>>>>,
    log: Shelf,
    no_time: Arc<Mutex<bool>>,
}

/// A second place in the flash.
#[derive(Clone, Default)]
struct Shelf {
    kept: Arc<Mutex<Option<String>>>,
    fails: Arc<Mutex<bool>>,
}

impl Vault for Shelf {
    fn read(&self) -> Option<String> {
        self.kept.lock().unwrap().clone()
    }
    fn write(&self, kept: &str) -> Result<(), String> {
        if *self.fails.lock().unwrap() {
            return Err("flash full".into());
        }
        *self.kept.lock().unwrap() = Some(kept.to_string());
        Ok(())
    }
}

impl Clock for Board {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn utc_ms(&self) -> Option<u64> {
        (!*self.no_time.lock().unwrap()).then_some(1_794_229_199_007)
    }
}

impl Entropy for Board {
    fn fill(&self, bytes: &mut [u8]) {
        for b in bytes {
            *b = self.next.fetch_add(37, Ordering::SeqCst);
        }
    }
}

impl Vault for Board {
    fn read(&self) -> Option<String> {
        self.kept.lock().unwrap().clone()
    }
    fn write(&self, kept: &str) -> Result<(), String> {
        if *self.vault_fails.lock().unwrap() {
            return Err("flash full".into());
        }
        *self.kept.lock().unwrap() = Some(kept.to_string());
        Ok(())
    }
}

impl Readings for Board {
    fn read(&self) -> Status {
        Status {
            uptime_ms: self.now_ms(),
            internal_free: 240_000,
            internal_least: 150_000,
            psram_free: 8_000_000,
            wifi_rssi: Some(-52),
        }
    }
}

/// A camera that gives the same picture, or the same failure, every time.
struct Eye(Result<Vec<u8>, String>);

impl Camera for Eye {
    fn take(&self) -> Result<Frame, String> {
        self.0.clone().map(|jpeg| Frame {
            jpeg,
            width: 640,
            height: 480,
        })
    }
}

impl Board {
    fn seeing(self, picture: Result<&[u8], &str>) -> Self {
        let eye = Eye(picture.map(<[u8]>::to_vec).map_err(str::to_string));
        *self.eye.lock().unwrap() = Some(Box::new(eye));
        self
    }

    fn door(&self) -> Door {
        self.door_at(ENDPOINT)
    }

    fn door_at(&self, endpoint: &str) -> Door {
        Door::new(
            "robot.example.com",
            endpoint,
            Ports {
                clock: Box::new(self.clone()),
                entropy: Box::new(self.clone()),
                vault: Box::new(self.clone()),
                readings: Box::new(self.clone()),
                camera: self.eye.lock().unwrap().take(),
                head: self.neck.lock().unwrap().take(),
                log: Box::new(self.log.clone()),
            },
        )
    }
    fn wait(&self, seconds: u64) {
        self.now.fetch_add(seconds * 1000, Ordering::SeqCst);
    }
}

fn req<'a>(method: &'a str, path: &'a str) -> Request<'a> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    Request {
        method,
        path,
        query,
        authorization: None,
        content_type: None,
        body: b"",
    }
}

fn post_form<'a>(path: &'a str, body: &'a str) -> Request<'a> {
    Request {
        content_type: Some("application/x-www-form-urlencoded"),
        body: body.as_bytes(),
        ..req("POST", path)
    }
}

fn json_of(r: &Response) -> Value {
    serde_json::from_str(r.body.as_deref().unwrap()).unwrap()
}

fn register(door: &Door, form: &str) -> Value {
    let r = door.answer(&post_form("/auth/device", form));
    assert_eq!(r.status, 200, "{:?}", r.body);
    json_of(&r)
}

fn poll(door: &Door, device_code: &str) -> Response {
    let body = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code={device_code}&client_id=t"
    );
    door.answer(&post_form("/auth/token", &body))
}

fn approve(door: &Door, user_code: &str) -> Response {
    door.answer(&post_form("/approve", &format!("code={user_code}&action=approve")))
}

fn query<'a>(path: &'a str, token: &'a str) -> Request<'a> {
    Request {
        authorization: Some(token),
        ..req("GET", path)
    }
}

/// An agent signed in all the way: registered, armed on the page, approved by the button, redeemed.
fn signed_in(board: &Board, door: &Door, form: &str) -> Value {
    let reg = register(door, form);
    approve(door, reg["user_code"].as_str().unwrap());
    assert!(door.press().is_some());
    board.wait(5);
    let r = poll(door, reg["device_code"].as_str().unwrap());
    assert_eq!(r.status, 200, "{:?}", r.body);
    json_of(&r)
}

#[test]
fn the_manifest_is_public_json_under_the_reversed_name_with_an_etag_and_a_cache_life() {
    let r = Board::default().door().answer(&req("GET", "/.well-known/best"));
    assert_eq!((r.status, r.content_type), (200, JSON));
    let m = json_of(&r);
    assert_eq!(m["best"]["version"], BEST_VERSION);
    assert_eq!(m["best"]["services"]["com.example.robot"]["http"]["endpoint"], ENDPOINT);
    assert_eq!(m["best"]["capabilities"][0]["service"], "com.example.robot");
    assert!(r.headers.iter().any(|(h, v)| *h == "ETag" && v.starts_with('"')));
    assert!(r
        .headers
        .iter()
        .any(|(h, v)| *h == "Cache-Control" && !v.contains("no-store")));
}

#[test]
fn the_manifest_declares_the_sign_in_with_the_guidance_verbatim_and_the_queries_active() {
    let m = json_of(&Board::default().door().answer(&req("GET", "/.well-known/best")));
    let auth = &m["best"]["authentication"];
    assert_eq!(auth["type"], "bearer");
    assert_eq!(auth["deviceAuthorizationUrl"], format!("{ENDPOINT}/auth/device"));
    assert_eq!(auth["tokenUrl"], format!("{ENDPOINT}/auth/token"));
    assert_eq!(auth["note"], guidance::MANIFEST);
    let q = &m["best"]["capabilities"][0];
    assert_eq!(q["status"], "active");
    assert_eq!(q["schema"], "https://behavioralstate.io/v1/schemas/agents/queries.json");
}

#[test]
fn what_is_not_served_is_refused_in_the_best_error_format_with_what_to_do() {
    let door = Board::default().door();
    let r = door.answer(&req("GET", "/nothing"));
    assert_eq!(r.status, 404);
    assert_eq!(json_of(&r)["error"]["code"], "NOT_FOUND");
    let r = door.answer(&req("POST", "/.well-known/best"));
    assert_eq!(r.status, 405);
    assert_eq!(r.headers, vec![("Allow", "GET, HEAD".to_string())]);
    assert_eq!(json_of(&r)["error"]["code"], "METHOD_NOT_ALLOWED");
}

#[test]
fn head_on_the_manifest_is_answered_as_get_is() {
    let door = Board::default().door();
    assert_eq!(
        door.answer(&req("HEAD", "/.well-known/best")),
        door.answer(&req("GET", "/.well-known/best"))
    );
}

#[test]
fn the_device_answer_carries_rfc_8628_s_members_a_secret_code_and_the_device_text() {
    let door = Board::default().door();
    let a = register(&door, "client_id=best&connection=robot&agent_label=Claude");
    let (device, user) = (a["device_code"].as_str().unwrap(), a["user_code"].as_str().unwrap());
    assert_eq!(device.len(), 64);
    assert_ne!(device, user);
    assert_eq!(user.len(), 9);
    assert_eq!(a["verification_uri"], format!("{ENDPOINT}/approve"));
    let complete = a["verification_uri_complete"].as_str().unwrap();
    assert!(complete.ends_with(user) && !complete.contains(device));
    assert_eq!((a["expires_in"].as_u64(), a["interval"].as_u64()), (Some(300), Some(5)));
    assert_eq!(a["note"], guidance::DEVICE);
}

#[test]
fn the_sign_in_endpoints_refuse_in_rfc_6749_s_words_and_say_why() {
    let door = Board::default().door();
    let refused = |r: Response, error: &str| {
        assert_eq!(r.status, 400, "{:?}", r.body);
        let e = json_of(&r);
        assert_eq!(e["error"], error);
        assert!(!e["error_description"].as_str().unwrap().is_empty());
    };
    let json = Request {
        content_type: Some("application/json"),
        body: br#"{"client_id":"best"}"#,
        ..req("POST", "/auth/device")
    };
    refused(door.answer(&json), "invalid_request");
    refused(
        door.answer(&post_form("/auth/device", "client_id=best&connection=Not+One+Word")),
        "invalid_request",
    );
    refused(
        door.answer(&post_form("/auth/device", "connection=robot")),
        "invalid_request",
    );
    refused(
        door.answer(&post_form("/auth/device", "client_id=best&credential_lifetime=forever")),
        "invalid_request",
    );
    refused(poll(&door, "unknown"), "invalid_grant");
    refused(
        door.answer(&post_form("/auth/token", "grant_type=client_credentials")),
        "unsupported_grant_type",
    );
}

#[test]
fn an_agent_polls_pending_is_told_to_slow_down_and_collects_its_credential_once_the_button_approves() {
    let board = Board::default();
    let door = board.door();
    let reg = register(&door, "client_id=best&connection=robot");
    let (device, user) = (reg["device_code"].as_str().unwrap(), reg["user_code"].as_str().unwrap());
    assert_eq!(json_of(&poll(&door, device))["error"], "authorization_pending");
    assert_eq!(json_of(&poll(&door, device))["error"], "slow_down");
    assert_eq!(door.press(), None, "a press approves nothing that is not armed");
    approve(&door, &user.to_lowercase().replace('-', ""));
    assert_eq!(door.press().as_deref(), Some(user));
    board.wait(10);
    let r = poll(&door, device);
    assert_eq!(r.status, 200);
    assert!(r.headers.contains(&("Cache-Control", "no-store".into())));
    let t = json_of(&r);
    assert_eq!(t["token_type"], "Bearer");
    assert_eq!(t["name"], "robot");
    assert!(t["note"].as_str().unwrap().starts_with(guidance::TOKEN));
    assert_eq!(t["endpoint"], ENDPOINT);
    assert!(t.get("expires_in").is_none() && t.get("tenant_id").is_none());
    assert!(!t["http"]["header"]
        .as_str()
        .unwrap()
        .contains(t["access_token"].as_str().unwrap()));
    assert_eq!(
        json_of(&poll(&door, device))["error"],
        "invalid_grant",
        "a device_code is redeemed once"
    );
}

#[test]
fn the_credential_opens_the_queries_and_nothing_else_does() {
    let board = Board::default();
    let door = board.door();
    for auth in [None, Some("Bearer nope")] {
        let r = door.answer(&Request {
            authorization: auth,
            ..req("GET", "/queries")
        });
        assert_eq!(r.status, 401);
        assert_eq!(json_of(&r)["error"]["code"], "UNAUTHORIZED");
        assert!(r.headers.contains(&("WWW-Authenticate", "Bearer".into())));
    }
    let token = format!(
        "Bearer {}",
        signed_in(&board, &door, "client_id=best")["access_token"]
            .as_str()
            .unwrap()
    );
    let catalogue = json_of(&door.answer(&query("/queries", &token)));
    let entry = &catalogue["queries"][0];
    assert_eq!(
        (entry["schema"].as_str(), entry["version"].as_str()),
        (Some("get-status"), Some("1.0"))
    );
    assert_eq!(entry["dataschema"], format!("{ENDPOINT}/queries/get-status/1.0"));
    assert!(json_of(&door.answer(&query("/queries/get-status/1.0", &token)))["response"].is_object());
    let s = json_of(&door.answer(&query("/queries/get-status", &token)));
    assert_eq!(s["memory"]["internalFree"], 240_000);
    assert_eq!(s["wifi"]["rssi"], -52);
    let r = door.answer(&query("/queries/get-nothing", &token));
    assert_eq!(
        (r.status, json_of(&r)["error"]["code"].as_str()),
        (404, Some("SCHEMA_NOT_FOUND"))
    );
}

#[test]
fn a_session_credential_ends_after_its_life_and_is_never_kept() {
    let board = Board::default();
    let door = board.door();
    let t = signed_in(&board, &door, "client_id=claude&credential_lifetime=session");
    assert_eq!(t["expires_in"], signin::SESSION_S);
    assert_eq!(
        t["name"], "robot.example.com",
        "with no connection, the name is the manifest's address"
    );
    let token = format!("Bearer {}", t["access_token"].as_str().unwrap());
    assert_eq!(door.answer(&query("/queries", &token)).status, 200);
    assert!(board.kept.lock().unwrap().is_none());
    board.wait(signin::SESSION_S);
    assert_eq!(door.answer(&query("/queries", &token)).status, 401);
}

#[test]
fn a_durable_credential_survives_a_restart_until_the_long_press_forgets_it() {
    let board = Board::default();
    let t = signed_in(&board, &board.door(), "client_id=best&connection=robot");
    let token = format!("Bearer {}", t["access_token"].as_str().unwrap());
    let kept = board.kept.lock().unwrap().clone().unwrap();
    assert!(
        !kept.contains(t["access_token"].as_str().unwrap()),
        "only its hash is kept"
    );
    let restarted = board.door();
    assert_eq!(restarted.answer(&query("/queries", &token)).status, 200);
    restarted.forget_credentials().unwrap();
    assert_eq!(restarted.answer(&query("/queries", &token)).status, 401);
    assert_eq!(board.door().answer(&query("/queries", &token)).status, 401);
}

#[test]
fn a_credential_the_flash_cannot_keep_is_withdrawn_and_the_agent_told() {
    let board = Board::default();
    let door = board.door();
    *board.vault_fails.lock().unwrap() = true;
    let reg = register(&door, "client_id=best");
    approve(&door, reg["user_code"].as_str().unwrap());
    door.press();
    let r = poll(&door, reg["device_code"].as_str().unwrap());
    assert_eq!(r.status, 500);
    assert_eq!(json_of(&r)["error"], "server_error");
}

#[test]
fn denied_and_expired_registrations_end_and_say_so() {
    let board = Board::default();
    let door = board.door();
    let denied = register(&door, "client_id=best");
    door.answer(&post_form(
        "/approve",
        &format!("code={}&action=deny", denied["user_code"].as_str().unwrap()),
    ));
    assert_eq!(
        json_of(&poll(&door, denied["device_code"].as_str().unwrap()))["error"],
        "access_denied"
    );
    let late = register(&door, "client_id=best");
    board.wait(signin::EXPIRES_IN_S);
    assert_eq!(
        json_of(&poll(&door, late["device_code"].as_str().unwrap()))["error"],
        "expired_token"
    );
    let r = approve(&door, late["user_code"].as_str().unwrap());
    assert!(r.body.unwrap().contains("no longer waiting"));
}

#[test]
fn an_armed_code_waits_thirty_seconds_and_only_one_is_armed_at_a_time() {
    let board = Board::default();
    let door = board.door();
    let (a, b) = (register(&door, "client_id=a"), register(&door, "client_id=b"));
    approve(&door, a["user_code"].as_str().unwrap());
    let busy = approve(&door, b["user_code"].as_str().unwrap()).body.unwrap();
    assert!(busy.contains("Another sign-in waits for the button"), "{busy}");
    board.wait(signin::ARMED_FOR_S);
    assert_eq!(door.press(), None, "the press came too late");
    approve(&door, b["user_code"].as_str().unwrap());
    assert_eq!(door.press().as_deref(), b["user_code"].as_str());
}

#[test]
fn the_robot_holds_eight_credentials_and_says_how_to_free_them() {
    let board = Board::default();
    let door = board.door();
    for _ in 0..signin::CAPACITY {
        signed_in(&board, &door, "client_id=best");
    }
    let reg = register(&door, "client_id=best");
    let page = approve(&door, reg["user_code"].as_str().unwrap()).body.unwrap();
    assert!(page.contains("holds 8 credentials already"), "{page}");
    assert_eq!(door.press(), None);
}

#[test]
fn too_many_waiting_sign_ins_are_refused_until_one_ends() {
    let board = Board::default();
    let door = board.door();
    for _ in 0..signin::WAITING {
        register(&door, "client_id=best");
    }
    let r = door.answer(&post_form("/auth/device", "client_id=best"));
    assert_eq!(
        (r.status, json_of(&r)["error"].as_str()),
        (400, Some("temporarily_unavailable"))
    );
    board.wait(signin::EXPIRES_IN_S);
    register(&door, "client_id=best");
}

#[test]
fn the_page_shows_the_agent_s_words_escaped() {
    let door = Board::default().door();
    let reg = register(
        &door,
        "client_id=%3Cscript%3Ealert(1)%3C%2Fscript%3E&agent_label=%22%3E%3Cimg%3E",
    );
    let code = reg["user_code"].as_str().unwrap();
    let r = door.answer(&req("GET", &format!("/approve?code={code}")));
    assert_eq!(r.content_type, HTML);
    let page = r.body.unwrap();
    assert!(!page.contains("<script>") && !page.contains("<img>"), "{page}");
    assert!(page.contains("&lt;script&gt;") && page.contains(code));
    assert!(door.answer(&req("GET", "/approve")).body.unwrap().contains("name=code"));
}

#[test]
fn a_form_is_read_as_browsers_write_it() {
    let f = form(b"a=1+2&b=%41%2b%zz&c&d=&a=3");
    assert_eq!(f.get("a").map(String::as_str), Some("3"));
    assert_eq!(f.get("b").map(String::as_str), Some("A+%zz"));
    assert_eq!(
        (f.get("c"), f.get("d")),
        (None, None),
        "a name without a value is omitted"
    );
}

#[test]
fn a_registration_s_empty_optional_parameters_are_read_as_omitted() {
    let board = Board::default();
    let door = board.door();
    let t = signed_in(
        &board,
        &door,
        "client_id=best&agent_label=&connection=&credential_lifetime=",
    );
    assert!(t.get("expires_in").is_none(), "durable, as when omitted");
    assert_eq!(t["name"], "robot.example.com");
}

#[test]
fn the_bearer_scheme_is_matched_without_regard_to_case() {
    let board = Board::default();
    let door = board.door();
    let token = signed_in(&board, &door, "client_id=best")["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    for scheme in ["Bearer", "bearer", "BEARER"] {
        let auth = format!("{scheme} {token}");
        assert_eq!(door.answer(&query("/queries", &auth)).status, 200, "{scheme}");
    }
}

#[test]
fn the_long_press_takes_back_an_approval_not_yet_collected() {
    let board = Board::default();
    let door = board.door();
    let reg = register(&door, "client_id=best");
    approve(&door, reg["user_code"].as_str().unwrap());
    assert!(door.press().is_some());
    let armed = register(&door, "client_id=other");
    approve(&door, armed["user_code"].as_str().unwrap());
    door.forget_credentials().unwrap();
    assert_eq!(door.press(), None, "the armed code is taken back too");
    board.wait(5);
    assert_eq!(
        json_of(&poll(&door, reg["device_code"].as_str().unwrap()))["error"],
        "access_denied"
    );
}

#[test]
fn the_long_press_leaves_a_code_whose_arming_lapsed_waiting_for_the_person() {
    let board = Board::default();
    let door = board.door();
    let reg = register(&door, "client_id=best");
    approve(&door, reg["user_code"].as_str().unwrap());
    board.wait(40);
    door.forget_credentials().unwrap();
    assert_eq!(
        json_of(&poll(&door, reg["device_code"].as_str().unwrap()))["error"],
        "authorization_pending"
    );
}

#[test]
fn the_long_press_leaves_an_approval_that_ran_out_expired() {
    let board = Board::default();
    let door = board.door();
    let reg = register(&door, "client_id=best");
    approve(&door, reg["user_code"].as_str().unwrap());
    assert!(door.press().is_some());
    board.wait(310);
    door.forget_credentials().unwrap();
    assert_eq!(
        json_of(&poll(&door, reg["device_code"].as_str().unwrap()))["error"],
        "expired_token"
    );
}

#[test]
fn a_long_press_the_flash_cannot_keep_still_refuses_every_credential_and_says_so() {
    let board = Board::default();
    let door = board.door();
    let t = signed_in(&board, &door, "client_id=best");
    let token = format!("Bearer {}", t["access_token"].as_str().unwrap());
    *board.vault_fails.lock().unwrap() = true;
    let e = door.forget_credentials().unwrap_err();
    assert!(e.contains("refused until the robot restarts"), "{e}");
    assert_eq!(door.answer(&query("/queries", &token)).status, 401);
}

#[test]
fn an_approval_frees_its_waiting_place_and_ends_with_the_code_s_time() {
    let board = Board::default();
    let door = board.door();
    let mut approved = Vec::new();
    for _ in 0..signin::WAITING {
        let reg = register(&door, "client_id=best");
        approve(&door, reg["user_code"].as_str().unwrap());
        assert!(door.press().is_some());
        approved.push(reg);
    }
    register(&door, "client_id=best");
    board.wait(signin::EXPIRES_IN_S);
    assert_eq!(
        json_of(&poll(&door, approved[0]["device_code"].as_str().unwrap()))["error"],
        "expired_token"
    );
}

#[test]
fn a_body_too_long_for_the_board_is_refused_in_the_best_error_format() {
    let r = too_large(4096);
    assert_eq!(r.status, 413);
    assert!(json_of(&r)["error"]["message"].as_str().unwrap().contains("4096"));
}

const PICTURE: &[u8] = b"\xff\xd8a picture\xff\xd9";

fn watch<'a>(path: &'a str, authorization: Option<&'a str>) -> Request<'a> {
    Request {
        authorization,
        ..req("GET", path)
    }
}

fn token_of(board: &Board, door: &Door) -> String {
    format!(
        "Bearer {}",
        signed_in(board, door, "client_id=best")["access_token"]
            .as_str()
            .unwrap()
    )
}

#[test]
fn a_board_without_a_camera_declares_none_and_serves_none() {
    let board = Board::default();
    let door = board.door();
    let m = json_of(&door.answer(&req("GET", "/.well-known/best")));
    assert_eq!(m["best"]["capabilities"].as_array().unwrap().len(), 1);
    let token = token_of(&board, &door);
    let catalogue = json_of(&door.answer(&query("/queries", &token)));
    assert_eq!(catalogue["queries"].as_array().unwrap().len(), 1);
    assert_eq!(door.answer(&query("/queries/get-snapshot", &token)).status, 404);
    let refused = door.admit_watcher(&watch("/camera/stream", Some(&token))).unwrap_err();
    assert_eq!(refused.status, 404);
}

#[test]
fn the_manifest_declares_the_live_frames_as_a_service_of_their_own() {
    let door = Board::default().seeing(Ok(PICTURE)).door();
    let m = json_of(&door.answer(&req("GET", "/.well-known/best")));
    let service = "com.example.robot.camera";
    assert_eq!(
        m["best"]["services"][service]["http"]["endpoint"],
        format!("{ENDPOINT}:{STREAM_PORT}")
    );
    let capability = &m["best"]["capabilities"][1];
    assert_eq!(capability["name"], service);
    assert_eq!(capability["service"], service);
    assert_eq!(capability["endpoints"][0]["path"], "/camera/stream");
}

#[test]
fn a_snapshot_is_behind_the_sign_in_and_answers_the_picture_as_json() {
    use base64::Engine;

    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    assert_eq!(door.answer(&req("GET", "/queries/get-snapshot")).status, 401);
    let token = token_of(&board, &door);
    let catalogue = json_of(&door.answer(&query("/queries", &token)));
    let names: Vec<_> = catalogue["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["schema"].clone())
        .collect();
    assert_eq!(names, ["get-status", "get-snapshot", "get-camera-link"]);
    assert!(json_of(&door.answer(&query("/queries/get-snapshot/1.0", &token)))["response"].is_object());
    let r = door.answer(&query("/queries/get-snapshot", &token));
    assert_eq!((r.status, r.content_type), (200, JSON));
    assert!(r.headers.iter().any(|(h, v)| *h == "Cache-Control" && v == "no-store"));
    let s = json_of(&r);
    assert_eq!(s["contentType"], "image/jpeg");
    assert_eq!(s["takenAt"], "2026-11-09T12:59:59.007Z");
    assert_eq!((s["width"].as_u64(), s["height"].as_u64()), (Some(640), Some(480)));
    let data = base64::engine::general_purpose::STANDARD
        .decode(s["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(data, PICTURE);
}

#[test]
fn a_camera_that_gives_no_picture_is_refused_with_what_to_do() {
    let board = Board::default().seeing(Err("no frame in time"));
    let door = board.door();
    let token = token_of(&board, &door);
    let r = door.answer(&query("/queries/get-snapshot", &token));
    assert_eq!(r.status, 503);
    let e = json_of(&r);
    assert_eq!(e["error"]["code"], "CAMERA_UNAVAILABLE");
    assert!(e["error"]["message"].as_str().unwrap().contains("no frame in time"));
}

#[test]
fn live_frames_are_refused_without_a_credential_or_a_link_and_say_how() {
    let door = Board::default().seeing(Ok(PICTURE)).door();
    let r = door
        .admit_watcher(&watch("/camera/stream?ticket=wrong", None))
        .unwrap_err();
    assert_eq!(r.status, 401);
    assert!(r.headers.iter().any(|(h, _)| *h == "WWW-Authenticate"));
    assert!(json_of(&r)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("get-camera-link"));
}

#[test]
fn a_credential_opens_the_live_frames_and_each_part_is_one_jpeg() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let watcher = door.admit_watcher(&watch("/camera/stream", Some(&token))).unwrap();
    let mut expected = format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        PICTURE.len()
    )
    .into_bytes();
    expected.extend_from_slice(PICTURE);
    expected.extend_from_slice(b"\r\n");
    assert_eq!(door.next_part(&watcher).unwrap(), expected);
}

fn ticket_of(door: &Door, token: &str) -> String {
    let link = json_of(&door.answer(&query("/queries/get-camera-link", token)));
    let url = link["url"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("{ENDPOINT}:{STREAM_PORT}/camera/stream?ticket=")),
        "{url}"
    );
    assert_eq!(link["expiresIn"], 30);
    url.rsplit_once('=').unwrap().1.to_string()
}

#[test]
fn a_camera_link_opens_once() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let path = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    assert!(door.admit_watcher(&watch(&path, None)).is_ok());
    assert_eq!(door.admit_watcher(&watch(&path, None)).unwrap_err().status, 401);
}

#[test]
fn a_camera_link_ends_after_its_time_and_a_new_one_replaces_it() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let late = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    board.wait(31);
    assert_eq!(door.admit_watcher(&watch(&late, None)).unwrap_err().status, 401);
    let replaced = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    let latest = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    assert_eq!(door.admit_watcher(&watch(&replaced, None)).unwrap_err().status, 401);
    assert!(door.admit_watcher(&watch(&latest, None)).is_ok());
}

#[test]
fn the_live_frames_are_not_on_the_doors_own_port() {
    let door = Board::default().seeing(Ok(PICTURE)).door();
    let r = door.answer(&req("GET", "/camera/stream"));
    assert_eq!(r.status, 404);
    assert!(json_of(&r)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("get-camera-link"));
}

#[test]
fn live_frames_opened_with_a_credential_end_when_it_is_forgotten() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let watcher = door.admit_watcher(&watch("/camera/stream", Some(&token))).unwrap();
    assert!(door.next_part(&watcher).is_ok());
    door.forget_credentials().unwrap();
    assert!(door.next_part(&watcher).unwrap_err().contains("ended"));
}

#[test]
fn live_frames_opened_with_a_link_end_with_the_credential_that_asked_for_it() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let path = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    let watcher = door.admit_watcher(&watch(&path, None)).unwrap();
    assert!(door.next_part(&watcher).is_ok());
    door.forget_credentials().unwrap();
    assert!(door.next_part(&watcher).is_err());
}

#[test]
fn live_frames_end_when_a_session_credential_runs_out() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let t = signed_in(&board, &door, "client_id=claude&credential_lifetime=session");
    let token = format!("Bearer {}", t["access_token"].as_str().unwrap());
    let watcher = door.admit_watcher(&watch("/camera/stream", Some(&token))).unwrap();
    assert!(door.next_part(&watcher).is_ok());
    board.wait(t["expires_in"].as_u64().unwrap());
    assert!(door.next_part(&watcher).is_err());
}

#[test]
fn a_camera_link_taken_before_the_credentials_are_forgotten_opens_nothing() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let path = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    door.forget_credentials().unwrap();
    assert_eq!(door.admit_watcher(&watch(&path, None)).unwrap_err().status, 401);
}

#[test]
fn the_live_frames_keep_their_port_when_the_endpoint_names_one() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door_at("https://robot.example.com:444");
    let m = json_of(&door.answer(&req("GET", "/.well-known/best")));
    assert_eq!(
        m["best"]["services"]["com.example.robot.camera"]["http"]["endpoint"],
        format!("https://robot.example.com:{STREAM_PORT}")
    );
}

#[test]
fn a_head_on_the_camera_link_leaves_the_link_waiting() {
    let board = Board::default().seeing(Ok(PICTURE));
    let door = board.door();
    let token = token_of(&board, &door);
    let path = format!("/camera/stream?ticket={}", ticket_of(&door, &token));
    let head = Request {
        method: "HEAD",
        ..query("/queries/get-camera-link", &token)
    };
    assert_eq!(door.answer(&head).status, 200);
    assert!(door.admit_watcher(&watch(&path, None)).is_ok());
}

#[test]
fn a_head_on_the_snapshot_is_answered_as_a_get_is() {
    for picture in [Ok(PICTURE), Err("no frame in time")] {
        let board = Board::default().seeing(picture);
        let door = board.door();
        let token = token_of(&board, &door);
        let get = door.answer(&query("/queries/get-snapshot", &token));
        let head = door.answer(&Request {
            method: "HEAD",
            ..query("/queries/get-snapshot", &token)
        });
        assert_eq!((head.status, head.content_type), (get.status, get.content_type));
        assert_eq!(head.headers, get.headers);
    }
}

#[derive(Clone, Default)]
struct Neck {
    moves: Arc<Mutex<Vec<Toward>>>,
    rests: Arc<AtomicU8>,
    fails: Arc<Mutex<bool>>,
}

impl Head for Neck {
    fn look(&self, toward: Toward) -> Result<(), String> {
        if *self.fails.lock().unwrap() {
            return Err("no signal".into());
        }
        self.moves.lock().unwrap().push(toward);
        Ok(())
    }
    fn rest(&self) {
        self.rests.fetch_add(1, Ordering::SeqCst);
    }
}

impl Neck {
    fn moved(&self) -> Vec<Toward> {
        self.moves.lock().unwrap().clone()
    }
}

/// A board with a head, its door, and a credential for it.
fn turning() -> (Board, Neck, Door, String) {
    let (board, neck) = (Board::default(), Neck::default());
    *board.neck.lock().unwrap() = Some(Box::new(neck.clone()));
    let door = board.door();
    let token = token_of(&board, &door);
    (board, neck, door, token)
}

fn envelope(id: &str, toward: &str) -> Value {
    json!({
        "specversion": "1.0",
        "id": id,
        "source": "urn:test",
        "type": "Look",
        "datacontenttype": "application/json",
        "dataschema": format!("{ENDPOINT}/commands/look/1.0"),
        "time": "2026-11-09T12:46:15Z",
        "data": { "toward": toward },
    })
}

fn send(door: &Door, token: &str, body: &Value) -> Response {
    send_as(door, token, "application/cloudevents+json", body.to_string().as_bytes())
}

fn send_as(door: &Door, token: &str, kind: &str, body: &[u8]) -> Response {
    door.answer(&Request {
        method: "POST",
        path: "/commands",
        query: "",
        authorization: Some(token),
        content_type: Some(kind),
        body,
    })
}

fn field_of(r: &Response) -> String {
    json_of(r)["error"]["details"]["field"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn events_of(door: &Door, token: &str, query_string: &str) -> Value {
    let path = format!("/events?{query_string}");
    let r = door.answer(&query(&path, token));
    assert_eq!(r.status, 200, "{:?}", r.body);
    json_of(&r)
}

#[test]
fn a_board_without_a_head_declares_no_commands_and_takes_none() {
    let board = Board::default();
    let door = board.door();
    let m = json_of(&door.answer(&req("GET", "/.well-known/best")));
    let names: Vec<_> = m["best"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["io.best.agents.queries"]);
    assert_eq!(door.answer(&req("GET", "/commands")).status, 404);
    assert_eq!(door.answer(&req("GET", "/events")).status, 404);
}

#[test]
fn a_board_with_a_head_declares_commands_and_events() {
    let (_, _, door, _) = turning();
    let m = json_of(&door.answer(&req("GET", "/.well-known/best")));
    let names: Vec<_> = m["best"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"io.best.agents.commands".to_string()));
    assert!(names.contains(&"io.best.agents.events".to_string()));
}

#[test]
fn commands_and_events_are_behind_the_sign_in() {
    let (_, neck, door, _) = turning();
    assert_eq!(door.answer(&req("GET", "/commands")).status, 401);
    assert_eq!(door.answer(&req("GET", "/events")).status, 401);
    assert_eq!(send(&door, "Bearer wrong", &envelope("a", "left")).status, 401);
    assert!(neck.moved().is_empty());
}

#[test]
fn the_catalogue_names_a_skill_and_its_schema_says_what_it_takes() {
    let (_, _, door, token) = turning();
    let c = json_of(&door.answer(&query("/commands", &token)));
    let entry = &c["commands"][0];
    assert_eq!(c["commands"].as_array().unwrap().len(), 1);
    assert_eq!(entry["schema"], "look");
    assert_eq!(entry["version"], "1.0");
    assert_eq!(entry["commandType"], "Look");
    assert_eq!(entry["dataschema"], format!("{ENDPOINT}/commands/look/1.0"));
    let r = door.answer(&query("/commands/look/1.0", &token));
    assert_eq!((r.status, r.content_type), (200, "application/schema+json"));
    let s = json_of(&r);
    assert_eq!(s["commandType"], "Look");
    assert_eq!(s["produces"], json!(["Looked", "LookFailed"]));
    assert_eq!(s["properties"]["toward"]["enum"], json!(["left", "ahead", "right"]));
    assert_eq!(door.answer(&query("/commands/look/9.9", &token)).status, 404);
}

#[test]
fn a_command_moves_the_head_and_its_event_carries_the_correlation() {
    let (_, neck, door, token) = turning();
    let r = send(&door, &token, &envelope("cmd-1", "left"));
    assert_eq!(r.status, 201, "{:?}", r.body);
    let a = json_of(&r);
    assert_eq!(
        (a["id"].as_str(), a["correlationId"].as_str()),
        (Some("cmd-1"), Some("cmd-1"))
    );
    assert_eq!(neck.moved(), [Toward::Left]);
    let log = events_of(&door, &token, "correlationId=cmd-1");
    let e = &log["events"][0];
    assert_eq!(log["events"].as_array().unwrap().len(), 1);
    assert_eq!(log.get("nextCursor"), None);
    assert_eq!(e["type"], "Looked");
    assert_eq!(e["specversion"], "1.0");
    assert_eq!(e["datacontenttype"], "application/json");
    assert_eq!(e["correlationid"], "cmd-1");
    assert_eq!(e["time"], "2026-11-09T12:59:59.007Z");
    assert_eq!(e["data"]["toward"], "left");
}

#[test]
fn a_command_may_name_its_correlation_and_plain_json_is_taken_too() {
    let (_, _, door, token) = turning();
    let mut c = envelope("cmd-1", "right");
    c["correlationid"] = json!("trip-7");
    let r = send_as(
        &door,
        &token,
        "application/json; charset=utf-8",
        c.to_string().as_bytes(),
    );
    assert_eq!(r.status, 201);
    assert_eq!(json_of(&r)["correlationId"], "trip-7");
    assert_eq!(
        events_of(&door, &token, "correlationId=trip-7")["events"][0]["correlationid"],
        "trip-7"
    );
}

#[test]
fn a_look_that_fails_is_told_as_an_event_with_the_reason() {
    let (_, neck, door, token) = turning();
    *neck.fails.lock().unwrap() = true;
    assert_eq!(send(&door, &token, &envelope("cmd-1", "left")).status, 201);
    let e = &events_of(&door, &token, "")["events"][0];
    assert_eq!(e["type"], "LookFailed");
    assert_eq!(e["correlationid"], "cmd-1");
    assert_eq!(e["data"]["reason"], "no signal");
}

#[test]
fn an_envelope_that_is_wrong_is_refused_naming_the_field() {
    let (_, neck, door, token) = turning();
    let without = |field: &str| {
        let mut c = envelope("cmd-1", "left");
        c.as_object_mut().unwrap().remove(field);
        c
    };
    let changed = |field: &str, value: Value| {
        let mut c = envelope("cmd-1", "left");
        c[field] = value;
        c
    };
    for field in [
        "specversion",
        "id",
        "source",
        "type",
        "datacontenttype",
        "dataschema",
        "time",
        "data",
    ] {
        let r = send(&door, &token, &without(field));
        assert_eq!((r.status, field_of(&r).as_str()), (400, field));
    }
    for (field, value) in [
        ("specversion", json!("0.3")),
        ("id", json!("x".repeat(65))),
        ("type", json!("Fly")),
        ("dataschema", json!("https://elsewhere.example/look")),
        ("time", json!("yesterday")),
        ("correlationid", json!(7)),
        ("correlationid", json!("x".repeat(65))),
    ] {
        let r = send(&door, &token, &changed(field, value));
        assert_eq!((r.status, field_of(&r).as_str()), (400, field));
    }
    assert!(neck.moved().is_empty());
}

#[test]
fn data_that_is_wrong_is_refused_naming_the_field() {
    let (_, neck, door, token) = turning();
    let r = send(&door, &token, &envelope("cmd-1", "up"));
    assert_eq!((r.status, field_of(&r).as_str()), (400, "data.toward"));
    assert!(json_of(&r)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("left, ahead or right"));
    let mut c = envelope("cmd-1", "left");
    c["data"]["degrees"] = json!(90);
    let r = send(&door, &token, &c);
    assert_eq!((r.status, field_of(&r).as_str()), (400, "data.degrees"));
    assert!(neck.moved().is_empty());
}

#[test]
fn a_body_that_is_not_taken_says_so() {
    let (_, _, door, token) = turning();
    let r = send_as(&door, &token, "text/plain", b"{}");
    assert_eq!((r.status, field_of(&r).as_str()), (415, "Content-Type"));
    assert!(json_of(&r)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("application/cloudevents+json"));
    let r = send_as(&door, &token, "application/json", b"not json");
    assert_eq!((r.status, field_of(&r).as_str()), (400, "body"));
    let deep = "[".repeat(2000);
    let r = send_as(&door, &token, "application/json", deep.as_bytes());
    assert_eq!((r.status, field_of(&r).as_str()), (400, "body"));
    assert!(json_of(&r)["error"]["message"].as_str().unwrap().contains("nests"));
}

#[test]
fn brackets_inside_a_string_are_not_depth() {
    let (_, neck, door, token) = turning();
    let mut c = envelope("cmd-1", "left");
    c["source"] = json!("[".repeat(100));
    assert_eq!(send(&door, &token, &c).status, 201);
    assert_eq!(neck.moved(), [Toward::Left]);
}

#[test]
fn a_duplicate_is_answered_as_the_first_was_and_moves_nothing() {
    let (board, neck, door, token) = turning();
    let first = send(&door, &token, &envelope("cmd-1", "left"));
    board.wait(1);
    let again = send(&door, &token, &envelope("cmd-1", "left"));
    assert_eq!(again, first);
    assert_eq!(neck.moved(), [Toward::Left]);
    assert_eq!(events_of(&door, &token, "")["events"].as_array().unwrap().len(), 1);
}

#[test]
fn an_id_reused_with_another_payload_is_a_conflict_naming_it() {
    let (board, neck, door, token) = turning();
    assert_eq!(send(&door, &token, &envelope("cmd-1", "left")).status, 201);
    board.wait(1);
    let r = send(&door, &token, &envelope("cmd-1", "right"));
    assert_eq!(r.status, 409);
    assert_eq!(json_of(&r)["error"]["details"]["id"], "cmd-1");
    assert_eq!(neck.moved(), [Toward::Left]);
}

#[test]
fn a_correlation_belongs_to_the_sender_that_first_used_it() {
    let (board, neck, door, token) = turning();
    let other = token_of(&board, &door);
    let mut c = envelope("cmd-1", "left");
    c["correlationid"] = json!("trip-7");
    assert_eq!(send(&door, &token, &c).status, 201);
    let mut theirs = envelope("cmd-2", "right");
    theirs["correlationid"] = json!("trip-7");
    let r = send(&door, &other, &theirs);
    assert_eq!((r.status, field_of(&r).as_str()), (400, "correlationid"));
    assert_eq!(neck.moved(), [Toward::Left]);
}

#[test]
fn two_senders_of_one_id_with_no_correlation_are_one_accepted_and_one_refused() {
    let (board, _, door, token) = turning();
    let other = token_of(&board, &door);
    assert_eq!(send(&door, &token, &envelope("same", "left")).status, 201);
    let r = send(&door, &other, &envelope("same", "left"));
    assert_eq!((r.status, field_of(&r).as_str()), (400, "correlationid"));
}

#[test]
fn a_sender_reads_only_what_its_own_commands_did() {
    let (board, _, door, token) = turning();
    let other = token_of(&board, &door);
    assert_eq!(send(&door, &token, &envelope("mine", "left")).status, 201);
    assert_eq!(events_of(&door, &other, "")["events"].as_array().unwrap().len(), 0);
    assert_eq!(
        events_of(&door, &other, "correlationId=mine")["events"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn the_event_log_filters_and_pages() {
    let (board, _, door, token) = turning();
    for (id, toward) in [("a", "left"), ("b", "ahead"), ("c", "right")] {
        assert_eq!(send(&door, &token, &envelope(id, toward)).status, 201);
        board.wait(1);
    }
    let page = events_of(&door, &token, "limit=2");
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    assert_eq!(page["events"][0]["correlationid"], "a");
    let cursor = page["nextCursor"].as_str().unwrap();
    let rest = events_of(&door, &token, &format!("limit=2&after={cursor}"));
    assert_eq!(rest["events"].as_array().unwrap().len(), 1);
    assert_eq!(rest["events"][0]["correlationid"], "c");
    assert_eq!(rest.get("nextCursor"), None);
    assert_eq!(
        events_of(&door, &token, "type=Looked")["events"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        events_of(&door, &token, "type=LookFailed")["events"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        events_of(&door, &token, &format!("source={ENDPOINT}"))["events"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let time = |q: &str| events_of(&door, &token, q)["events"].as_array().unwrap().len();
    assert_eq!(time("from=2026-11-09T12:59:59Z&to=2026-11-09T13:00:00Z"), 3);
    assert_eq!(time("from=2026-11-09T13:00:00Z"), 0);
    assert_eq!(time("to=2026-11-09T12:00:00Z"), 0);
}

#[test]
fn a_parameter_that_is_wrong_is_refused_naming_it() {
    let (_, _, door, token) = turning();
    for (q, parameter) in [
        ("from=yesterday", "from"),
        ("to=2026-13-01T00:00:00Z", "to"),
        ("limit=0", "limit"),
        ("limit=many", "limit"),
        ("after=gone", "after"),
    ] {
        let r = door.answer(&query(&format!("/events?{q}"), &token));
        assert_eq!(r.status, 400, "{q}");
        assert_eq!(json_of(&r)["error"]["details"]["parameter"], parameter);
    }
}

#[test]
fn a_command_past_the_pace_is_told_when_to_send_again_and_may_be_retried() {
    let (board, neck, door, token) = turning();
    assert_eq!(send(&door, &token, &envelope("a", "left")).status, 201);
    let r = send(&door, &token, &envelope("b", "right"));
    assert_eq!(r.status, 429);
    assert!(r.headers.iter().any(|(h, v)| *h == "Retry-After" && v == "1"));
    assert_eq!(neck.moved(), [Toward::Left]);
    board.wait(1);
    assert_eq!(send(&door, &token, &envelope("b", "right")).status, 201);
}

#[test]
fn the_pace_is_each_senders_own() {
    let (board, _, door, token) = turning();
    let other = token_of(&board, &door);
    assert_eq!(send(&door, &token, &envelope("a", "left")).status, 201);
    assert_eq!(send(&door, &other, &envelope("b", "right")).status, 201);
}

#[test]
fn a_retry_is_answered_even_inside_the_pace() {
    let (_, neck, door, token) = turning();
    let first = send(&door, &token, &envelope("a", "left"));
    assert_eq!(send(&door, &token, &envelope("a", "left")), first);
    assert_eq!(neck.moved().len(), 1);
}

#[test]
fn before_the_robot_knows_the_time_a_command_is_refused_and_its_id_stays_free() {
    let (board, neck, door, token) = turning();
    *board.no_time.lock().unwrap() = true;
    let r = send(&door, &token, &envelope("a", "left"));
    assert_eq!(r.status, 400);
    assert_eq!(json_of(&r)["error"]["code"], "TIME_UNKNOWN");
    assert!(neck.moved().is_empty());
    *board.no_time.lock().unwrap() = false;
    assert_eq!(send(&door, &token, &envelope("a", "right")).status, 201);
    assert_eq!(neck.moved(), [Toward::Right]);
}

#[test]
fn the_button_stops_the_head_until_it_is_pressed_again() {
    let (board, neck, door, token) = turning();
    assert_eq!(door.stop_or_release(), Some(true));
    assert_eq!(neck.rests.load(Ordering::SeqCst), 1);
    let r = send(&door, &token, &envelope("a", "left"));
    assert_eq!(r.status, 400);
    assert_eq!(json_of(&r)["error"]["code"], "STOPPED");
    assert!(neck.moved().is_empty());
    assert_eq!(door.stop_or_release(), Some(false));
    board.wait(1);
    assert_eq!(send(&door, &token, &envelope("a", "left")).status, 201);
    assert_eq!(neck.moved(), [Toward::Left]);
}

#[test]
fn a_robot_with_no_head_has_nothing_to_stop() {
    assert_eq!(Board::default().door().stop_or_release(), None);
    assert_eq!(Board::default().door().released_after(100), Pressed::NoHead);
}

#[test]
fn a_short_press_stops_the_head_and_approves_no_sign_in_armed_from_the_network() {
    let (board, neck, door, _) = turning();
    let reg = register(&door, "client_id=best&connection=stranger");
    let (user, device) = (reg["user_code"].as_str().unwrap(), reg["device_code"].as_str().unwrap());
    approve(&door, user);

    assert_eq!(door.released_after(100), Pressed::Stopped);
    assert_eq!(neck.rests.load(Ordering::SeqCst), 1);
    assert_eq!(door.released_after(APPROVE_AFTER_MS - 1), Pressed::LetGo);
    board.wait(5);
    assert_ne!(poll(&door, device).status, 200, "a short press approved a code");

    assert_eq!(
        door.released_after(APPROVE_AFTER_MS),
        Pressed::Approved(user.to_string())
    );
    assert_eq!(neck.rests.load(Ordering::SeqCst), 1, "a hold is not the stop");
    board.wait(5);
    assert_eq!(poll(&door, device).status, 200);
}

#[test]
fn a_hold_with_no_sign_in_armed_does_nothing() {
    let (_, neck, door, _) = turning();
    assert!(!door.button_down());
    assert_eq!(door.released_after(APPROVE_AFTER_MS), Pressed::NothingApproved);
    assert_eq!(neck.rests.load(Ordering::SeqCst), 0);
}

#[test]
fn the_head_lets_go_when_the_button_goes_down_however_long_it_is_held() {
    for held in [100, APPROVE_AFTER_MS, 12_000] {
        let (board, neck, door, token) = turning();
        assert_eq!(send(&door, &token, &envelope("a", "left")).status, 201);

        assert!(door.button_down());
        assert_eq!(
            neck.rests.load(Ordering::SeqCst),
            1,
            "it lets go before the button comes up"
        );
        board.wait(1);
        let r = send(&door, &token, &envelope("b", "left"));
        assert_eq!(json_of(&r)["error"]["code"], "STOPPED", "held {held}");

        let expected = if held < APPROVE_AFTER_MS {
            Pressed::Stopped
        } else {
            Pressed::NothingApproved
        };
        assert_eq!(door.released_after(held), expected);
        let r = send(&door, &token, &envelope("b", "left"));
        assert_eq!(
            json_of(&r)["error"]["code"],
            "STOPPED",
            "letting go did not let the head go on"
        );

        assert!(!door.button_down());
        assert_eq!(door.released_after(100), Pressed::LetGo);
        assert_eq!(neck.rests.load(Ordering::SeqCst), 1);
        assert_eq!(send(&door, &token, &envelope("b", "left")).status, 201);
    }
}

#[test]
fn a_hold_that_approves_still_stops_a_head_holding_a_position() {
    let (_, neck, door, token) = turning();
    assert_eq!(send(&door, &token, &envelope("a", "left")).status, 201);
    let reg = register(&door, "client_id=best&connection=stranger");
    let user = reg["user_code"].as_str().unwrap();
    approve(&door, user);

    assert!(door.button_down());
    assert_eq!(
        door.released_after(APPROVE_AFTER_MS),
        Pressed::Approved(user.to_string())
    );
    assert_eq!(neck.rests.load(Ordering::SeqCst), 1);
    let r = send(&door, &token, &envelope("b", "left"));
    assert_eq!(json_of(&r)["error"]["code"], "STOPPED");
}

#[test]
fn a_press_after_the_hold_that_forgot_is_counted_afresh() {
    let (board, _, door, token) = turning();
    assert_eq!(send(&door, &token, &envelope("a", "left")).status, 201);
    assert!(door.button_down());
    // A hold that reached the forget is never reported as let go of.
    assert!(!door.button_down());
    assert_eq!(door.released_after(100), Pressed::LetGo);
    board.wait(1);
    assert_eq!(send(&door, &token, &envelope("b", "left")).status, 201);
}

#[test]
fn an_id_too_long_is_measured_and_told_in_bytes() {
    let (_, neck, door, token) = turning();
    let r = send(&door, &token, &envelope(&"\u{20ac}".repeat(22), "left"));
    assert_eq!((r.status, field_of(&r).as_str()), (400, "id"));
    assert!(json_of(&r)["error"]["message"].as_str().unwrap().contains("64 bytes"));
    assert!(neck.moved().is_empty());
}

#[test]
fn what_was_sent_is_remembered_across_a_restart() {
    let (board, neck, door, token) = turning();
    let first = send(&door, &token, &envelope("cmd-1", "left"));
    assert_eq!(first.status, 201);
    drop(door);
    *board.neck.lock().unwrap() = Some(Box::new(neck.clone()));
    let restarted = board.door();
    assert_eq!(send(&restarted, &token, &envelope("cmd-1", "left")), first);
    assert_eq!(neck.moved().len(), 1);
    assert_eq!(events_of(&restarted, &token, "")["events"][0]["correlationid"], "cmd-1");
}

#[test]
fn a_flash_that_will_not_take_it_makes_the_answer_a_202_every_time() {
    let (board, neck, door, token) = turning();
    *board.log.fails.lock().unwrap() = true;
    let first = send(&door, &token, &envelope("cmd-1", "left"));
    assert_eq!(first.status, 202);
    assert_eq!(send(&door, &token, &envelope("cmd-1", "left")), first);
    assert_eq!(neck.moved().len(), 1);
    assert_eq!(events_of(&door, &token, "")["events"].as_array().unwrap().len(), 1);
}

#[test]
fn the_robot_remembers_its_latest_commands_alone_and_keeps_them_all_in_flash() {
    let (board, neck, door, token) = turning();
    for i in 0..commands::REMEMBERED + 2 {
        assert_eq!(send(&door, &token, &envelope(&format!("c{i}"), "left")).status, 201);
        board.wait(1);
    }
    let log = events_of(&door, &token, "limit=99");
    let events = log["events"].as_array().unwrap();
    assert_eq!(events.len(), commands::REMEMBERED);
    assert_eq!(events[0]["correlationid"], "c2");
    let kept: Vec<Value> = serde_json::from_str(&board.log.read().unwrap()).unwrap();
    assert_eq!(kept.len(), commands::REMEMBERED);
    assert_eq!(
        kept[commands::REMEMBERED - 1]["i"],
        format!("c{}", commands::REMEMBERED + 1)
    );
    // Past the window, an id is free again.
    assert_eq!(send(&door, &token, &envelope("c0", "right")).status, 201);
    assert_eq!(neck.moved().last(), Some(&Toward::Right));
}

#[test]
fn a_flash_that_holds_rubbish_starts_an_empty_log() {
    let (board, neck) = (Board::default(), Neck::default());
    *board.neck.lock().unwrap() = Some(Box::new(neck));
    *board.log.kept.lock().unwrap() = Some("{not json".into());
    let door = board.door();
    let token = token_of(&board, &door);
    assert_eq!(events_of(&door, &token, "")["events"].as_array().unwrap().len(), 0);
}

#[test]
fn rfc_3339_is_read_with_its_offset_and_its_fraction() {
    use crate::camera::{parse_rfc3339, rfc3339};
    let at = 1_794_229_199_007;
    assert_eq!(parse_rfc3339(&rfc3339(at)), Some(at));
    assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_rfc3339("1970-01-01T01:00:00+01:00"), Some(0));
    assert_eq!(parse_rfc3339("1970-01-01T00:00:01.5z"), Some(1500));
    assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
    for wrong in [
        "",
        "yesterday",
        "2026-11-09",
        "2026-11-09 12:00:00Z",
        "2026-11-09T12:00:00",
        "2026-02-30T00:00:00Z",
        "2025-02-29T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-00-01T00:00:00Z",
        "2026-11-09T24:00:00Z",
        "2026-11-09T12:00:00.Z",
        "2026-11-09T12:00:00+1:00",
        "+026-11-09T12:00:00Z",
        "1969-12-31T23:59:59Z",
    ] {
        assert_eq!(parse_rfc3339(wrong), None, "{wrong}");
    }
}
