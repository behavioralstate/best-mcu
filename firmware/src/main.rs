//! The firmware: brings the network up, then serves the door over HTTPS. The door decides every answer
//! (`best-mcu-door`); this file only carries requests to it and answers back, gives it the board's ports
//! ([`board`]), and reports the memory it takes.

use std::sync::Arc;
use std::time::Duration;

use best_mcu_door::{Door, Ports, Request};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::{Read, Write};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys;
use esp_idf_svc::tls::X509;

mod board;
#[cfg(not(feature = "qemu"))]
mod button;
#[cfg(not(feature = "qemu"))]
mod camera;
mod network;

/// The name the door is known by and the address it is served at, set when the firmware is built.
const NAME: &str = match option_env!("BEST_MCU_NAME") {
    Some(n) => n,
    None => "localhost",
};
const ENDPOINT: &str = match option_env!("BEST_MCU_ENDPOINT") {
    Some(e) => e,
    None => "https://localhost",
};

/// The server's certificate and key, PEM with a closing NUL, never committed: the board's in `certs`
/// (`scripts/cluster-cert.sh`, or `scripts/dev-cert.sh`), QEMU's in `certs/qemu`, self-signed for localhost.
#[cfg(not(feature = "qemu"))]
macro_rules! certs {
    ($file:literal) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/certs/", $file)
    };
}
#[cfg(feature = "qemu")]
macro_rules! certs {
    ($file:literal) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/certs/qemu/", $file)
    };
}
const CERT: &[u8] = include_bytes!(certs!("cert.pem"));
const KEY: &[u8] = include_bytes!(certs!("key.pem"));

/// The largest body the door reads: a sign-in form is a few hundred bytes.
const BODY_LIMIT: usize = 4096;

/// The stack of a server's task: the JPEG encoder keeps about 12 KB on it, on top of what TLS needs.
const SERVER_STACK: usize = 24 * 1024;

fn main() -> anyhow::Result<()> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    report("at start");

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let _network = network::up(peripherals.modem, peripherals.mac, sysloop, nvs.clone())?;
    report("network up");

    // QEMU emulates no camera, and has no time to take from the network.
    #[cfg(feature = "qemu")]
    let camera = None;
    #[cfg(not(feature = "qemu"))]
    let camera = camera::start_camera();
    #[cfg(not(feature = "qemu"))]
    let _time = esp_idf_svc::sntp::EspSntp::new_default()?;
    let sees = camera.is_some();
    report("camera up");

    let door = Arc::new(Door::new(
        NAME,
        ENDPOINT,
        Ports {
            clock: Box::new(board::Board),
            entropy: Box::new(board::Board),
            vault: Box::new(board::Flash::open(nvs)?),
            readings: Box::new(board::Board),
            camera,
        },
    ));
    // QEMU emulates no button.
    #[cfg(not(feature = "qemu"))]
    button::watch(peripherals.pins.gpio0, door.clone())?;

    let mut server = EspHttpServer::new(&Configuration {
        stack_size: SERVER_STACK,
        uri_match_wildcard: true,
        server_certificate: Some(X509::pem_until_nul(CERT)),
        private_key: Some(X509::pem_until_nul(KEY)),
        ..Default::default()
    })?;
    // The methods HTTP defines for a resource like this one are the door's to answer, a refusal included, in the BEST
    // error format; any other (PROPFIND, say) is refused by ESP-IDF itself, in plain text.
    let methods = [
        (Method::Get, "GET"),
        (Method::Head, "HEAD"),
        (Method::Post, "POST"),
        (Method::Put, "PUT"),
        (Method::Patch, "PATCH"),
        (Method::Delete, "DELETE"),
        (Method::Options, "OPTIONS"),
    ];
    for (m, method) in methods {
        let door = door.clone();
        server.fn_handler::<anyhow::Error, _>("/*", m, move |mut req| {
            let uri = req.uri().to_string();
            let (path, query) = uri.split_once('?').unwrap_or((&uri, ""));
            let authorization = req.header("Authorization").map(str::to_string);
            let content_type = req.header("Content-Type").map(str::to_string);
            let answer = match read_body(&mut req)? {
                Some(body) => door.answer(&Request {
                    method,
                    path,
                    query,
                    authorization: authorization.as_deref(),
                    content_type: content_type.as_deref(),
                    body: &body,
                }),
                None => best_mcu_door::too_large(BODY_LIMIT),
            };
            let mut headers: Vec<(&str, &str)> = vec![("Content-Type", answer.content_type)];
            headers.extend(answer.headers.iter().map(|(h, v)| (*h, v.as_str())));
            let mut resp = req.into_response(answer.status, None, &headers)?;
            if let Some(body) = answer.body.filter(|_| method != "HEAD") {
                resp.write_all(body.as_bytes())?;
            }
            Ok(())
        })?;
    }
    #[cfg(not(feature = "qemu"))]
    let _frames = sees.then(|| camera::serve_frames(door.clone())).transpose()?;
    #[cfg(feature = "qemu")]
    let _ = sees;
    report("door served");
    log::info!("the door is served at {ENDPOINT}/.well-known/best");

    loop {
        std::thread::sleep(Duration::from_secs(10));
        report("serving");
    }
}

/// The request's body, or none when it is longer than [`BODY_LIMIT`].
fn read_body<R: Read>(req: &mut R) -> anyhow::Result<Option<Vec<u8>>>
where
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let mut body = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let n = req.read(&mut chunk)?;
        if n == 0 {
            return Ok(Some(body));
        }
        if body.len() + n > BODY_LIMIT {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..n]);
    }
}

/// Logs the free heap, internal and in PSRAM, and the least it has been since start: the cost of what ran.
fn report(when: &str) {
    let (internal, internal_least, psram) = board::memory();
    log::info!("memory {when}: internal free {internal} (least {internal_least}), PSRAM free {psram}");
}
