# best-mcu

A [Behavioral State Protocol](https://behavioralstate.io) door served from a microcontroller: a robot made of a
Wi-Fi board, servos and sensors, with no Linux computer and no ROS, whose skills an agent reads and drives from the
manifest alone. The question it answers is whether everything the spec asks of a door fits on such a board. The plan
is [behavioralstate/spec#275](https://github.com/behavioralstate/spec/issues/275), on the
[Behavioralstate board](https://github.com/orgs/Amarcode-Studios/projects/4).

The board is an ESP32-S3 with 16 MB of flash and 8 MB of PSRAM (N16R8). The firmware is Rust with the standard
library on ESP-IDF.

**Where it stands:** the door serves its manifest over HTTPS, with an ETag and a cache life, and refuses what it does
not serve in the BEST error format. It runs on the board, a Freenove ESP32-S3-WROOM-1 on Wi-Fi, as it does in
Espressif's QEMU: best-validate 0.4.3 finds it conformant on both (discovery only, since it offers no capability yet).

ESP-IDF's HTTP server gives every answer it sends without a body a length of 0, where HTTP allows none or the length
GET's answer has. So a client sending `If-None-Match` is sent the manifest whole, never a 304, with the ETag still on
it so the client can tell it changed; and `HEAD`, answered as GET is without the body, says `Content-Length: 0`. A
client learns the manifest's length by reading it with GET.

What it costs:

| | QEMU | The board |
|---|---|---|
| Firmware image (ESP-IDF, lwIP, mbedTLS, the HTTPS server, the door; the board's with Wi-Fi) | 776 KB, 4.7% of the flash | 1.1 MB, 6.8% |
| Internal RAM free at start | 364 KB | 309 KB |
| … with the network up and the door served | 324 KB | 240 KB |
| … least, after TLS handshakes | 259 KB | 151 KB |
| PSRAM used | almost none | almost none |
| The manifest read over a new connection, from a machine on the same network | | 0.7 to 1 s, the TLS handshake nearly all of it |

Timings are the board's alone: QEMU's say nothing of them.

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
paths help: ESP-IDF's build nests deep. `ESP_IDF_TOOLS_INSTALL_DIR=custom:D:/esp`, a `CARGO_TARGET_DIR` such as
`D:/t/mcu` for the board's build and `BEST_MCU_QEMU_TARGET=D:/t/mcuq` for QEMU's keep them short.

## On the board

The board is flashed and read through its UART USB-C port (a CH343, which Windows names `USB-Enhanced-SERIAL CH343`).
It joins a 2.4 GHz network only. It is reached by the address the network gives it, which its serial log shows
(`Wi-Fi up: IpInfo { ip: ... }`); a reservation on the router keeps the address. The door is built for that address,
with a certificate for it:

```bash
scripts/dev-cert.sh 192.168.1.104
```

then, from `firmware/` with the board on USB:

```bash
BEST_MCU_NAME=192.168.1.104 BEST_MCU_ENDPOINT=https://192.168.1.104 BEST_MCU_WIFI_SSID=... BEST_MCU_WIFI_PASSWORD=... cargo run --release
```

It is flashed and its serial log follows. The name and the endpoint are what the manifest says the door is reached
at (`localhost` and `https://localhost` when unset); the network's name and password are read when the firmware is
built, and never committed. The first build, before the address is known, needs only the network's. best-validate is
run against it as the gate runs it against QEMU:

```bash
NODE_TLS_REJECT_UNAUTHORIZED=0 npx @behavioralstate/best-validate@0.4.3 https://192.168.1.104
```
