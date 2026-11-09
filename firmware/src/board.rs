//! The board's side of the door's ports: its clock, its random generator, the flash that keeps credentials, and its
//! readings of itself. Each is thin: the door decides what they mean.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use best_mcu_door::{Clock, Entropy, Readings, Status, Vault};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::sys;

pub struct Board;

const YEAR_2025_MS: u64 = 1_735_689_600_000;

impl Clock for Board {
    fn now_ms(&self) -> u64 {
        (unsafe { sys::esp_timer_get_time() } / 1000) as u64
    }

    /// The system clock, once the network's time has set it: before that it reads a date long past.
    fn utc_ms(&self) -> Option<u64> {
        let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        Some(since_epoch.as_millis() as u64).filter(|ms| *ms >= YEAR_2025_MS)
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
