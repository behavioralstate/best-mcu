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
}

impl Clock for Board {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn utc_ms(&self) -> Option<u64> {
        Some(1_794_229_199_007)
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
