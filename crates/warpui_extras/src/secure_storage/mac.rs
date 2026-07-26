//! Implementations of the [`SecureStorage`] service for the macOS platform.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::anyhow;
use security_framework::os::macos::{
    keychain::SecKeychain, keychain_item::SecKeychainItem, passwords::SecKeychainItemPassword,
};

use super::Error;

const KEYCHAIN_OPERATION_TIMEOUT: Duration = Duration::from_secs(3);

/// Implementation of the SecureStorage service using macOS Security
/// framework keychains.
pub struct SecureStorage {
    /// The name of the service under which to store the values.
    service_name: String,
}

impl SecureStorage {
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_owned(),
        }
    }
}

impl super::SecureStorage for SecureStorage {
    fn write_value(&self, key: &str, value: &str) -> Result<(), Error> {
        let service_name = self.service_name.clone();
        let key = key.to_owned();
        let value = value.to_owned();
        run_keychain_operation("write", key.clone(), move || {
            let keychain = SecKeychain::default()?;
            keychain
                .set_generic_password(service_name.as_str(), key.as_str(), value.as_bytes())
                .map_err(Into::into)
        })
    }

    fn read_value(&self, key: &str) -> Result<String, Error> {
        let service_name = self.service_name.clone();
        let key = key.to_owned();
        run_keychain_operation("read", key.clone(), move || {
            let (password, _) = get_password_item(service_name.as_str(), key.as_str())?;
            String::from_utf8(password.as_ref().to_vec())
                .map_err(|err| Error::DecodeError(err.utf8_error()))
        })
    }

    fn remove_value(&self, key: &str) -> Result<(), Error> {
        let service_name = self.service_name.clone();
        let key = key.to_owned();
        run_keychain_operation("remove", key.clone(), move || {
            let (_, item) = get_password_item(service_name.as_str(), key.as_str())?;
            item.delete();
            Ok(())
        })
    }
}

fn get_password_item(
    service_name: &str,
    key: &str,
) -> Result<(SecKeychainItemPassword, SecKeychainItem), Error> {
    let keychain = SecKeychain::default()?;
    keychain
        .find_generic_password(service_name, key)
        .map_err(|_| Error::NotFound)
}

fn run_keychain_operation<T>(
    operation: &'static str,
    key: String,
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error>
where
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name(format!("secure-storage-{operation}"))
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|err| {
            Error::Unknown(anyhow!(
                "failed to spawn macOS Keychain {operation} worker for {key}: {err}"
            ))
        })?;

    match rx.recv_timeout(KEYCHAIN_OPERATION_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Unknown(anyhow!(
            "macOS Keychain {operation} timed out for {key} after {:?}",
            KEYCHAIN_OPERATION_TIMEOUT
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Unknown(anyhow!(
            "macOS Keychain {operation} worker exited without a result for {key}"
        ))),
    }
}

impl From<security_framework::base::Error> for Error {
    fn from(value: security_framework::base::Error) -> Self {
        Error::Unknown(anyhow!(value))
    }
}
