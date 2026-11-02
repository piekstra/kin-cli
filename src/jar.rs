//! The portal session: the cookies `app.kin.com` set, kept between runs.
//!
//! reqwest's cookie store lives and dies with one process, but a Kin session
//! has to outlive it: `auth login` parks a login waiting for its one-time
//! code, `auth login --code` finishes it in a later run, and every read
//! replays the stored session. So the jar is a plain map that serializes into
//! the keychain bundle.
//!
//! The portal is one host and one session cookie (`_bright-policy_session`,
//! re-issued on every response), so this is deliberately not RFC 6265: no
//! domains, paths or expiry. It records what `app.kin.com` set and sends it
//! back to `app.kin.com`.

use std::collections::BTreeMap;

use reqwest::header::{HeaderMap, SET_COOKIE};
use serde::{Deserialize, Serialize};

/// The cookie that carries the signed-in session.
pub const SESSION_COOKIE: &str = "_bright-policy_session";

/// Cookie name → value. `BTreeMap` so the serialized form is stable.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jar {
    cookies: BTreeMap<String, String>,
}

impl std::fmt::Debug for Jar {
    /// Names only: values are session credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jar")
            .field("cookies", &self.cookies.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Jar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one `Set-Cookie` value. An empty value is a deletion: Rails
    /// clears a cookie by setting it empty with a past expiry, and replaying
    /// an empty cookie is not the same as not sending it.
    pub fn set_cookie(&mut self, header: &str) {
        let pair = header.split(';').next().unwrap_or("").trim();
        let Some((name, value)) = pair.split_once('=') else {
            return;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() {
            return;
        }
        if value.is_empty() {
            self.cookies.remove(name);
        } else {
            self.cookies.insert(name.to_string(), value.to_string());
        }
    }

    /// Record every `Set-Cookie` header of a response.
    pub fn absorb(&mut self, headers: &HeaderMap) {
        for value in headers.get_all(SET_COOKIE) {
            if let Ok(v) = value.to_str() {
                self.set_cookie(v);
            }
        }
    }

    /// The `Cookie` request header, or `None` for an empty jar.
    pub fn header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    /// Whether the session cookie is present (not whether it is still valid:
    /// only Kin knows that).
    pub fn has_session(&self) -> bool {
        self.cookies.contains_key(SESSION_COOKIE)
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a map of strings serializes")
    }

    pub fn from_json(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_replays_cookies() {
        let mut jar = Jar::new();
        jar.set_cookie("_bright-policy_session=abc123; path=/; secure; httponly; samesite=lax");
        jar.set_cookie("other=1; path=/");
        assert!(jar.has_session());
        assert_eq!(
            jar.header().unwrap(),
            "_bright-policy_session=abc123; other=1"
        );
    }

    #[test]
    fn a_rotated_session_replaces_the_old_value() {
        let mut jar = Jar::new();
        jar.set_cookie("_bright-policy_session=first; path=/");
        jar.set_cookie("_bright-policy_session=second; path=/");
        assert_eq!(jar.header().unwrap(), "_bright-policy_session=second");
    }

    #[test]
    fn an_empty_value_deletes_the_cookie() {
        let mut jar = Jar::new();
        jar.set_cookie("_bright-policy_session=abc; path=/");
        jar.set_cookie("_bright-policy_session=; path=/; max-age=0");
        assert!(!jar.has_session());
        assert!(jar.header().is_none());
    }

    #[test]
    fn malformed_headers_are_ignored() {
        let mut jar = Jar::new();
        jar.set_cookie("no-equals-sign");
        jar.set_cookie("=value-without-name");
        assert!(jar.is_empty());
    }

    #[test]
    fn round_trips_through_json_and_debug_hides_values() {
        let mut jar = Jar::new();
        jar.set_cookie("_bright-policy_session=secret-value");
        let back = Jar::from_json(&jar.to_json()).unwrap();
        assert_eq!(back, jar);
        assert!(!format!("{jar:?}").contains("secret-value"));
    }
}
