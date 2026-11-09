//! The board's camera: Espressif's `esp32-camera` driver behind the door's port, with its frames in PSRAM, and the
//! server of the live frames. The board's sensor, a GC0308, has no JPEG of its own, so each frame is taken raw and
//! encoded here. QEMU emulates no camera, so this module is left out of its build.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use best_mcu_door::{Camera, Door, Frame, Request, FRAMES_TYPE, STREAM_PORT};
use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write;
use esp_idf_svc::sys;
use esp_idf_svc::sys::camera as driver;
use esp_idf_svc::tls::X509;

/// The quality frames are encoded at, from 1 to 100.
const QUALITY: u8 = 60;

/// The sensor on the board, wired as Freenove's ESP32-S3-WROOM camera board wires it. Takes hold of it, or says why
/// not: the door then serves without a camera.
fn start() -> Result<(), sys::EspError> {
    let config = driver::camera_config_t {
        pin_pwdn: -1,
        pin_reset: -1,
        pin_xclk: 15,
        __bindgen_anon_1: driver::camera_config_t__bindgen_ty_1 { pin_sccb_sda: 4 },
        __bindgen_anon_2: driver::camera_config_t__bindgen_ty_2 { pin_sccb_scl: 5 },
        pin_d7: 16,
        pin_d6: 17,
        pin_d5: 18,
        pin_d4: 12,
        pin_d3: 10,
        pin_d2: 8,
        pin_d1: 9,
        pin_d0: 11,
        pin_vsync: 6,
        pin_href: 7,
        pin_pclk: 13,
        // Not the usual 20 MHz: its harmonic at 2460 MHz sits in Wi-Fi channel 11 and cost the board half its packets.
        // 24 MHz puts its harmonics at 2448 and 2472 MHz, outside the channel.
        xclk_freq_hz: 24_000_000,
        ledc_timer: driver::ledc_timer_t_LEDC_TIMER_0,
        ledc_channel: driver::ledc_channel_t_LEDC_CHANNEL_0,
        pixel_format: driver::pixformat_t_PIXFORMAT_YUV422,
        frame_size: driver::framesize_t_FRAMESIZE_VGA,
        jpeg_quality: 0,
        jpeg_buffer_size: 0,
        fb_count: 2,
        fb_location: driver::camera_fb_location_t_CAMERA_FB_IN_PSRAM,
        grab_mode: driver::camera_grab_mode_t_CAMERA_GRAB_LATEST,
        sccb_i2c_port: -1,
    };
    sys::esp!(unsafe { driver::esp_camera_init(&config) })
}

/// Handed to the encoder, which gives the picture a piece at a time and ends with no piece at all.
unsafe extern "C" fn collect(jpeg: *mut c_void, _at: usize, piece: *const c_void, len: usize) -> usize {
    if !piece.is_null() {
        let piece = unsafe { std::slice::from_raw_parts(piece as *const u8, len) };
        unsafe { &mut *(jpeg as *mut Vec<u8>) }.extend_from_slice(piece);
    }
    len
}

/// The driver is taken from by one caller at a time: the snapshot's server and the live frames' have a task each.
struct OnBoard(Mutex<()>);

impl Camera for OnBoard {
    fn take(&self) -> Result<Frame, String> {
        let _one_at_a_time = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let fb = NonNull::new(unsafe { driver::esp_camera_fb_get() }).ok_or("no frame in time")?;
        let mut jpeg = Vec::new();
        let sink = &mut jpeg as *mut Vec<u8> as *mut c_void;
        let encoded = unsafe { driver::frame2jpg_cb(fb.as_ptr(), QUALITY, Some(collect), sink) };
        let (width, height) = unsafe { (fb.as_ref().width as u32, fb.as_ref().height as u32) };
        unsafe { driver::esp_camera_fb_return(fb.as_ptr()) };
        if !encoded {
            return Err("the frame could not be encoded".to_string());
        }
        Ok(Frame { jpeg, width, height })
    }
}

/// The camera, when it starts.
pub fn start_camera() -> Option<Box<dyn Camera>> {
    match start() {
        Ok(()) => Some(Box::new(OnBoard(Mutex::new(())))),
        Err(e) => {
            log::error!("the camera did not start ({e}): the door serves without it");
            None
        }
    }
}

/// The live frames' own server: one viewer at a time, on its own task, so the door's other answers never wait for it.
/// Only `GET /camera/stream` is served here; anything else is refused by ESP-IDF itself, in plain text.
pub fn serve_frames(door: Arc<Door>) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        https_port: STREAM_PORT,
        ctrl_port: 32769,
        max_open_sockets: 1,
        stack_size: crate::SERVER_STACK,
        server_certificate: Some(X509::pem_until_nul(crate::CERT)),
        private_key: Some(X509::pem_until_nul(crate::KEY)),
        ..Default::default()
    })?;
    server.fn_handler::<anyhow::Error, _>("/camera/stream", Method::Get, move |req| {
        let uri = req.uri().to_string();
        let (path, query) = uri.split_once('?').unwrap_or((&uri, ""));
        let authorization = req.header("Authorization").map(str::to_string);
        let admitted = door.admit_watcher(&Request {
            method: "GET",
            path,
            query,
            authorization: authorization.as_deref(),
            content_type: None,
            body: &[],
        });
        if let Err(refused) = admitted {
            let mut headers: Vec<(&str, &str)> = vec![("Content-Type", refused.content_type)];
            headers.extend(refused.headers.iter().map(|(h, v)| (*h, v.as_str())));
            let mut resp = req.into_response(refused.status, None, &headers)?;
            resp.write_all(refused.body.unwrap_or_default().as_bytes())?;
            return Ok(());
        }
        let mut resp = req.into_response(
            200,
            None,
            &[("Content-Type", FRAMES_TYPE), ("Cache-Control", "no-store")],
        )?;
        // Until the viewer goes, or the camera stops giving frames.
        while let Ok(part) = door.next_part() {
            if resp.write_all(&part).is_err() {
                break;
            }
        }
        Ok(())
    })?;
    Ok(server)
}
