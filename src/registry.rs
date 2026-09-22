//! The TOML-backed port registry.
//!
//! ```toml
//! [settings]
//! start = 4001
//!
//! [ports."/home/me/src/app"]
//! main = 4001
//! debug = 4002
//! ```
//!
//! Directories are stored as canonical absolute paths. Ports are unique across
//! the whole file, so two projects never share a number.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

/// Environment variable that overrides the registry location.
pub const CONFIG_ENV: &str = "PORTEZ_CONFIG";
/// Lowest port handed out when the registry has no `settings.start`.
pub const DEFAULT_START: u16 = 4001;
/// Name used when a request does not supply one.
pub const DEFAULT_NAME: &str = "main";

/// One `(directory, name) → port` row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Assignment {
    pub dir: String,
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("cannot determine a registry path; set {CONFIG_ENV} or HOME")]
    NoPath,
    #[error("cannot read registry {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("cannot write registry {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("cannot lock registry {path}: {source}")]
    Lock { path: PathBuf, source: io::Error },
    #[error("registry {path} is not valid TOML: {source}")]
    Parse {
        path: PathBuf,
        source: toml_edit::TomlError,
    },
    #[error("registry {path}: {message}")]
    Malformed { path: PathBuf, message: String },
    #[error("invalid name {0:?}: use letters, digits, '-' and '_'")]
    InvalidName(String),
    #[error("no free port at or above {start}")]
    Exhausted { start: u16 },
    #[error("no port registered for {name:?} in {dir}")]
    NotRegistered { dir: String, name: String },
}

type Result<T> = std::result::Result<T, RegistryError>;

/// An open registry document and the path it came from.
#[derive(Debug)]
pub struct Registry {
    path: PathBuf,
    doc: DocumentMut,
    dirty: bool,
}

impl Registry {
    /// Resolve the registry path: `$PORTEZ_CONFIG`, else
    /// `$XDG_CONFIG_HOME/portez/ports.toml`, else `~/.config/portez/ports.toml`.
    ///
    /// # Errors
    /// Fails when neither the override nor a home directory is available.
    pub fn default_path() -> Result<PathBuf> {
        if let Some(explicit) = std::env::var_os(CONFIG_ENV).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(explicit));
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".config")))
            .ok_or(RegistryError::NoPath)?;
        Ok(base.join("portez").join("ports.toml"))
    }

    /// Load the registry at `path`. A missing file yields an empty registry.
    ///
    /// # Errors
    /// Fails when the file exists but cannot be read, is not TOML, or has an
    /// invalid shape (non-table sections, non-port values, duplicate ports).
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(RegistryError::Read { path, source }),
        };
        let doc = text
            .parse::<DocumentMut>()
            .map_err(|source| RegistryError::Parse {
                path: path.clone(),
                source,
            })?;
        let registry = Self {
            path,
            doc,
            dirty: false,
        };
        registry.validate()?;
        Ok(registry)
    }

    /// Run `f` against the registry while holding an exclusive lock, then
    /// save if anything changed. The lock file sits beside the registry.
    ///
    /// # Errors
    /// Fails when the lock cannot be taken, the registry cannot be loaded or
    /// saved, or `f` itself fails.
    pub fn edit<T>(path: impl Into<PathBuf>, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let path = path.into();
        let _guard = Lock::exclusive(&path)?;
        let mut registry = Self::load(&path)?;
        let out = f(&mut registry)?;
        if registry.dirty {
            registry.save()?;
        }
        Ok(out)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lowest port this registry will hand out.
    #[must_use]
    pub fn start(&self) -> u16 {
        self.doc
            .get("settings")
            .and_then(Item::as_table_like)
            .and_then(|settings| settings.get("start"))
            .and_then(Item::as_integer)
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(DEFAULT_START)
    }

    /// The port for `(dir, name)`, if registered.
    #[must_use]
    pub fn lookup(&self, dir: &str, name: &str) -> Option<u16> {
        self.ports()?
            .get(dir)?
            .as_table_like()?
            .get(name)?
            .as_integer()
            .and_then(|n| u16::try_from(n).ok())
    }

    /// Every assignment, ordered by port.
    #[must_use]
    pub fn assignments(&self) -> Vec<Assignment> {
        let mut rows = Vec::new();
        if let Some(ports) = self.ports() {
            for (dir, item) in ports.iter() {
                let Some(names) = item.as_table_like() else {
                    continue;
                };
                for (name, port) in names.iter() {
                    if let Some(port) = port.as_integer().and_then(|n| u16::try_from(n).ok()) {
                        rows.push(Assignment {
                            dir: dir.to_owned(),
                            name: name.to_owned(),
                            port,
                        });
                    }
                }
            }
        }
        rows.sort_by(|a, b| a.port.cmp(&b.port).then_with(|| a.dir.cmp(&b.dir)));
        rows
    }

    /// Return the port for `(dir, name)`, allocating and recording one if
    /// this is the first request. The bool is `true` when a new port was
    /// registered by this call.
    ///
    /// # Errors
    /// Fails on an invalid name or when every port from `start` up is taken.
    pub fn assign(&mut self, dir: &str, name: &str) -> Result<(u16, bool)> {
        validate_name(name)?;
        if let Some(port) = self.lookup(dir, name) {
            return Ok((port, false));
        }
        let port = self.next_free()?;
        let ports = self.ports_mut();
        let entry = ports
            .entry(dir)
            .or_insert_with(|| Item::Table(Table::new()));
        let Some(names) = entry.as_table_like_mut() else {
            return Err(RegistryError::Malformed {
                path: self.path.clone(),
                message: format!("ports.{dir:?} is not a table"),
            });
        };
        names.insert(name, value(i64::from(port)));
        self.dirty = true;
        Ok((port, true))
    }

    /// Remove `(dir, name)`, or every name under `dir` when `name` is `None`.
    /// Returns what was removed.
    ///
    /// # Errors
    /// Fails when nothing matches.
    pub fn remove(&mut self, dir: &str, name: Option<&str>) -> Result<Vec<Assignment>> {
        let before = self.assignments();
        let removed: Vec<Assignment> = before
            .into_iter()
            .filter(|a| a.dir == dir && name.is_none_or(|n| n == a.name))
            .collect();
        if removed.is_empty() {
            return Err(RegistryError::NotRegistered {
                dir: dir.to_owned(),
                name: name.unwrap_or("*").to_owned(),
            });
        }
        let ports = self.ports_mut();
        match name {
            None => {
                ports.remove(dir);
            }
            Some(name) => {
                if let Some(names) = ports.get_mut(dir).and_then(Item::as_table_like_mut) {
                    names.remove(name);
                    if names.is_empty() {
                        ports.remove(dir);
                    }
                }
            }
        }
        self.dirty = true;
        Ok(removed)
    }

    /// Write the document back atomically (temp file + rename).
    ///
    /// # Errors
    /// Fails when the directory or file cannot be written.
    pub fn save(&self) -> Result<()> {
        let write_err = |source| RegistryError::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(write_err)?;
        }
        let tmp = self
            .path
            .with_extension(format!("toml.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = File::create(&tmp)?;
            file.write_all(self.doc.to_string().as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map_err(write_err)
    }

    fn next_free(&self) -> Result<u16> {
        let start = self.start();
        let used: BTreeSet<u16> = self.assignments().into_iter().map(|a| a.port).collect();
        (start..=u16::MAX)
            .find(|port| !used.contains(port))
            .ok_or(RegistryError::Exhausted { start })
    }

    fn ports(&self) -> Option<&dyn toml_edit::TableLike> {
        self.doc.get("ports").and_then(Item::as_table_like)
    }

    fn ports_mut(&mut self) -> &mut dyn toml_edit::TableLike {
        let item = self.doc.entry("ports").or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        });
        item.as_table_like_mut()
            .expect("ports was validated to be a table")
    }

    fn validate(&self) -> Result<()> {
        let malformed = |message: String| RegistryError::Malformed {
            path: self.path.clone(),
            message,
        };
        if let Some(settings) = self.doc.get("settings") {
            let table = settings
                .as_table_like()
                .ok_or_else(|| malformed("settings is not a table".into()))?;
            if let Some(start) = table.get("start") {
                let ok = start
                    .as_integer()
                    .is_some_and(|n| (1..=i64::from(u16::MAX)).contains(&n));
                if !ok {
                    return Err(malformed(
                        "settings.start must be an integer 1..=65535".into(),
                    ));
                }
            }
        }
        let Some(ports) = self.doc.get("ports") else {
            return Ok(());
        };
        let ports = ports
            .as_table_like()
            .ok_or_else(|| malformed("ports is not a table".into()))?;
        let mut seen = BTreeSet::new();
        for (dir, item) in ports.iter() {
            let names = item
                .as_table_like()
                .ok_or_else(|| malformed(format!("ports.{dir:?} is not a table")))?;
            for (name, port) in names.iter() {
                let port = port
                    .as_integer()
                    .and_then(|n| u16::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        malformed(format!("ports.{dir:?}.{name} is not a port number"))
                    })?;
                if !seen.insert(port) {
                    return Err(malformed(format!("port {port} is assigned more than once")));
                }
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(RegistryError::InvalidName(name.to_owned()))
    }
}

/// Canonical absolute form of a project directory, used as the registry key.
///
/// # Errors
/// Fails when `dir` does not exist or cannot be resolved.
pub fn canonical_dir(dir: &Path) -> io::Result<String> {
    let path = fs::canonicalize(dir)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Exclusive advisory lock on `<registry>.lock`, released on drop.
struct Lock {
    file: File,
}

impl Lock {
    fn exclusive(registry: &Path) -> Result<Self> {
        let path = registry.with_extension("toml.lock");
        let lock_err = |source| RegistryError::Lock {
            path: path.clone(),
            source,
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(lock_err)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(lock_err)?;
        file.lock().map_err(lock_err)?;
        Ok(Self { file })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_registry() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ports.toml");
        (dir, path)
    }

    #[test]
    fn allocates_sequentially_and_is_idempotent() {
        let (_dir, path) = temp_registry();
        let first = Registry::edit(&path, |r| r.assign("/a", "main")).unwrap();
        let second = Registry::edit(&path, |r| r.assign("/b", "main")).unwrap();
        let again = Registry::edit(&path, |r| r.assign("/a", "main")).unwrap();
        assert_eq!(first, (4001, true));
        assert_eq!(second, (4002, true));
        assert_eq!(again, (4001, false));
    }

    #[test]
    fn reuses_gaps_after_removal() {
        let (_dir, path) = temp_registry();
        Registry::edit(&path, |r| {
            r.assign("/a", "main")?;
            r.assign("/a", "debug")?;
            r.assign("/b", "main")
        })
        .unwrap();
        let removed = Registry::edit(&path, |r| r.remove("/a", Some("debug"))).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].port, 4002);
        let fresh = Registry::edit(&path, |r| r.assign("/c", "main")).unwrap();
        assert_eq!(fresh, (4002, true));
    }

    #[test]
    fn honours_settings_start_and_preserves_comments() {
        let (_dir, path) = temp_registry();
        fs::write(
            &path,
            "# keep me\n[settings]\nstart = 5000\n\n[ports.\"/x\"]\nmain = 5000 # taken\n",
        )
        .unwrap();
        let port = Registry::edit(&path, |r| r.assign("/y", "main")).unwrap();
        assert_eq!(port, (5001, true));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"), "{text}");
        assert!(text.contains("main = 5000 # taken"), "{text}");
        assert!(text.contains("[ports.\"/y\"]\nmain = 5001"), "{text}");
    }

    #[test]
    fn rejects_bad_names_and_duplicate_ports() {
        let (_dir, path) = temp_registry();
        let err = Registry::edit(&path, |r| r.assign("/a", "no spaces")).unwrap_err();
        assert!(matches!(err, RegistryError::InvalidName(_)));
        fs::write(
            &path,
            "[ports.\"/a\"]\nmain = 4001\n[ports.\"/b\"]\nmain = 4001\n",
        )
        .unwrap();
        let err = Registry::load(&path).unwrap_err();
        assert!(matches!(err, RegistryError::Malformed { .. }), "{err}");
    }

    #[test]
    fn remove_whole_dir_and_missing_entries() {
        let (_dir, path) = temp_registry();
        Registry::edit(&path, |r| {
            r.assign("/a", "main")?;
            r.assign("/a", "debug")
        })
        .unwrap();
        let removed = Registry::edit(&path, |r| r.remove("/a", None)).unwrap();
        assert_eq!(removed.len(), 2);
        let err = Registry::edit(&path, |r| r.remove("/a", None)).unwrap_err();
        assert!(matches!(err, RegistryError::NotRegistered { .. }));
        assert!(Registry::load(&path).unwrap().assignments().is_empty());
    }
}
