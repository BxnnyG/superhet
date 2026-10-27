//! Installed plugins on disk: `<dir>/<plugin id>/<version>/plugin.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use superhet_plugin_protocol::Package;

/// Largest plugin tree accepted by `install` (bytes).
const MAX_INSTALL_BYTES: u64 = 256 * 1024 * 1024;
/// Deepest directory nesting copied by `install`.
const MAX_DEPTH: usize = 16;

/// Registry failures.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The source is not a usable plugin directory.
    #[error("{0}")]
    Invalid(String),
    /// The same id and version is already installed.
    #[error("{0} is already installed")]
    Exists(String),
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// All installed plugins; the highest version of an id is the active one.
#[derive(Debug)]
pub struct Registry {
    dir: PathBuf,
    // id -> version -> package
    plugins: RwLock<BTreeMap<String, BTreeMap<semver::Version, Arc<Package>>>>,
}

impl Registry {
    /// Scan `dir` (created when missing). Broken plugin directories are logged and skipped.
    ///
    /// # Errors
    /// Fails when `dir` cannot be created or read.
    pub fn open(dir: &Path) -> Result<Self, RegistryError> {
        std::fs::create_dir_all(dir)?;
        let registry = Self {
            dir: dir.to_path_buf(),
            plugins: RwLock::default(),
        };
        for id_dir in std::fs::read_dir(dir)?.flatten() {
            if !id_dir.path().is_dir() || id_dir.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            for version_dir in std::fs::read_dir(id_dir.path())?.flatten() {
                match registry.load_dir(&version_dir.path()) {
                    Ok(()) => {}
                    Err(err) => tracing::warn!(
                        path = %version_dir.path().display(),
                        %err,
                        "skipping unusable plugin directory"
                    ),
                }
            }
        }
        Ok(registry)
    }

    fn load_dir(&self, path: &Path) -> Result<(), RegistryError> {
        let package = Package::load(path).map_err(|err| RegistryError::Invalid(err.to_string()))?;
        let meta = &package.manifest.plugin;
        // The directory name must match the manifest, otherwise a copied tree could shadow
        // another plugin.
        let expected = self.dir.join(&meta.id).join(&meta.version);
        if path != expected {
            return Err(RegistryError::Invalid(format!(
                "directory should be {}",
                expected.display()
            )));
        }
        let version = semver::Version::parse(&meta.version)
            .map_err(|err| RegistryError::Invalid(err.to_string()))?;
        let id = meta.id.clone();
        self.plugins
            .write()
            .map_err(|_| RegistryError::Invalid("registry lock poisoned".into()))?
            .entry(id)
            .or_default()
            .insert(version, Arc::new(package));
        Ok(())
    }

    /// The active (highest) version of every plugin.
    #[must_use]
    pub fn list(&self) -> Vec<Arc<Package>> {
        self.plugins
            .read()
            .map(|map| {
                map.values()
                    .filter_map(|versions| versions.values().next_back().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The active version of `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<Package>> {
        self.plugins
            .read()
            .ok()?
            .get(id)?
            .values()
            .next_back()
            .cloned()
    }

    /// Install the plugin directory at `source` (validated first, copied to
    /// `<dir>/<id>/<version>`). Symbolic links are refused. Build commands are not run.
    ///
    /// # Errors
    /// [`RegistryError`] for an invalid source, an existing version or I/O failures.
    pub fn install(&self, source: &Path) -> Result<Arc<Package>, RegistryError> {
        if !source.is_dir() {
            return Err(RegistryError::Invalid(format!(
                "{} is not a directory",
                source.display()
            )));
        }
        let package =
            Package::load(source).map_err(|err| RegistryError::Invalid(err.to_string()))?;
        let meta = &package.manifest.plugin;
        let target = self.dir.join(&meta.id).join(&meta.version);
        if target.exists() {
            return Err(RegistryError::Exists(format!(
                "{} {}",
                meta.id, meta.version
            )));
        }
        let staging = self
            .dir
            .join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
        let mut copied = 0;
        let result = copy_tree(source, &staging, 0, &mut copied).and_then(|()| {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&staging, &target)?;
            Ok(())
        });
        if let Err(err) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
        self.load_dir(&target)?;
        self.get(&meta.id)
            .ok_or_else(|| RegistryError::Invalid("installed plugin vanished".into()))
    }
}

fn copy_tree(from: &Path, to: &Path, depth: usize, copied: &mut u64) -> Result<(), RegistryError> {
    if depth > MAX_DEPTH {
        return Err(RegistryError::Invalid(
            "plugin tree nested too deeply".into(),
        ));
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_symlink() {
            return Err(RegistryError::Invalid(format!(
                "{} is a symbolic link; plugins must not contain links",
                entry.path().display()
            )));
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &dest, depth + 1, copied)?;
        } else {
            *copied += entry.metadata()?.len();
            if *copied > MAX_INSTALL_BYTES {
                return Err(RegistryError::Invalid("plugin larger than 256 MiB".into()));
            }
            // `copy` keeps the permission bits (executables stay executable).
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}
