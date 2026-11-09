//! The robot's BOOT button: going down stops a head that holds a position, a short press stops the head or lets it go,
//! a hold of a few seconds approves a sign-in, and ten seconds forgets every credential. QEMU has no button: the firmware built for it leaves this out.

use std::sync::Arc;
use std::time::Duration;

use best_mcu_door::{Door, Pressed};
use esp_idf_svc::hal::gpio::{Input, InputPin, PinDriver, Pull};

/// How long the button is held to forget every credential.
const FORGET_AFTER: Duration = Duration::from_secs(10);
const SAMPLE: Duration = Duration::from_millis(20);

/// Watches the BOOT button, reading the pin's level, low while pressed, every twenty milliseconds. What each press does
/// is the door's to decide.
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
    let (mut down, mut forgot) = (false, false);
    loop {
        std::thread::sleep(SAMPLE);
        if button.is_low() {
            held += SAMPLE;
            if held >= 2 * SAMPLE && !down {
                down = true;
                if door.button_down() {
                    log::info!("the button went down: the head let go");
                }
            }
            if held >= FORGET_AFTER && !forgot {
                forgot = true;
                match door.forget_credentials() {
                    Ok(()) => log::info!("the button was held: every credential is forgotten"),
                    Err(e) => log::warn!("the button was held: {e}"),
                }
            }
            continue;
        }
        // Released: a press past a bounce, and not the hold that already forgot.
        if down && !forgot {
            match door.released_after(held.as_millis() as u64) {
                Pressed::Approved(code) => log::info!("the button approved the sign-in {code}"),
                Pressed::NothingApproved => log::info!("the button was held; it approved no sign-in"),
                Pressed::Stopped => log::info!("the button stopped the head: it refuses commands until pressed again"),
                Pressed::LetGo => log::info!("the button released the head"),
                Pressed::NoHead => log::info!("the button was pressed; this robot has no head"),
            }
        }
        held = Duration::ZERO;
        (down, forgot) = (false, false);
    }
}
