# best-mcu

A [Behavioral State Protocol](https://behavioralstate.io) door served from a microcontroller: a robot made of a
Wi-Fi board, servos and sensors, with no Linux computer and no ROS, whose skills an agent reads and drives from the
manifest alone. The question it answers is whether everything the spec asks of a door fits on such a board. The plan
is [behavioralstate/spec#275](https://github.com/behavioralstate/spec/issues/275), on the
[Behavioralstate board](https://github.com/orgs/Amarcode-Studios/projects/4).

The board is an ESP32-S3 with 16 MB of flash and 8 MB of PSRAM (N16R8). The firmware is Rust with the standard
library on ESP-IDF.

**Where it stands:** the door serves its manifest over HTTPS, with an ETag and a cache life, and refuses what it does
not serve in the BEST error format. An agent signs in with the device flow, approved by holding the robot's BOOT
button, and reads `get-status`: the robot's uptime, free memory and Wi-Fi signal. It sees: `get-snapshot` answers a
picture, and live frames open in a browser or OpenCV ([the camera](#the-camera)). It moves: the command `look` turns a
servo left, ahead or right, an event says how each went, and the BOOT button stops it ([the servo](#the-servo)). It runs
on the board, a Freenove ESP32-S3-WROOM-1 on Wi-Fi, as `https://robot.behavioralstate.io` with a Let's Encrypt
certificate, and in Espressif's QEMU, without a camera or a servo: best-validate finds it conformant on both, the board's
certificate verified. best-cli signs in to the board, queries it and commands it:
[acceptance/sign-in.md](acceptance/sign-in.md), [acceptance/camera.md](acceptance/camera.md),
[acceptance/servo.md](acceptance/servo.md).

ESP-IDF's HTTP server gives every answer it sends without a body a length of 0, where HTTP allows none or the length
GET's answer has. So a client sending `If-None-Match` is sent the manifest whole, never a 304, with the ETag still on
it so the client can tell it changed; and `HEAD`, answered as GET is without the body, says `Content-Length: 0`. A
client learns the manifest's length by reading it with GET.

What it costs:

| | QEMU | The board |
|---|---|---|
| Firmware image (ESP-IDF, lwIP, mbedTLS, the HTTPS server, the door; the board's with Wi-Fi, the camera and the servo) | 1.0 MB, 6.1% of the flash | 1.5 MB, 9.0% |
| Internal RAM free at start | 355 KB | 282 KB |
| … with the network up, the camera started and the door served | 297 KB | 100 KB |
| … least, after TLS handshakes | 227 KB | |
| … least, with four connections to the door and a viewer of the live frames at once | | 40 KB |
| PSRAM used | almost none | 1.2 MB by the camera, up to 170 KB more by TLS |
| The manifest read over a new connection, from a machine on the same network | | 1.0 to 1.3 s, the TLS handshake nearly all of it |
| `best query robot get-status`: the manifest, the catalogue and the query, each over a new connection | | 0.9 to 1.1 s |

The board's figures are with its Let's Encrypt chain, four certificates in 4.7 KB. The server keeps four connections
open at most, and TLS holds their buffers in PSRAM: in internal RAM a viewer and two more connections left the board
168 bytes, and it refused the next. Timings are the board's alone: QEMU's say nothing of them.

## Layout

- `crates/door`: the door, plain Rust: what each request is answered with, the sign-in and its page, the queries, and
  the commands and their events. It knows nothing of the board: it is given a clock, a random generator, vaults for the
  credentials and the commands, the board's readings and, where there are, a camera and a servo, and its tests run on
  any machine with fakes of them: `cargo test`.
- `firmware`: the board's side, built with the esp toolchain: the network (Wi-Fi, or QEMU's Ethernet with the `qemu`
  feature), ESP-IDF's HTTPS server carrying requests to the door, the flash that keeps credentials and commands, the BOOT
  button, the camera and its server, the servo, and the memory report on the serial log.

## Try it in QEMU

Once: the esp toolchain (`cargo install espup espflash ldproxy`, then `espup install --targets esp32s3 --std`).
Then, in Git Bash:

```bash
scripts/qemu.sh
```

It builds the firmware for the emulator and runs it; the first run fetches ESP-IDF and its QEMU into
`firmware/.embuild`, and makes a self-signed certificate for `localhost` in `firmware/certs/qemu`, apart from the
board's (`scripts/dev-cert.sh`). The serial log shows
`the door is served at https://localhost:8443/.well-known/best`. The certificate is self-signed, so a client is told
to accept it for this address alone (`curl -k`, or `NODE_TLS_REJECT_UNAUTHORIZED=0` for best-validate).

`scripts/gate.sh all` is the proof a change is held to before it is pushed ([SDLC.md](SDLC.md)): fmt, clippy and the
door's tests, the board's build, and best-validate against the door served in QEMU.

On Windows, cargo runs inside the MSVC Build Tools environment (the host's build scripts link with it), and short
paths help: ESP-IDF's build nests deep. `ESP_IDF_TOOLS_INSTALL_DIR=custom:D:/esp`, a `CARGO_TARGET_DIR` such as
`D:/t/mcu` for the board's build and `BEST_MCU_QEMU_TARGET=D:/t/mcuq` for QEMU's keep them short.

## On the board

The board is flashed and read through its UART USB-C port (a CH343, which Windows names `USB-Enhanced-SERIAL CH343`).
It joins a 2.4 GHz network only, whose name and password are read when the firmware is built and never committed.
The certificates are made from the repository's root and the firmware is built from `firmware/`. With the board on USB
and a certificate made (`scripts/dev-cert.sh`, for `localhost` until the board has its own), from `firmware/`:

```bash
BEST_MCU_WIFI_SSID=... BEST_MCU_WIFI_PASSWORD=... cargo run --release
```

It is flashed and its serial log follows, showing the address the network gives it (`Wi-Fi up: IpInfo { ip: ... }`);
a reservation on the router keeps it.

The board is then given its name, under a domain its owner holds, and a real certificate for it. The manifest keys its
service by the name reversed (`io.behavioralstate.robot`). The certificate is Let's Encrypt's, issued by the
cert-manager of the owner's cluster over HTTP-01: while it is issued or renewed, the name is a CNAME to the cluster's
load balancer, and a cert-manager `Certificate` for it renews it: secret `robot-tls` in namespace
`behavioralstateprotocol` (what `scripts/cluster-cert.sh` reads unless told otherwise), the `letsencrypt-prod` ClusterIssuer,
an ECDSA P-256 key with `rotationPolicy: Always`. The owner's is kept in the cluster's own configuration, not here. From
the repository's root, with kubectl on the cluster:

```bash
scripts/cluster-cert.sh
```

then from `firmware/`:

```bash
BEST_MCU_NAME=robot.behavioralstate.io BEST_MCU_ENDPOINT=https://robot.behavioralstate.io BEST_MCU_WIFI_SSID=... BEST_MCU_WIFI_PASSWORD=... cargo run --release
```

The name and the endpoint are `localhost` and `https://localhost` when unset. The certificate is built into the
firmware, so each renewal is copied and flashed again the same way. Where the name resolves to the cluster, a machine
on the board's network reaches the board by the name mapped to its address (its hosts file, or the router's DNS), and
best-validate is run against it as any client runs it, the certificate verified:

```bash
npx @behavioralstate/best-validate@0.4.3 https://robot.behavioralstate.io
```

A board without a name is reached by its address instead, with a self-signed certificate for it
(`scripts/dev-cert.sh 192.168.1.104`, endpoint `https://192.168.1.104`), and best-validate is told to accept it
(`NODE_TLS_REJECT_UNAUTHORIZED=0`), as the gate does for QEMU.

## Signing an agent in

The manifest declares the device flow (RFC 8628): the agent asks the robot for a code and gives the person a link to
the robot's own page, `/approve?code=...`, which shows who asks and the code. The person checks the code against what
the agent shows and clicks Approve, then holds the robot's BOOT button for 3 seconds and lets go, within 30 seconds of the
click: the hold is what approves, so only someone at the robot can let an agent in. A short press approves nothing, because it is
[the stop](#the-servo), and anyone who can reach the robot can arm a code. Deny refuses it. With best-cli:

```bash
best login robot --manifest robot.behavioralstate.io
```

```bash
best query robot get-status
```

A code lives 5 minutes, approved or not: an agent that collects its credential later is told the code expired. Four
codes may wait for the person at once; an approved one no longer counts among them. The robot keeps only a hash of
each credential, eight at most. A durable credential (the default) is kept in the board's flash and survives a
restart; a session credential lives 8 hours and is forgotten on a restart. Holding BOOT for 10 seconds forgets every
credential and takes back every code armed or approved but not yet collected, so each agent signs in again. QEMU has
no button, so an agent cannot be approved there: the door's tests prove the sign-in on any machine.

## The camera

The board's camera is a GC0308 at 640 by 480. A board without one, QEMU's included, declares none of what follows.
Everything past the manifest is behind the sign-in, as the rest of the door is.

An agent asks for a picture as it asks for anything else:

```bash
best query robot get-snapshot
```

It answers JSON, the picture base64 inside it, so any BEST client can read it as it is:

```json
{ "contentType": "image/jpeg", "takenAt": "2026-11-09T12:25:37.090Z", "width": 640, "height": 480, "data": "/9j/4AAQ..." }
```

`takenAt` is `null` until the board has the time from the network. A camera that gives no picture is answered with a
503, `CAMERA_UNAVAILABLE`.

Live frames are this robot's own, outside the spec: the custom capability `io.behavioralstate.robot.camera`, a service
of its own in the manifest, serving `GET /camera/stream` as MJPEG (`multipart/x-mixed-replace`) over HTTPS. A browser
and OpenCV open it as they are. Neither can send a credential, so an agent asks for a link and gives it to them:

```bash
best query robot get-camera-link
```

The link answered is `https://robot.behavioralstate.io:8443/camera/stream?ticket=...`, good once and for 30 seconds; a
later link replaces it. A client that can send the credential opens `/camera/stream` with it, without a link. The stream
is on a port of its own, because a server's task is held for as long as a viewer watches: the door's other answers
never wait for it. There is one viewer at a time. Frames are never on `/events/stream`. Live frames end when the
credential they are watched under does, by its life or by BOOT held for 10 seconds; a link's is the credential that
asked for it, and a link from a credential since forgotten opens nothing.

Measured on the board, from a machine on the same network:

| | |
|---|---|
| A frame in PSRAM | 614 KB raw, two of them held; 19 to 25 KB as a JPEG, at quality 60 |
| Encoding a frame on the CPU | 341 ms at 240 MHz (552 ms at 160 MHz, with C built for debugging and PSRAM at 40 MHz) |
| Live frames | 2.2 a second at 640 by 480, in OpenCV and to `curl` alike |
| `best query robot get-snapshot`: the manifest, the catalogue and the query, each over a new connection | 1.5 to 2.1 s, some 0.7 s more than `get-status` |

The sensor has no JPEG of its own, so each frame is encoded by the CPU, and that is what holds the frame rate. The
camera's clock is 24 MHz and not the usual 20: at 20 its harmonic at 2460 MHz fell inside Wi-Fi channel 11, the
network's here, and the board lost half its packets, with answers up to 3 s late. At 24 it lost none of 15 pings.

## The servo

The servo's signal is on GPIO 14, its ground is joined with the board's, and its power is what the servo is rated for.
One lightly loaded servo can run from the board's 5V pin, as the proof here did; one that works harder needs a supply of
its own, or the board resets when USB cannot keep up. A board without a servo, QEMU's included, declares none of what
follows. Everything past the manifest is behind the sign-in.

The catalogue names a skill, never a motor:

```bash
best commands robot
```

```bash
best send robot look --data '{"toward":"left"}'
```

`toward` is `left`, `ahead` or `right`: pulses of 2000, 1500 and 1000 µs. How far those turn it, and which way is left,
depend on the servo and how it is mounted. It is sent nothing until the first command, then holds where it was told.

The result is an event:

```bash
best events robot
```

`Looked` says the servo was told to turn, `LookFailed` says why it could not. A hobby servo reports nothing back, so
neither says where it arrived.

The retention window, for a command's `id` and for the event log alike, is **the last 8 commands accepted**, kept in
flash through a restart and rewritten there by each one. Within it, a command sent again with the same `id` by the same
credential is answered as the first was and moves nothing, and the same `id` with another payload is a `409`. Past it
the `id` is new again. A credential reads only the events of its own commands.

Commands are taken at two a second for each credential, and a faster one is a `429` with `Retry-After`. An `id` or a
correlation id is 64 bytes at most. Until the board has the time from the network a command is refused with a
`400`, `TIME_UNKNOWN`, since every event carries it. If the flash does not take a command the answer is `202` and not
`201`: the servo has already moved. There is no live stream of events; a client reads `GET /events` again.

**The stop is the BOOT button.** The moment it goes down, with a sign-in waiting or not and however long it is then
held, a servo holding a position has the pulses taken off it and goes loose, and every command is refused with a `400`,
`STOPPED`. Every command is refused for as long as the button is down. A short press does to a loose servo what going
down does to a holding one. Stopped, a short press lets commands through again and
moves nothing: the servo stays loose until the next one. A hold that approves a sign-in stops a servo holding a position
all the same. The stop works with the network down, and a restart clears it, with the servo still sent nothing until a
command. It stops the signal and not the power, so a switch in the servo's own supply
is the way to cut that.

Everything the spec asks of commands and events fit on the board: nothing was left out for want of flash or RAM.
