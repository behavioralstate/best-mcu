# best-mcu

A [Behavioral State Protocol](https://behavioralstate.io) door served from a microcontroller: a robot made of a
Wi-Fi board, servos and sensors, with no Linux computer and no ROS, whose skills an agent reads and drives from the
manifest alone. The question it answers is whether everything the spec asks of a door fits on such a board. The plan
is [behavioralstate/spec#275](https://github.com/behavioralstate/spec/issues/275), on the
[Behavioralstate board](https://github.com/orgs/Amarcode-Studios/projects/4).

The board is an ESP32-S3 with 16 MB of flash and 8 MB of PSRAM (N16R8). The firmware is Rust with the standard
library on ESP-IDF.

**Where it stands:** the door serves its manifest over HTTPS, with an ETag and a cache life, and refuses what it does
not serve in the BEST error format. It is proven in Espressif's QEMU, not yet on the board: best-validate 0.4.3 finds
it conformant (discovery only, since it offers no capability yet). What it costs there:

| | |
|---|---|
| Firmware image for QEMU (ESP-IDF, lwIP, mbedTLS, the HTTPS server, the door) | 781 KB, 4.8% of the flash |
| … for the board, with Wi-Fi (built, not yet run) | 1.1 MB, 6.8% of the flash |
| Internal RAM free at start | 364 KB |
| … with the network up and the door served | 325 KB |
| … least, after TLS handshakes | 261 KB |
| PSRAM used | almost none |

Timings in QEMU say nothing of the board's, which are measured on it.

## Layout

- `crates/door`: the door, plain Rust: what each request is answered with. It knows nothing of the board, and its
  tests run on any machine: `cargo test`.
- `firmware`: the board's side, built with the esp toolchain: the network (Wi-Fi, or QEMU's Ethernet with the `qemu`
  feature), ESP-IDF's HTTPS server carrying requests to the door, and the memory report on the serial log.

## Try it in QEMU

Once: the esp toolchain (`cargo install espup espflash ldproxy`, then `espup install --targets esp32s3 --std`).
Then, in Git Bash:

```bash
scripts/qemu.sh
```

It builds the firmware for the emulator and runs it; the first run fetches ESP-IDF and its QEMU into
`firmware/.embuild`, and makes a self-signed certificate for `localhost` (`scripts/dev-cert.sh`). The serial log shows
`the door is served at https://localhost:8443/.well-known/best`. The certificate is self-signed, so a client is told
to accept it for this address alone (`curl -k`, or `NODE_TLS_REJECT_UNAUTHORIZED=0` for best-validate).

`scripts/gate.sh all` is the proof a change is held to before it is pushed ([SDLC.md](SDLC.md)): fmt, clippy and the
door's tests, the board's build, and best-validate against the door served in QEMU.

On Windows, cargo runs inside the MSVC Build Tools environment (the host's build scripts link with it), and short
paths help: ESP-IDF's build nests deep. `ESP_IDF_TOOLS_INSTALL_DIR=custom:D:/esp` and a `CARGO_TARGET_DIR` such as
`D:/t/mcu` keep both short.

## On the board

```bash
BEST_MCU_WIFI_SSID=... BEST_MCU_WIFI_PASSWORD=... cargo run --release
```

from `firmware/`, with the board on USB: it is flashed and its serial log follows. The network's name and password
are read when the firmware is built, and never committed.
