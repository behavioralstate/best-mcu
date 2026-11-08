//! The firmware: brings the network up, then serves the door over HTTPS. The door decides every answer
//! (`best-mcu-door`); this file only carries requests to it and answers back, and reports the memory it takes.

use std::time::Duration;

use best_mcu_door::{Door, Request};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write;
use esp_idf_svc::sys;
use esp_idf_svc::tls::X509;

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

/// The server's certificate and key, PEM with a closing NUL, made by `scripts/dev-cert.sh` and never committed.
const CERT: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/certs/cert.pem"));
const KEY: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/certs/key.pem"));

fn main() -> anyhow::Result<()> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    report("at start");

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let _network = network::up(peripherals, sysloop)?;
    report("network up");

    let door = Door::new(NAME, ENDPOINT);
    let mut server = EspHttpServer::new(&Configuration {
        uri_match_wildcard: true,
        server_certificate: Some(X509::pem_until_nul(CERT)),
        private_key: Some(X509::pem_until_nul(KEY)),
        ..Default::default()
    })?;
    let door = std::sync::Arc::new(door);
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
        server.fn_handler::<anyhow::Error, _>("/*", m, move |req| {
            let path = req.uri().split('?').next().unwrap_or("/").to_string();
            let answer = door.answer(&Request { method, path: &path });
            let mut headers: Vec<(&str, &str)> = vec![("Content-Type", "application/json")];
            headers.extend(answer.headers.iter().map(|(h, v)| (*h, v.as_str())));
            let mut resp = req.into_response(answer.status, None, &headers)?;
            if let Some(body) = answer.body.filter(|_| method != "HEAD") {
                resp.write_all(body.as_bytes())?;
            }
            Ok(())
        })?;
    }
    report("door served");
    log::info!("the door is served at {ENDPOINT}/.well-known/best");

    loop {
        std::thread::sleep(Duration::from_secs(10));
        report("serving");
    }
}

/// Logs the free heap, internal and in PSRAM, and the least it has been since start: the cost of what ran.
fn report(when: &str) {
    let (internal, internal_least, psram) = unsafe {
        (
            sys::heap_caps_get_free_size(sys::MALLOC_CAP_INTERNAL),
            sys::heap_caps_get_minimum_free_size(sys::MALLOC_CAP_INTERNAL),
            sys::heap_caps_get_free_size(sys::MALLOC_CAP_SPIRAM),
        )
    };
    log::info!("memory {when}: internal free {internal} (least {internal_least}), PSRAM free {psram}");
}
