//! The page the person opens from the agent's link: the robot's own, where they see who asks, approve by pressing the
//! robot's button, or deny. Everything the agent sent is escaped: it is the agent's text, shown to a person.

use crate::signin::{Registration, State, ARMED_FOR_S};

/// The page for one code, or the form to type one when there is none.
pub fn approve(registration: Option<&Registration>, typed: Option<&str>, now: u64, notice: Option<&str>) -> String {
    let body = match (registration, typed) {
        (Some(r), _) => about(r, now),
        (None, Some(code)) => format!(
            "<p>No sign-in waits under the code <b>{}</b>. Check it against what your agent shows, or ask it to \
             sign in again.</p>{}",
            escape(code),
            ask_code()
        ),
        (None, None) => ask_code(),
    };
    let notice = notice
        .map(|n| format!("<p class=notice>{}</p>", escape(n)))
        .unwrap_or_default();
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content=\"width=device-width, \
         initial-scale=1\"><title>Sign an agent in to the robot</title><style>body{{font:16px system-ui,sans-serif;\
         max-width:32em;margin:2em auto;padding:0 1em}}code,b.code{{font-size:1.4em;letter-spacing:.1em}}\
         button{{font-size:1em;padding:.5em 1em;margin-right:.5em}}.notice{{background:#fff3cd;padding:.5em}}\
         </style></head><body><h1>Sign an agent in to the robot</h1>{notice}{body}</body></html>"
    )
}

fn about(r: &Registration, now: u64) -> String {
    let who = format!(
        "<p><b>{}</b>{} asks to use the robot{}.</p><p>Code: <b class=code>{}</b>. Check that your agent shows the \
         same.</p>",
        escape(r.agent_label.as_deref().unwrap_or(&r.client_id)),
        r.agent_label
            .as_ref()
            .map(|_| format!(" ({})", escape(&r.client_id)))
            .unwrap_or_default(),
        r.connection
            .as_ref()
            .map(|c| format!(" as <b>{}</b>", escape(c)))
            .unwrap_or_default(),
        escape(&r.user_code)
    );
    let code = escape(&r.user_code);
    let what = match r.state_at(now) {
        State::Pending => format!(
            "<form method=post action=approve><input type=hidden name=code value=\"{code}\"><button name=action \
             value=approve>Approve</button><button name=action value=deny>Deny</button></form><p>Approve, then \
             press the robot's BOOT button within {ARMED_FOR_S} seconds: the press is what approves.</p>"
        ),
        State::Armed { until_ms } => format!(
            "<p><b>Press the robot's BOOT button now</b>: {} seconds left. This page shows the outcome when it \
             reloads.</p><meta http-equiv=refresh content=\"3; url=approve?code={code}\">",
            until_ms.saturating_sub(now).div_ceil(1000)
        ),
        State::Approved | State::Redeemed => "<p>Approved. The agent collects its credential by itself; you can \
                                               close this page.</p>"
            .into(),
        State::Denied => "<p>Denied. The agent is told so.</p>".into(),
        State::Expired => "<p>This code is past its time. Ask the agent to sign in again.</p>".into(),
    };
    who + &what
}

fn ask_code() -> String {
    "<form method=get action=approve><label>The code your agent shows: <input name=code autocomplete=off \
     autocapitalize=characters required></label> <button>Look it up</button></form>"
        .into()
}

pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&#39;".into(),
            c => c.to_string(),
        })
        .collect()
}
