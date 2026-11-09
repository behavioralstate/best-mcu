//! The robot's head: a hobby servo on one pin, driven by the chip's LED controller, which holds the pulses without the
//! CPU. QEMU has no servo, so this module is left out of its build. The camera's driver holds timer 0 and channel 0.

use std::sync::Mutex;

use best_mcu_door::{Head, Toward};
use esp_idf_svc::hal::gpio::OutputPin;
use esp_idf_svc::hal::ledc::config::TimerConfig;
use esp_idf_svc::hal::ledc::{LedcDriver, LedcTimerDriver, Resolution, CHANNEL1, TIMER1};
use esp_idf_svc::hal::units::FromValueType;

/// A hobby servo reads one pulse every 20 ms.
const PERIOD_US: u32 = 20_000;

/// The pulse for each way of looking, in the range every hobby servo takes. How far it turns depends on the servo.
fn pulse_us(toward: Toward) -> u32 {
    match toward {
        Toward::Left => 2000,
        Toward::Ahead => 1500,
        Toward::Right => 1000,
    }
}

struct Servo(Mutex<LedcDriver<'static>>);

impl Head for Servo {
    fn look(&self, toward: Toward) -> Result<(), String> {
        let mut output = self.0.lock().map_err(|_| "the servo's lock is poisoned".to_string())?;
        let duty = pulse_us(toward) * (output.get_max_duty() + 1) / PERIOD_US;
        output.set_duty(duty).map_err(|e| e.to_string())
    }

    fn rest(&self) {
        if let Ok(mut output) = self.0.lock() {
            if let Err(e) = output.set_duty(0) {
                log::error!("the servo would not rest: {e}");
            }
        }
    }
}

/// The head, on `pin`. It sends no pulses until it is told to look, so the servo does not jump at start.
pub fn start(
    timer: TIMER1<'static>,
    channel: CHANNEL1<'static>,
    pin: impl OutputPin + 'static,
) -> anyhow::Result<Box<dyn Head>> {
    let timer = LedcTimerDriver::new(
        timer,
        &TimerConfig::default().frequency(50.Hz()).resolution(Resolution::Bits14),
    )?;
    let output = LedcDriver::new(channel, timer, pin)?;
    Ok(Box::new(Servo(Mutex::new(output))))
}
