//! Signing an agent in (SPEC.md, Agent Registration): RFC 8628's device flow, approved on the robot itself. The person
//! opens the link the agent shows, arms the code they see there, and presses the robot's button: whoever holds the
//! robot approves. Only a hash of each credential is kept; durable ones survive a restart through the vault, session
//! ones live in memory and end with it.

use sha2::{Digest, Sha256};

/// How long a registration waits for the person, and how often the agent may poll.
pub const EXPIRES_IN_S: u64 = 300;
pub const INTERVAL_S: u64 = 5;
/// How long an armed code waits for the button.
pub const ARMED_FOR_S: u64 = 30;
/// The life of a session credential.
pub const SESSION_S: u64 = 8 * 3600;
/// The credentials the robot keeps, approved or issued.
pub const CAPACITY: usize = 8;
/// The registrations that may wait for the person at once.
pub const WAITING: usize = 4;

/// RFC 8628 §6.1: no vowels, no look-alikes.
const USER_CODE_LETTERS: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lifetime {
    Durable,
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Pending,
    Armed {
        until_ms: u64,
    },
    Approved,
    Denied,
    Redeemed,
    /// Never stored: what a code nobody answered in time reads as.
    Expired,
}

pub struct Registration {
    pub device_code: String,
    pub user_code: String,
    pub client_id: String,
    pub agent_label: Option<String>,
    pub connection: Option<String>,
    pub lifetime: Lifetime,
    pub expires_ms: u64,
    interval_ms: u64,
    last_poll_ms: Option<u64>,
    pub state: State,
}

impl Registration {
    /// What the page shows of it: its state at `now`, a code unanswered or uncollected when it ran out read as expired.
    pub fn state_at(&self, now: u64) -> State {
        match self.state {
            State::Pending | State::Armed { .. } | State::Approved if now >= self.expires_ms => State::Expired,
            State::Armed { until_ms } if now >= until_ms => State::Pending,
            s => s,
        }
    }

    pub fn expired_at(&self, now: u64) -> bool {
        matches!(self.state_at(now), State::Expired)
    }
}

struct Credential {
    hash: [u8; 32],
    name: String,
    /// A session credential's end; a durable one has none and is the one the vault keeps.
    expires_ms: Option<u64>,
}

/// An error of the device and token endpoints, in RFC 6749 §5.2's words.
#[derive(Debug, PartialEq)]
pub struct OAuthError {
    pub error: &'static str,
    pub description: String,
}

fn oauth(error: &'static str, description: impl Into<String>) -> OAuthError {
    OAuthError {
        error,
        description: description.into(),
    }
}

/// A credential issued to an agent: the secret itself, once, and what goes with it.
pub struct Issued {
    pub access_token: String,
    pub name: Option<String>,
    pub expires_in: Option<u64>,
}

/// Why a code could not be armed, in words for the person on the page.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    Unknown,
    NotWaiting,
    Full,
    Busy { seconds: u64 },
}

#[derive(Default)]
pub struct SignIn {
    registrations: Vec<Registration>,
    credentials: Vec<Credential>,
}

impl SignIn {
    /// The durable credentials the vault kept, one per line: the hash in hex, a space, the connection's name.
    pub fn restore(kept: &str) -> Self {
        let credentials = kept
            .lines()
            .filter_map(|line| {
                let (hash, name) = line.split_once(' ')?;
                Some(Credential {
                    hash: from_hex(hash)?,
                    name: name.to_string(),
                    expires_ms: None,
                })
            })
            .collect();
        Self {
            registrations: Vec::new(),
            credentials,
        }
    }

    /// What the vault keeps: the durable credentials, as [`SignIn::restore`] reads them.
    pub fn kept(&self) -> String {
        self.credentials
            .iter()
            .filter(|c| c.expires_ms.is_none())
            .map(|c| format!("{} {}\n", hex(&c.hash), c.name))
            .collect()
    }

    /// RFC 8628 §3.1–3.2: a registration, waiting for the person.
    pub fn register(
        &mut self,
        now: u64,
        random: &mut dyn FnMut(&mut [u8]),
        client_id: Option<&str>,
        agent_label: Option<&str>,
        connection: Option<&str>,
        lifetime: Option<&str>,
    ) -> Result<&Registration, OAuthError> {
        let client_id = client_id.filter(|c| !c.is_empty()).ok_or_else(|| {
            oauth(
                "invalid_request",
                "client_id is required: the name of your software, such as best-mcp",
            )
        })?;
        if let Some(c) = connection {
            if !is_connection(c) {
                return Err(oauth(
                    "invalid_request",
                    "connection is one word of lowercase letters, digits and hyphens, at most 64, starting with a \
                     letter or digit (^[a-z0-9][a-z0-9-]{0,63}$), such as robot or acme-payroll-dev",
                ));
            }
        }
        let lifetime = match lifetime {
            None | Some("durable") => Lifetime::Durable,
            Some("session") => Lifetime::Session,
            Some(_) => return Err(oauth("invalid_request", "credential_lifetime is durable or session")),
        };
        self.forget_old(now);
        let waiting = self.registrations.iter().filter(|r| waits(r, now)).count();
        if waiting >= WAITING {
            return Err(oauth(
                "temporarily_unavailable",
                format!(
                    "{WAITING} sign-ins are waiting for the person already; ask again once one is approved, denied or \
                     past its {EXPIRES_IN_S} seconds"
                ),
            ));
        }
        let mut device = [0u8; 32];
        random(&mut device);
        let user_code = loop {
            let code = user_code(random);
            if !self.registrations.iter().any(|r| r.user_code == code) {
                break code;
            }
        };
        self.registrations.push(Registration {
            device_code: hex(&device),
            user_code,
            client_id: client_id.to_string(),
            agent_label: agent_label.filter(|l| !l.is_empty()).map(str::to_string),
            connection: connection.map(str::to_string),
            lifetime,
            expires_ms: now + EXPIRES_IN_S * 1000,
            interval_ms: INTERVAL_S * 1000,
            last_poll_ms: None,
            state: State::Pending,
        });
        Ok(self.registrations.last().expect("just pushed"))
    }

    /// RFC 8628 §3.4–3.5: the agent polls with its device code.
    pub fn redeem(
        &mut self,
        now: u64,
        random: &mut dyn FnMut(&mut [u8]),
        device_code: &str,
    ) -> Result<Issued, OAuthError> {
        self.forget_old(now);
        let unknown = || {
            oauth(
                "invalid_grant",
                "this device_code is not one the robot gave, or it was redeemed already: register again",
            )
        };
        let r = self
            .registrations
            .iter_mut()
            .find(|r| r.device_code == device_code)
            .ok_or_else(unknown)?;
        if r.expired_at(now) {
            return Err(oauth(
                "expired_token",
                "nobody approved this sign-in within its time: register again",
            ));
        }
        match r.state {
            State::Redeemed | State::Expired => Err(unknown()),
            State::Denied => Err(oauth(
                "access_denied",
                "the sign-in was denied on the robot, or taken back by holding its button to forget every credential: \
                 register again if the person wants you in",
            )),
            State::Pending | State::Armed { .. } => {
                let early = r.last_poll_ms.is_some_and(|last| now < last + r.interval_ms);
                r.last_poll_ms = Some(now);
                if early {
                    r.interval_ms += 5000;
                    Err(oauth(
                        "slow_down",
                        format!("poll no faster than every {} seconds", r.interval_ms / 1000),
                    ))
                } else {
                    Err(oauth(
                        "authorization_pending",
                        "the person has not approved yet: keep polling at the interval",
                    ))
                }
            }
            State::Approved => {
                r.state = State::Redeemed;
                let (name, lifetime) = (r.connection.clone(), r.lifetime);
                let mut secret = [0u8; 32];
                random(&mut secret);
                let access_token = hex(&secret);
                let expires_in = (lifetime == Lifetime::Session).then_some(SESSION_S);
                self.credentials.push(Credential {
                    hash: Sha256::digest(access_token.as_bytes()).into(),
                    name: name.clone().unwrap_or_default(),
                    expires_ms: expires_in.map(|s| now + s * 1000),
                });
                Ok(Issued {
                    access_token,
                    name,
                    expires_in,
                })
            }
        }
    }

    /// The registration a person's code names, typed as they read it.
    pub fn find(&self, user_code: &str) -> Option<&Registration> {
        let wanted = normal(user_code);
        self.registrations.iter().find(|r| normal(&r.user_code) == wanted)
    }

    /// The person confirms the code on the page: the button approves it for the next [`ARMED_FOR_S`] seconds.
    pub fn arm(&mut self, now: u64, user_code: &str) -> Result<(), Refusal> {
        self.forget_old(now);
        if let Some(other) = self
            .registrations
            .iter()
            .find(|r| matches!(r.state_at(now), State::Armed { .. }) && normal(&r.user_code) != normal(user_code))
        {
            let State::Armed { until_ms } = other.state_at(now) else {
                unreachable!()
            };
            return Err(Refusal::Busy {
                seconds: (until_ms - now).div_ceil(1000),
            });
        }
        let full = self.held(now) >= CAPACITY;
        let wanted = normal(user_code);
        let r = self
            .registrations
            .iter_mut()
            .find(|r| normal(&r.user_code) == wanted)
            .ok_or(Refusal::Unknown)?;
        if !matches!(r.state_at(now), State::Pending | State::Armed { .. }) {
            return Err(Refusal::NotWaiting);
        }
        if full {
            return Err(Refusal::Full);
        }
        r.state = State::Armed {
            until_ms: (now + ARMED_FOR_S * 1000).min(r.expires_ms),
        };
        Ok(())
    }

    /// The person denies the code on the page.
    pub fn deny(&mut self, now: u64, user_code: &str) -> Result<(), Refusal> {
        let wanted = normal(user_code);
        let r = self
            .registrations
            .iter_mut()
            .find(|r| normal(&r.user_code) == wanted)
            .ok_or(Refusal::Unknown)?;
        if !matches!(r.state_at(now), State::Pending | State::Armed { .. }) {
            return Err(Refusal::NotWaiting);
        }
        r.state = State::Denied;
        Ok(())
    }

    /// The robot's button was pressed: the armed code, if one is, is approved. Returns its user code.
    pub fn press(&mut self, now: u64) -> Option<String> {
        let full = self.held(now) >= CAPACITY;
        let r = self
            .registrations
            .iter_mut()
            .find(|r| matches!(r.state_at(now), State::Armed { .. }))?;
        if full {
            return None;
        }
        r.state = State::Approved;
        Some(r.user_code.clone())
    }

    /// Whether `token` is a credential the robot issued and still holds.
    pub fn admits(&mut self, now: u64, token: &str) -> bool {
        self.credentials.retain(|c| c.expires_ms.is_none_or(|end| now < end));
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.credentials.iter().any(|c| c.hash == hash)
    }

    /// A credential just issued is taken back.
    pub fn withdraw(&mut self, token: &str) {
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.credentials.retain(|c| c.hash != hash);
    }

    /// Every credential is forgotten, and every code armed or approved but not yet collected is taken back, as a
    /// denial: each agent signs in again.
    pub fn forget_credentials(&mut self) {
        self.credentials.clear();
        for r in &mut self.registrations {
            if matches!(r.state, State::Approved | State::Armed { .. }) {
                r.state = State::Denied;
            }
        }
    }

    /// Credentials held, and approvals that will become one.
    fn held(&self, now: u64) -> usize {
        let live = self
            .credentials
            .iter()
            .filter(|c| c.expires_ms.is_none_or(|end| now < end))
            .count();
        live + self
            .registrations
            .iter()
            .filter(|r| r.state_at(now) == State::Approved)
            .count()
    }

    /// A registration is kept for as long again as it lived, so a late poll still learns it expired; then it goes.
    fn forget_old(&mut self, now: u64) {
        self.registrations
            .retain(|r| now < r.expires_ms + EXPIRES_IN_S * 1000 && r.state != State::Redeemed);
    }
}

fn waits(r: &Registration, now: u64) -> bool {
    matches!(r.state_at(now), State::Pending | State::Armed { .. })
}

fn user_code(random: &mut dyn FnMut(&mut [u8])) -> String {
    let mut letters = String::with_capacity(9);
    while letters.len() < 9 {
        let mut b = [0u8; 1];
        random(&mut b);
        // 240 is the largest multiple of 20 below 256: every letter equally likely.
        if b[0] < 240 {
            if letters.len() == 4 {
                letters.push('-');
            }
            letters.push(USER_CODE_LETTERS[(b[0] % 20) as usize] as char);
        }
    }
    letters
}

fn normal(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

pub fn is_connection(c: &str) -> bool {
    let b = c.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || *x == b'-')
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
