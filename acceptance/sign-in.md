# An agent signs in to the board

best-cli 0.3.0, as the agent, against the board at `https://robot.behavioralstate.io` (a Freenove ESP32-S3-WROOM-1
N16R8 on Wi-Fi, with its Let's Encrypt certificate), on 2026-11-08. The person approved on the board's page and
pressed its BOOT button. The credential is in best-cli's credential store and is printed nowhere here.

## Sign in

```text
$ best login robot --manifest robot.behavioralstate.io
Resolved robot.behavioralstate.io to https://robot.behavioralstate.io/.well-known/best

Open this link in your own browser, sign in and approve:

    https://robot.behavioralstate.io/approve?code=FXXQ-FRNT

The code is FXXQ-FRNT. Waiting for your approval (the code expires in 5 minutes)…

The service's sign-in guidance, as it wrote it:

(the manifest's and the device answer's guidance, verbatim from the spec's texts of 0.9.17)

Signed in as 'robot' at robot.behavioralstate.io; the credential is durable until revoked.
The credential is in …\.best-mcp\credentials.json, where best-mcp finds it too. It is printed only when asked, by `best credential`.
  io.behavioralstate.robot  https://robot.behavioralstate.io
Next: `best queries robot` and `best commands robot`.
```

The page showed who asked and the code; Approve armed it, and the board's serial log shows the press:

```text
I (472718) best_mcu::board: the button approved the sign-in FXXQ-FRNT
```

An earlier code, left unapproved, ran out: best-cli said `the code expired before it was approved` and stopped.

## The credential survives a restart

The board was restarted (its RST button) after the sign-in, and later flashed with a new build of the firmware, which
leaves the key-value store alone. The credential's hash is in the board's flash, so the same credential is admitted
after either:

```text
$ best queries robot
QUERY       VERSION  DESCRIPTION
get-status  1.0      How the robot is: how long it has run, the memory it has free, and the strength of its Wi…

$ best query robot get-status
{
  "memory": {
    "internalFree": 198027,
    "internalLeast": 156260,
    "psramFree": 8386040
  },
  "uptimeSeconds": 23,
  "wifi": {
    "rssi": -48
  }
}

$ best schema robot get-status --kind query
{
  "description": "How the robot is: how long it has run, the memory it has free, and the strength of its Wi-Fi signal.",
  "response": { "type": "object", "required": ["uptimeSeconds", "memory", "wifi"], … }
}
```

## Without a credential

```text
$ curl -i https://robot.behavioralstate.io/queries
HTTP/1.1 401
WWW-Authenticate: Bearer
{"error":{"code":"UNAUTHORIZED","message":"a credential is required: Authorization: Bearer <token>, obtained by the sign-in the manifest's authentication block declares"}}
```
