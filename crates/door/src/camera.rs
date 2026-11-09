//! The robot's camera as the door answers it: a snapshot as JSON, live frames as MJPEG, and the short-lived link that
//! lets a browser or OpenCV, which send no `Authorization` header, open them behind the sign-in.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The port the live frames are served on. A stream holds its server's one task for as long as it is watched, so it
/// has a server of its own and the door's other answers never wait for it.
pub const STREAM_PORT: u16 = 8443;

/// The content type of the live frames.
pub const FRAMES_TYPE: &str = "multipart/x-mixed-replace; boundary=frame";

pub(crate) const LINK_LIFE_S: u64 = 30;

pub(crate) const SNAPSHOT_DESCRIPTION: &str = "What the robot sees now: one picture from its camera, as a JPEG.";
pub(crate) const LINK_DESCRIPTION: &str = "An address to watch the robot's camera live, which a browser or OpenCV can \
                                           open as it is. It works once, for a short time.";

/// One picture from the camera.
pub struct Frame {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The body of `get-snapshot`. Written by hand: a picture is too large to hold twice as a JSON tree.
pub(crate) fn snapshot(frame: &Frame, utc_ms: Option<u64>) -> String {
    let taken_at = utc_ms.map_or("null".to_string(), |ms| format!("\"{}\"", rfc3339(ms)));
    let mut body = format!(
        "{{\"contentType\":\"image/jpeg\",\"takenAt\":{taken_at},\"width\":{},\"height\":{},\"data\":\"",
        frame.width, frame.height
    );
    STANDARD.encode_string(&frame.jpeg, &mut body);
    body.push_str("\"}");
    body
}

/// One part of the MJPEG stream.
pub(crate) fn part(frame: &Frame) -> Vec<u8> {
    let head = format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        frame.jpeg.len()
    );
    let mut part = Vec::with_capacity(head.len() + frame.jpeg.len() + 2);
    part.extend_from_slice(head.as_bytes());
    part.extend_from_slice(&frame.jpeg);
    part.extend_from_slice(b"\r\n");
    part
}

pub(crate) fn snapshot_schema() -> Value {
    json!({
        "description": SNAPSHOT_DESCRIPTION,
        "response": {
            "type": "object",
            "required": ["contentType", "takenAt", "width", "height", "data"],
            "properties": {
                "contentType": { "const": "image/jpeg" },
                "takenAt": {
                    "type": ["string", "null"],
                    "format": "date-time",
                    "description": "When it was taken, null while the robot does not yet know the time.",
                },
                "width": { "type": "integer", "minimum": 1, "description": "In pixels." },
                "height": { "type": "integer", "minimum": 1, "description": "In pixels." },
                "data": { "type": "string", "contentEncoding": "base64", "description": "The JPEG." },
            },
        },
    })
}

pub(crate) fn link_schema() -> Value {
    json!({
        "description": LINK_DESCRIPTION,
        "response": {
            "type": "object",
            "required": ["url", "expiresIn"],
            "properties": {
                "url": { "type": "string", "format": "uri", "description": "Open it to watch; it works once." },
                "expiresIn": { "type": "integer", "minimum": 1, "description": "Seconds it can still be opened." },
            },
        },
    })
}

/// Someone admitted to the live frames, by the credential they are watched under: a link's is the credential that
/// asked for it. The frames end when it does.
#[derive(Debug)]
pub struct Watcher(pub(crate) [u8; 32]);

/// The address of the live frames: the endpoint's host, on their own port.
pub(crate) fn frames_origin(endpoint: &str) -> String {
    let host = endpoint
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(endpoint, |(host, _)| host);
    format!("{host}:{STREAM_PORT}")
}

struct Waiting {
    secret: [u8; 32],
    ends_ms: u64,
    asker: [u8; 32],
}

/// The one link waiting to be opened: the hash of its secret, when it ends and the credential that asked for it. A new
/// link replaces the last.
#[derive(Default)]
pub(crate) struct Ticket(Option<Waiting>);

impl Ticket {
    pub(crate) fn issue(&mut self, now_ms: u64, asker: [u8; 32], random: &mut dyn FnMut(&mut [u8])) -> String {
        let mut secret = [0u8; 16];
        random(&mut secret);
        let secret: String = secret.iter().map(|b| format!("{b:02x}")).collect();
        self.0 = Some(Waiting {
            secret: Sha256::digest(secret.as_bytes()).into(),
            ends_ms: now_ms + LINK_LIFE_S * 1000,
            asker,
        });
        secret
    }

    /// The credential that asked for the link waiting, when `given` is that link, in time; either way a matching link
    /// is spent.
    pub(crate) fn redeem(&mut self, now_ms: u64, given: &str) -> Option<[u8; 32]> {
        let hash: [u8; 32] = Sha256::digest(given.as_bytes()).into();
        let waiting = self.0.take_if(|w| w.secret == hash)?;
        (now_ms < waiting.ends_ms).then_some(waiting.asker)
    }
}

/// `ms` since the epoch as RFC 3339, UTC (the days to a date by Howard Hinnant's `civil_from_days`).
fn rfc3339(ms: u64) -> String {
    let (secs, ms) = (ms / 1000, ms % 1000);
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn a_time_is_written_as_rfc_3339_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339(1_794_229_199_007), "2026-11-09T12:59:59.007Z");
    }
}
