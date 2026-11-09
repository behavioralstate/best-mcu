# An agent sees through the board's camera

best-cli 0.3.0 and best-validate 0.4.5, as the agent, against the board at `https://robot.behavioralstate.io` (a Freenove
ESP32-S3-WROOM-1 N16R8 with a GC0308 camera, on Wi-Fi, with its Let's Encrypt certificate), on 2026-11-09. The agent
used the credential it holds from [its sign-in](sign-in.md), which is printed nowhere here. The picture is not kept in
the repository, since it shows the room the board is in.

## What the board offers

From the manifest, as any client finds it:

```text
$ best queries robot
QUERY            VERSION  DESCRIPTION
get-status       1.0      How the robot is: how long it has run, the memory it has free, and the strength of its Wi…
get-snapshot     1.0      What the robot sees now: one picture from its camera, as a JPEG.
get-camera-link  1.0      An address to watch the robot's camera live, which a browser or OpenCV can open as it is.…
```

## The agent reads a snapshot and says what it sees

```text
$ best query robot get-snapshot
{ "contentType": "image/jpeg", "takenAt": "2026-11-09T12:25:37.090Z", "width": 640, "height": 480, "data": "<32972 characters of base64>" }
```

The data decoded to 24,727 bytes of JPEG. The agent, reading that picture:

> A plain ceiling and wall, cream with a moulding, seen from low down. In front of them two dark computer screens,
> tilted away. The nearer one is on: a desktop with rows of icons, and a black-and-white bird on a branch as its
> wallpaper.

## The live frames

```text
$ best query robot get-camera-link
{
  "expiresIn": 30,
  "url": "https://robot.behavioralstate.io:8443/camera/stream?ticket=<one use>"
}
```

Opened as it is, the certificate verified:

- In a browser (the Chromium of Claude's desktop app): the document's type was `multipart/x-mixed-replace`, its image
  640 by 480, and what the screen in the picture showed changed between frames.
- In OpenCV 5.0.0, `cv2.VideoCapture(url, cv2.CAP_FFMPEG)`: opened, 34 frames of `(480, 640, 3)` in 15.3 s, 2.22 a
  second.
- A link used once: `401`. `curl` with it the first time had `200`, `Content-Type: multipart/x-mixed-replace;
  boundary=frame`.

## The board under the most it serves

A viewer of the live frames with four more TLS connections held open to the door: all four were answered, in 1.0 to
1.3 s, the viewer lost nothing (52 frames in 24 s), and the least internal RAM free was 45 KB. The board did not
restart.

## best-validate

```text
$ npx @behavioralstate/best-validate@0.4.5 https://robot.behavioralstate.io
CONFORMANT — 0 failure(s), 0 warning(s).
```

It reads the manifest, the custom capability's service and the sign-in's refusals. Past the sign-in it was given no
credential, so the catalogue's answers are from best-cli above.

No client failed on a sentence of the spec: best-cli and best-validate each read what the board serves as it is.
