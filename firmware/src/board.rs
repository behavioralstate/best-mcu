//! The board's side of the door's ports: its clock, its random generator, the flash that keeps credentials, its
//! readings of itself, and the BOOT button that approves a sign-in. Each is thin: the door decides what they mean.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use best_mcu_door::{Clock, Door, Entropy, Readings, Status, Vault};
use esp_idf_svc::hal::gpio::{Input, InputPin, PinDriver, Pull};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::sys;

pub struct Board;

impl Clock for Board {
    fn now_ms(&self) -> u64 {
        (unsafe { sys::esp_timer_get_time() } / 1000) as u64
    }
}

impl Entropy for Board {
    /// The hardware generator, true random while the radio is on (it is, for the network).
    fn fill(&self, bytes: &mut [u8]) {
        unsafe { sys::esp_fill_random(bytes.as_mut_ptr().cast(), bytes.len()) }
    }
}

impl Readings for Board {
    fn read(&self) -> Status {
        let (internal_free, internal_least, psram_free) = memory();
        Status {
            uptime_ms: self.now_ms(),
            internal_free,
            internal_least,
            psram_free,
            wifi_rssi: crate::network::rssi(),
        }
    }
}

/// Internal RAM free and the least it has been since start, and PSRAM free.
pub fn memory() -> (u32, u32, u32) {
    unsafe {
        (
            sys::heap_caps_get_free_size(sys::MALLOC_CAP_INTERNAL) as u32,
            sys::heap_caps_get_minimum_free_size(sys::MALLOC_CAP_INTERNAL) as u32,
            sys::heap_caps_get_free_size(sys::MALLOC_CAP_SPIRAM) as u32,
        )
    }
}

/// The durable credentials' hashes, in the flash's key-value store.
pub struct Flash(Mutex<EspDefaultNvs>);

const KEY: &str = "credentials";

impl Flash {
    pub fn open(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        Ok(Self(Mutex::new(EspDefaultNvs::new(partition, "best-door", true)?)))
    }
}

impl Vault for Flash {
    fn read(&self) -> Option<String> {
        let nvs = self.0.lock().ok()?;
        let len = nvs.str_len(KEY).ok()??;
        let mut buf = vec![0u8; len];
        nvs.get_str(KEY, &mut buf).ok()?.map(str::to_string)
    }

    fn write(&self, kept: &str) -> Result<(), String> {
        let nvs = self.0.lock().map_err(|_| "the flash's lock is poisoned".to_string())?;
        nvs.set_str(KEY, kept).map_err(|e| e.to_string())
    }
}

/// How long the button is held to forget every credential.
const FORGET_AFTER: Duration = Duration::from_secs(10);
const SAMPLE: Duration = Duration::from_millis(20);

/// Watches the BOOT button: a press approves the code armed on the sign-in page; held ten seconds, every credential is
/// forgotten. It reads the pin's level, low while pressed, every twenty milliseconds.
pub fn watch_button(pin: impl InputPin + 'static, door: Arc<Door>) -> anyhow::Result<()> {
    let button = PinDriver::input(pin, Pull::Up)?;
    std::thread::Builder::new()
        .name("button".into())
        .stack_size(6144)
        .spawn(move || watch(button, &door))?;
    Ok(())
}

fn watch(button: PinDriver<'static, Input>, door: &Door) {
    let mut held = Duration::ZERO;
    let mut forgot = false;
    loop {
        std::thread::sleep(SAMPLE);
        if button.is_low() {
            held += SAMPLE;
            if held >= FORGET_AFTER && !forgot {
                forgot = true;
                match door.forget_credentials() {
                    Ok(()) => log::info!("the button was held: every credential is forgotten"),
                    Err(e) => log::warn!("the button was held, but the credentials could not be forgotten: {e}"),
                }
            }
            continue;
        }
        // Released: a press shorter than the hold, past a bounce, approves.
        if held >= 2 * SAMPLE && !forgot {
            match door.press() {
                Some(code) => log::info!("the button approved the sign-in {code}"),
                None => log::info!("the button was pressed; no sign-in was armed"),
            }
        }
        held = Duration::ZERO;
        forgot = false;
    }
}
