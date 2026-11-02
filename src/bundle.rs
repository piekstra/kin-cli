//! Everything `kin` keeps secret, as one keychain item.
//!
//! The password, the portal session and a login parked while it waits for
//! its one-time code are one JSON item under service `piekstra.kin`, account
//! = the Kin login email. One item, not three, because on macOS every item a
//! freshly built binary reads is its own permission prompt (SPEC §1.7).
//!
//! Two views over the bundle plug it into the shared crates:
//! [`PasswordView`] is the keychain source of the `SecretResolver`, and
//! [`ParkedView`] is the `ParkingSlot` the one-time-code login parks in.

use std::cell::RefCell;
use std::collections::BTreeMap;

use pk_cli_auth::otp::{ParkedLogin, ParkingSlot};
use pk_cli_core::CliError;
use pk_cli_secrets::{CredentialStore, KeychainRead, Secret};
use serde::{Deserialize, Serialize};

use crate::jar::Jar;

/// The keychain item's contents.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Bundle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jar: Option<Jar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked: Option<ParkedLogin>,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("jar", &self.jar)
            .field("parked", &self.parked)
            .finish()
    }
}

impl Bundle {
    pub fn is_empty(&self) -> bool {
        self.password.is_none() && self.jar.is_none() && self.parked.is_none()
    }

    pub fn has_session(&self) -> bool {
        self.jar.as_ref().is_some_and(Jar::has_session)
    }
}

/// Where bundles live: the OS keychain in the binary, a map in tests.
pub trait Vault {
    fn service(&self) -> &str;
    fn load(&self, account: &str) -> Result<Option<Bundle>, CliError>;
    fn save(&self, account: &str, bundle: &Bundle) -> Result<(), CliError>;
    fn delete(&self, account: &str) -> Result<bool, CliError>;
}

impl Vault for CredentialStore {
    fn service(&self) -> &str {
        CredentialStore::service(self)
    }
    fn load(&self, account: &str) -> Result<Option<Bundle>, CliError> {
        self.get_json(account)
    }
    fn save(&self, account: &str, bundle: &Bundle) -> Result<(), CliError> {
        self.set_json(account, bundle)
    }
    fn delete(&self, account: &str) -> Result<bool, CliError> {
        CredentialStore::delete(self, account)
    }
}

/// An in-memory vault, for tests: the real keychain is never touched by
/// `cargo test` (every read from an ad-hoc-signed test binary is a prompt).
#[derive(Default)]
pub struct MemVault {
    items: RefCell<BTreeMap<String, String>>,
}

impl Vault for MemVault {
    fn service(&self) -> &str {
        "piekstra.kin.test"
    }
    fn load(&self, account: &str) -> Result<Option<Bundle>, CliError> {
        Ok(self
            .items
            .borrow()
            .get(account)
            .and_then(|raw| serde_json::from_str(raw).ok()))
    }
    fn save(&self, account: &str, bundle: &Bundle) -> Result<(), CliError> {
        let raw = serde_json::to_string(bundle)
            .map_err(|e| CliError::Other(format!("serializing the bundle: {e}")))?;
        self.items.borrow_mut().insert(account.to_string(), raw);
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<bool, CliError> {
        Ok(self.items.borrow_mut().remove(account).is_some())
    }
}

/// Read-modify-write of one account's bundle.
pub fn update(
    vault: &dyn Vault,
    account: &str,
    change: impl FnOnce(&mut Bundle),
) -> Result<(), CliError> {
    let mut bundle = vault.load(account)?.unwrap_or_default();
    change(&mut bundle);
    if bundle.is_empty() {
        vault.delete(account)?;
        Ok(())
    } else {
        vault.save(account, &bundle)
    }
}

/// The bundle's password as the resolver's keychain source.
pub struct PasswordView<'a> {
    pub vault: &'a dyn Vault,
}

impl KeychainRead for PasswordView<'_> {
    fn service(&self) -> &str {
        self.vault.service()
    }
    fn get(&self, account: &str) -> Result<Option<Secret>, CliError> {
        Ok(self
            .vault
            .load(account)?
            .and_then(|b| b.password)
            .map(Secret::new))
    }
}

/// The bundle's parked login as the one-time-code flow's parking slot.
pub struct ParkedView<'a> {
    pub vault: &'a dyn Vault,
    pub account: &'a str,
}

impl ParkingSlot for ParkedView<'_> {
    fn load(&self) -> Result<Option<ParkedLogin>, CliError> {
        Ok(self.vault.load(self.account)?.and_then(|b| b.parked))
    }
    fn save(&self, parked: &ParkedLogin) -> Result<(), CliError> {
        update(self.vault, self.account, |b| {
            b.parked = Some(parked.clone())
        })
    }
    fn clear(&self) -> Result<(), CliError> {
        update(self.vault, self.account, |b| b.parked = None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parked_view_meets_the_slot_contract() {
        let vault = MemVault::default();
        let slot = ParkedView {
            vault: &vault,
            account: "pat@example.com",
        };
        pk_cli_auth::otp::check_slot_contract(&slot);
    }

    #[test]
    fn parking_keeps_the_password_and_session() {
        let vault = MemVault::default();
        let acct = "pat@example.com";
        let mut jar = Jar::new();
        jar.set_cookie("_bright-policy_session=dummy");
        update(&vault, acct, |b| {
            b.password = Some("dummy-password".into());
            b.jar = Some(jar.clone());
        })
        .unwrap();
        let slot = ParkedView {
            vault: &vault,
            account: acct,
        };
        slot.save(&ParkedLogin::new("in-flight", 100)).unwrap();
        slot.clear().unwrap();
        let b = vault.load(acct).unwrap().unwrap();
        assert_eq!(b.password.as_deref(), Some("dummy-password"));
        assert_eq!(b.jar, Some(jar));
        assert!(b.parked.is_none());
    }

    #[test]
    fn an_emptied_bundle_removes_the_item() {
        let vault = MemVault::default();
        update(&vault, "a", |b| b.password = Some("x".into())).unwrap();
        update(&vault, "a", |b| b.password = None).unwrap();
        assert!(vault.load("a").unwrap().is_none());
    }

    #[test]
    fn the_password_view_reads_only_the_password() {
        let vault = MemVault::default();
        update(&vault, "a", |b| b.password = Some("dummy-password".into())).unwrap();
        let view = PasswordView { vault: &vault };
        assert_eq!(view.get("a").unwrap().unwrap().expose(), "dummy-password");
        assert!(view.get("missing").unwrap().is_none());
    }

    #[test]
    fn debug_never_shows_the_password() {
        let b = Bundle {
            password: Some("dummy-password".into()),
            ..Bundle::default()
        };
        assert!(!format!("{b:?}").contains("dummy-password"));
    }
}
