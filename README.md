# best-mcu

A [Behavioral State Protocol](https://behavioralstate.io) door served from a microcontroller: a robot made of a
Wi-Fi board, servos and sensors, with no Linux computer and no ROS, whose skills an agent reads and drives from the
manifest alone. The question it answers is whether everything the spec asks of a door fits on such a board. The plan
is [behavioralstate/spec#275](https://github.com/behavioralstate/spec/issues/275), on the
[Behavioralstate board](https://github.com/orgs/Amarcode-Studios/projects/4).

The board is an ESP32-S3 with 16 MB of flash and 8 MB of PSRAM (N16R8). The firmware is Rust with the standard
library on ESP-IDF.

**Where it stands:** the door serves its manifest over HTTPS, with an ETag and a cache life, and refuses what it does
not serve in the BEST error format. An agent signs in with the device flow, approved by a press of the robot's BOOT
button, and reads one query, `get-status`: the robot's uptime, free memory and Wi-Fi signal. It runs on the board, a
Freenove ESP32-S3-WROOM-1 on Wi-Fi, as `https://robot.behavioralstate.io` with a Let's Encrypt certificate, and in
Espressif's QEMU: best-validate 0.4.3 finds it conformant on both, the board's certificate verified. best-cli signs in
to the board and queries it: [acceptance/sign-in.md](acceptance/sign-in.md).

ESP-IDF's HTTP server gives every answer it sends without a body a length of 0, where HTTP allows none or the length
GET's answer has. So a client sending `If-None-Match` is sent the manifest whole, never a 304, with the ETag still on
it so the client can tell it changed; and `HEAD`, answered as GET is without the body, says `Content-Length: 0`. A
client learns the manifest's length by reading it with GET.

What it costs:

| | QEMU | The board |
|---|---|---|
| Firmware image (ESP-IDF, lwIP, mbedTLS, the HTTPS server, the door; the board's with Wi-Fi) | 833 KB, 5.1% of the flash | 1.1 MB, 7.1% |
| Internal RAM free at start | 354 KB | 307 KB |
| … with the network up and the door served | 311 KB | 226 KB |
| … least, after TLS handshakes | 244 KB | 123 KB |
| … least, while a person approves a sign-in in a browser and best-cli waits for it | | 60 KB |
| PSRAM used | almost none | almost none |
| The manifest read over a new connection, from a machine on the same network | | 0.9 to 1.2 s, the TLS handshake nearly all of it |
| `best query robot get-status`: the manifest, the catalogue and the query, each over a new connection | | 1.1 to 1.6 s |

The board's figures are with its Let's Encrypt chain, four certificates in 4.7 KB. Each open TLS connection holds
some 30 KB of internal RAM, and the server keeps four open at most, so a sign-in, with a browser's connections and
best-cli's at once, is what costs the board most. Timings are the board's alone: QEMU's say nothing of them.

## Layout

- `crates/door`: the door, plain Rust: what each request is answered with, the sign-in and its page, and the queries.
  It knows nothing of the board: it is given a clock, a random generator, a vault for the credentials and the
  board's readings, and its tests run on any machine with fakes of them: `cargo test`.
- `firmware`: the board's side, built with the esp toolchain: the network (Wi-Fi, or QEMU's Ethernet with the `qemu`
  feature), ESP-IDF's HTTPS server carrying requests to the door, the flash that keeps credentials, the BOOT button,
  and the memory report on the serial log.

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
the agent shows and clicks Approve, then presses the robot's BOOT button within 30 seconds: the press is what approves,
so only someone at the robot can let an agent in. Deny refuses it. With best-cli:

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
