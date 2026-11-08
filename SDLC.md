# How best-mcu is built and shipped

> This is the one place the lifecycle of this repository is written down. Open work of any kind (a slice, a bug, a
> decision) is an issue on the Behavioralstate board, never a file: the plan is
> [behavioralstate/spec#275](https://github.com/behavioralstate/spec/issues/275).

## 1. Intake

- Work is #275, or an issue filed next to it on the same board, with the problem, the outcome from the agent's side,
  numbered decisions, and a "Done when" list. Every one carries the `best-robot` label and no epic's.
- **The BEST spec is stable** ([behavioralstate/spec#37](https://github.com/behavioralstate/spec/issues/37) holds the
  rule). A behaviour the spec does not mention is best-mcu's own, documented in its README: never an entry on #37, a
  spec issue or a spec pull request. A requirement that cannot be met on the board is a finding: the session brings
  the proof to the owner in the chat and stops.
- The board's Status: **Backlog** until the first change; **In progress** from the first change until the issue
  closes; **Done** when it closes, every "Done when" ticked.

## 2. Design rules every change is held to

- BEST is the surface, written from the spec's text: a capability exists only when it has a schema, a catalogue entry
  and a description an agent can act on; a refusal says what to do; sign-in is the spec's.
- **Skills, never motors.** Nothing below the skill level is on the door: no PWM duty, no raw servo angle stream. The
  board keeps the real-time loop, and the stop is a button wired to the board, never only a command.
- **The door knows nothing of the board.** `crates/door` decides every answer from the request and the ports it is
  given; the firmware carries requests to it and chooses the adapters (the network, the storage, the pins). The door's
  tests run on any machine, against fakes. An adapter is thin, proven in QEMU where QEMU emulates it, and on the board
  by hand, with the proof on the issue.
- **Measured, not assumed.** What a change costs in flash and RAM is read from the build and the serial log's memory
  report, and the README's figures move with it. Timings are taken on the board, never in QEMU.
- **Nothing secret is committed.** The network's name and password are read when the firmware is built; certificates
  and keys live in `firmware/certs`, which git ignores.
- **Small and cohesive.** A module owns one thing; the wire shape of a fact lives next to its schema. Nothing is added
  before a slice needs it.
- Facts are events; an event is versioned, never mutated; a redelivery is idempotent; nothing is clamped silently.
- The README's "Try it" is the operator's path, kept true by the change that moves it.

## 3. Build

- Tests first where one applies, against the fakes.
- `scripts/gate.sh all` (in Git Bash on Windows, with the MSVC Build Tools environment loaded) runs `cargo fmt
  --check` on the door and the firmware, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test
  --workspace` on the door, the board's firmware build (Wi-Fi included, against a placeholder network), then the qemu
  lane: the firmware built for Espressif's QEMU and run there, `best-validate` against the door it serves over HTTPS,
  HEAD answered as GET without the body, and the manifest sent whole to a client that holds it. It ends in `GATE
  GREEN` or `GATE RED`; CI runs the same script. No board is ever needed by CI.

## 4. Ship

- **Every change is made on `main` and pushed there**, one commit per piece of work, the subject one plain sentence
  saying what changed and why, with no type prefix; the one prefix is `Review:` on a commit that answers the review.
- **Two gates before every push, both mandatory:**
  1. **The local proof**: `scripts/gate.sh all` ending in `GATE GREEN`.
  2. **The review**: the `workspace-review` skill, run on this repository, on what `main` holds that `origin/main`
     does not. Every finding is fixed on `main` with a test, in a `Review:` commit that names it, or set aside with
     its reason in that message. The local gate runs last, on the head that is pushed.
- `git pull --rebase` before the gates; a red `main` is fixed before anything else is pushed.
- A release, and the workflow that builds it, comes with the first slice that needs someone else to flash the board.

## 5. What we do not do

- No push without `GATE GREEN` and the review; no force-push to `main`.
- No plan file, no `TODO`, no dated narrative: open work is an issue.
