//! API keys. On Windows and macOS they go to the system credential store
//! (Credential Manager / Keychain); elsewhere, or when that store is not
//! available, to `secrets.json` in the app data folder, readable only by the
//! current user. Keys never leave this module except to call a model.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{Error, Result};

pub struct SecretStore {
    file: PathBuf,
    /// Use the system credential store when it is available.
    native: bool,
    lock: Mutex<()>,
}

impl SecretStore {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            file: data_dir.join("secrets.json"),
            native: true,
            lock: Mutex::new(()),
        }
    }

    /// Keeps keys only in `secrets.json`, e.g. for tests, which must not
    /// touch (or wait on) the user's keychain.
    pub fn file_only(data_dir: &std::path::Path) -> Self {
        Self {
            native: false,
            ..Self::new(data_dir)
        }
    }

    pub fn set(&self, name: &str, secret: &str) -> Result<()> {
        if self.native && native::set(name, secret).is_ok() {
            // A stale copy in the file would otherwise win after a store reset.
            return self.file_update(|m| {
                m.remove(name);
            });
        }
        self.file_update(|m| {
            m.insert(name.to_string(), secret.to_string());
        })
    }

    pub fn get(&self, name: &str) -> Result<Option<String>> {
        if let Some(s) = self.native.then(|| native::get(name)).flatten() {
            return Ok(Some(s));
        }
        Ok(self.file_read()?.remove(name))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        if self.native {
            native::delete(name);
        }
        self.file_update(|m| {
            m.remove(name);
        })
    }

    fn file_read(&self) -> Result<BTreeMap<String, String>> {
        match std::fs::read(&self.file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| Error::Storage(format!("密钥文件损坏: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn file_update(&self, f: impl FnOnce(&mut BTreeMap<String, String>)) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut map = self.file_read()?;
        let before = map.len();
        f(&mut map);
        if map.is_empty() && before == 0 && !self.file.exists() {
            return Ok(());
        }
        let data = serde_json::to_vec_pretty(&map).expect("map serializes");
        let tmp = self.file.with_extension("json.tmp");
        write_private(&tmp, &data)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }
}

fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(any(windows, target_os = "macos"))]
mod native {
    const SERVICE: &str = "AutoPassDoc";

    fn entry(name: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, name).ok()
    }

    pub fn set(name: &str, secret: &str) -> Result<(), ()> {
        entry(name).ok_or(())?.set_password(secret).map_err(|_| ())
    }

    pub fn get(name: &str) -> Option<String> {
        entry(name)?.get_password().ok()
    }

    pub fn delete(name: &str) {
        if let Some(e) = entry(name) {
            let _ = e.delete_credential();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod native {
    pub fn set(_: &str, _: &str) -> Result<(), ()> {
        Err(())
    }

    pub fn get(_: &str) -> Option<String> {
        None
    }

    pub fn delete(_: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_reads_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::file_only(dir.path());
        assert_eq!(store.get("provider:1").unwrap(), None);
        store.set("provider:1", "sk-test").unwrap();
        assert_eq!(store.get("provider:1").unwrap().as_deref(), Some("sk-test"));
        store.delete("provider:1").unwrap();
        assert_eq!(store.get("provider:1").unwrap(), None);
    }
}
