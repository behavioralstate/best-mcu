# CLAUDE.md

How this repository is built and shipped is [`SDLC.md`](SDLC.md), the one place the lifecycle is written down. Read it
before changing anything, and keep it the current rule when a change moves the process. What the door does is in
[`README.md`](README.md). In short:

- Open work is an issue on the Behavioralstate board (project 4 of Amarcode-Studios), filed in behavioralstate/spec
  with the `best-robot` label, never a plan file or a `TODO`. Starting on an issue, move it to In progress on the
  board's Status field first (`gh project item-edit`); it is Done when it closes, every "Done when" ticked.
- The BEST spec is stable (behavioralstate/spec#37). What the door does that the spec does not mention is best-mcu's
  own, in its README. A requirement the board cannot meet is a finding: bring the proof to the owner in the chat and
  stop. Never file a spec issue or draft spec text on your own.
- `crates/door` knows nothing of the board: every answer is decided from the request and the ports it is given, and
  its tests run on any machine against fakes (`cargo test`). The firmware is a thin adapter, proven in QEMU where QEMU
  emulates it and on the board by hand. Skills, never motors; the stop is the BOOT button, never only a command.
- Measured, not assumed: what a change costs in flash and RAM is read from the build and the serial log, and the
  README's figures move with it. Timings are the board's, never QEMU's.
- Nothing secret is committed: the Wi-Fi name and password are read at build time, and certificates live in
  `firmware/certs`, which git ignores.
- Every change is made on `main` and pushed there. The subject is one plain sentence saying what changed and why, with
  no type prefix; the one prefix is `Review:` on a commit that answers the review. `git pull --rebase` before the
  gates; a red `main` is fixed before anything else is pushed.
- Two gates before every push, both mandatory. First the local proof: `scripts/gate.sh all` (Git Bash on Windows,
  inside the MSVC Build Tools environment, with the short paths the README gives) must end in `GATE GREEN`; CI runs
  the same script. Then the review: the `workspace-review` skill on what `main` holds that `origin/main` does not,
  every finding fixed on `main` with a test in a `Review:` commit that names it, or set aside with its reason in that
  message. The local gate runs last, on the head that is pushed. No force-push to `main`.
