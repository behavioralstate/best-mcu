# An agent moves a servo on the board

best-cli 0.3.0 and best-validate 0.4.5, as the agent, against the board at `https://robot.behavioralstate.io` (a Freenove
ESP32-S3-WROOM-1 N16R8, on Wi-Fi, with its Let's Encrypt certificate), on 2026-11-09. The agent used the credential it
holds from [its sign-in](sign-in.md), which is printed nowhere here. The servo was a Hitec HS-422, a joint of an older
robot with its lead moved to the board: signal on GPIO 14, power from the board's 5V pin, nothing else on the robot
powered. A person at the robot watched it, and what they saw is said as such below.

## What the board offers

From the manifest, as any client finds it:

```text
$ best commands robot
COMMAND  VERSION  IMPACT  DESCRIPTION
look     1.0              Turn the robot's head to look left, ahead or right. Looked says it was told to, or LookFa…

$ best schema robot look
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "commandType": "Look",
  "description": "Turn the robot's head to look left, ahead or right. Looked says it was told to, or LookFailed says why not; the head does not report where it arrived.",
  "produces": [
    "Looked",
    "LookFailed"
  ],
  "properties": {
    "toward": {
      "description": "Where to look.",
      "enum": [
        "left",
        "ahead",
        "right"
      ],
      "type": "string"
    }
  },
  "required": [
    "toward"
  ],
  "type": "object"
}
```

## The servo moves

```text
$ best send robot look --data '{"toward":"ahead"}' --id servo-ahead-1 --json
{"correlationId":"servo-ahead-1","id":"servo-ahead-1"}
```

`left`, `right` and `ahead` again followed the same way. The person at the robot saw the joint go to each and hold, with
no buzzing against its bracket, and then sent `left`, `right` and `ahead` from their own terminal. The board did not
restart. Its log, as the last eight commands left it:

```text
$ best events robot
TIME                      TYPE    CORRELATION                           ID                                DATA
2026-11-09T17:37:11.833Z  Looked  servo-ahead-1                         a42fde58b6d7e12e29a1c4c00d64caa6  {"toward":"ahead"}
2026-11-09T17:37:29.552Z  Looked  servo-left-1                          e165f97bbfead1b554b658bea9d64ca1  {"toward":"left"}
2026-11-09T17:37:53.237Z  Looked  servo-right-1                         1fcbe7005f27d0499a2b65020c509dec  {"toward":"right"}
2026-11-09T17:38:52.106Z  Looked  servo-ahead-2                         c7a0ce11bc9b72f02b6eb05a0e6714a8  {"toward":"ahead"}
2026-11-09T17:47:57.812Z  Looked  servo-released-1                      bb71e5fa4a1086363b5a3242b7bef131  {"toward":"ahead"}
2026-11-09T17:52:54.359Z  Looked  40931e70-91d5-4962-becb-af93e3e7aaf6  8057bcc5db244c6d0df6101ef0e400f0  {"toward":"left"}
2026-11-09T17:53:04.283Z  Looked  efc34249-885d-4d17-b6f5-d5c5a4a1df41  aa7ab821732d9f7ad3720744ab590f73  {"toward":"right"}
2026-11-09T17:53:16.179Z  Looked  e7dd7d18-bcb2-477d-8dd9-9483c5733260  3490900ff7e9bdd946140f0263857630  {"toward":"ahead"}
```

## What it refuses

The same `id` and payload again is answered as the first was, and moves nothing. Another payload, and a value the
schema does not allow:

```text
$ best send robot look --data '{"toward":"ahead"}' --id servo-released-1 --json
{"correlationId":"servo-released-1","id":"servo-released-1"}

$ best send robot look --data '{"toward":"right"}' --id servo-released-1 --json
{"error":{"code":"DUPLICATE_ID","message":"the id servo-released-1 was sent with another payload: send this one under a new id","details":{"id":"servo-released-1"},"status":409},"exit":5}

$ best send robot look --data '{"toward":"up"}' --id servo-bad-1 --json
{"error":{"code":"INVALID_COMMAND","message":"toward is required: left, ahead or right","details":{"field":"data.toward"},"status":400},"exit":5}
```

## The button stops it

One short press of BOOT. The person at the robot felt the joint go loose, and the next command was refused, with no
event told for it:

```text
$ best send robot look --data '{"toward":"left"}' --id servo-stopped-1 --json
{"error":{"code":"STOPPED","message":"someone at the robot stopped it with its button: it takes commands again once the button is pressed again","details":{},"status":400},"exit":5}
```

A second press moved nothing, as it should. The next command was taken, and is `servo-released-1` in the log above.

## A sign-in cannot borrow the stop

Anyone who can reach the robot can arm a sign-in, so a short press is only ever the stop, and approving takes holding
the button for 3 seconds. The firmware was changed for that after the runs above, flashed, and tried with a script
playing a stranger on the network: it registered a session sign-in, armed it again every 18 s and polled it, while `look`
was sent every few seconds. The person at the robot gave two short presses and then a hold:

- At 4.6 s the sign-in was pending, and it stayed pending through both short presses.
- At 5.8 s a command was taken. At 31.1 s, after the first short press, one was refused; the script did not keep the
  refusal's code. At 38.6 s, after the second, one was taken again.
- At 44.3 s, after the hold, the sign-in was approved. Its credential was thrown away unread.

The stop was then moved to the moment the button goes down, so that a hold of any length stops the servo too, and the
firmware flashed again. With the joint holding a position and BOOT held for 3 seconds, the person at the robot felt the joint go loose as soon as the button went down, before it was let go.

Commands are also refused for as long as the button is down, so one that lands during a press cannot move the servo.
With `look` sent over and over, BOOT held for about 4 seconds and then one short press, the answers changed at:

```text
   2.2s Looked
  55.1s STOPPED: someone at the robot has its button down: send this again once it is let go
  61.7s STOPPED: someone at the robot stopped it with its button: it takes commands again once the button is pressed again
  64.4s STOPPED: someone at the robot has its button down: send this again once it is let go
  67.3s Looked
```

The person at the robot felt the joint go loose at the hold, and stiffen again when the first command after the short
press was taken.

## Through a restart

With no servo attached, on an earlier build with the same command code: ten more commands, `many-1` to `many-10`, then the board restarted. It
still held eight events, `many-3` to `many-10`, and `many-10` sent again was answered as the first was.

## What it costs

The firmware image is 1,481,200 bytes, 9.04% of the app partition. Internal RAM free was 282 KB at start and 100 KB with
the door served. A viewer of the live frames with four more TLS connections held open to the door, after the commands
above: all four were answered, the viewer had 52 frames in 24 s, the least internal RAM free since start was 39,904
bytes, and the board did not restart.

## best-validate

```text
$ npx @behavioralstate/best-validate@0.4.5 https://robot.behavioralstate.io
CONFORMANT — 0 failure(s), 0 warning(s). Mode: BEST 0.9.17
```

It reads the manifest, the custom capability's service and the sign-in's refusals. Past the sign-in it was given no
credential, so the commands' answers are from best-cli above.

Every requirement the spec makes of commands and events fit on the board. No client failed on a sentence of the spec:
best-cli and best-validate each read what the board serves as it is.
