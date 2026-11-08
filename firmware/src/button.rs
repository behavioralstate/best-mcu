//! The robot's BOOT button, which approves a sign-in and, held, forgets every credential. QEMU has no button: the
//! firmware built for it leaves this out.

use std::sync::Arc;
use std::time::Duration;

use best_mcu_door::Door;
use esp_idf_svc::hal::gpio::{Input, InputPin, PinDriver, Pull};

/// How long the button is held to forget every credential.
const FORGET_AFTER: Duration = Duration::from_secs(10);
const SAMPLE: Duration = Duration::from_millis(20);

/// Watches the BOOT button: a press approves the code armed on the sign-in page; held ten seconds, every credential is
/// forgotten. It reads the pin's level, low while pressed, every twenty milliseconds.
pub fn watch(pin: impl InputPin + 'static, door: Arc<Door>) -> anyhow::Result<()> {
    let button = PinDriver::input(pin, Pull::Up)?;
    std::thread::Builder::new()
        .name("button".into())
        .stack_size(6144)
        .spawn(move || follow(button, &door))?;
    Ok(())
}

fn follow(button: PinDriver<'static, Input>, door: &Door) {
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
                    Err(e) => log::warn!("the button was held: {e}"),
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
