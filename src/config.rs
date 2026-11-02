//! Non-secret settings, `${XDG_CONFIG_HOME:-~/.config}/kin/config.json`
//! (`pk-cli-config`).

use pk_cli_core::CliError;
use pk_cli_secrets::{OpRef, SourceOrder};
use serde::{Deserialize, Serialize};

pub const KEYS: &str = "username, policy, otp_channel, mailbox, op_ref, secret_sources";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// The Kin login email; also the keychain account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Default policy number when the account has more than one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    /// Where Kin sends the one-time code: `email` (default) or `text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otp_channel: Option<String>,
    /// Where to read an emailed code: `gro` (gro's active profile),
    /// `gro:<profile>`, or `none` (default: prompt, or `--code`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox: Option<String>,
    /// Where 1Password keeps the Kin password (`op://vault/item/field`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_ref: Option<OpRef>,
    /// Password source order, e.g. `op,keychain` (default `env,keychain,op`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_sources: Option<SourceOrder>,
}

impl Config {
    pub fn channel(&self) -> &str {
        self.otp_channel.as_deref().unwrap_or("email")
    }
}

pub fn validate_channel(v: &str) -> Result<String, CliError> {
    match v {
        "email" | "text" => Ok(v.to_string()),
        other => Err(CliError::Usage(format!(
            "`{other}` is not a code channel (expected `email` or `text`)"
        ))),
    }
}

/// What `mailbox` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxSetting {
    None,
    /// gro, with its active profile or the named one.
    Gro(Option<String>),
}

pub fn parse_mailbox(v: &str) -> Result<MailboxSetting, CliError> {
    match v {
        "none" => Ok(MailboxSetting::None),
        "gro" => Ok(MailboxSetting::Gro(None)),
        _ => {
            let profile = v.strip_prefix("gro:").ok_or_else(|| {
                CliError::Usage(format!(
                    "`{v}` is not a mailbox (expected `none`, `gro`, or `gro:<profile>`)"
                ))
            })?;
            let ok = profile
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
                && profile
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if !ok {
                return Err(CliError::Usage(format!(
                    "`{profile}` is not a gro profile name"
                )));
            }
            Ok(MailboxSetting::Gro(Some(profile.to_string())))
        }
    }
}

pub fn validate_email(v: &str) -> Result<String, CliError> {
    let v = v.trim();
    let ok = v.len() <= 254
        && v.split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
        && !v.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(v.to_string())
    } else {
        Err(CliError::Usage(format!("`{v}` is not an email address")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_settings_parse() {
        assert_eq!(parse_mailbox("none").unwrap(), MailboxSetting::None);
        assert_eq!(parse_mailbox("gro").unwrap(), MailboxSetting::Gro(None));
        assert_eq!(
            parse_mailbox("gro:work").unwrap(),
            MailboxSetting::Gro(Some("work".into()))
        );
        for bad in ["gmail", "gro:", "gro:a b", "gro:--ref"] {
            assert_eq!(parse_mailbox(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }

    #[test]
    fn channels_and_emails_validate() {
        assert!(validate_channel("email").is_ok());
        assert!(validate_channel("sms").is_err());
        assert!(validate_email("pat@example.com").is_ok());
        for bad in [
            "pat",
            "pat@",
            "@example.com",
            "pat@example",
            "pat @example.com",
        ] {
            assert!(validate_email(bad).is_err(), "{bad}");
        }
    }
}
