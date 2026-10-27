//! A plugin directory on disk: manifest plus the files it references.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::EventValidator;
use crate::manifest::{MANIFEST_FILE, Manifest, ManifestError};

/// Largest `events_schema` file read (bytes).
const MAX_SCHEMA_BYTES: u64 = 1024 * 1024;

/// A plugin directory could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    /// A file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// File involved.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// The manifest is unusable.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// `events_schema` is unreadable or not a JSON Schema.
    #[error("events_schema: {0}")]
    Schema(String),
}

/// A loaded plugin directory.
#[derive(Debug)]
pub struct Package {
    /// Directory containing `plugin.toml`.
    pub dir: PathBuf,
    /// Validated manifest.
    pub manifest: Manifest,
    /// Event checker built from `output.events_schema`.
    pub validator: EventValidator,
}

impl Package {
    /// Load and validate `dir/plugin.toml` and the event schema it names.
    ///
    /// # Errors
    /// [`PackageError`] describing what is wrong.
    pub fn load(dir: &Path) -> Result<Self, PackageError> {
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| PackageError::Io { path, source }
        };
        let manifest_path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest_path).map_err(io(&manifest_path))?;
        let manifest = Manifest::from_toml(&text)?;
        let schema = match &manifest.output.events_schema {
            Some(rel) => {
                let path = dir.join(rel);
                let meta = std::fs::metadata(&path).map_err(io(&path))?;
                if meta.len() > MAX_SCHEMA_BYTES {
                    return Err(PackageError::Schema(format!("{rel} is larger than 1 MiB")));
                }
                let raw = std::fs::read_to_string(&path).map_err(io(&path))?;
                let doc: Value = serde_json::from_str(&raw)
                    .map_err(|err| PackageError::Schema(format!("{rel}: {err}")))?;
                Some(doc)
            }
            None => None,
        };
        let validator = EventValidator::new(schema.as_ref())
            .map_err(|err| PackageError::Schema(err.to_string()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            manifest,
            validator,
        })
    }

    /// The program and arguments to spawn, with a path-like first element resolved against the
    /// plugin directory. `None` for plugins without a process.
    #[must_use]
    pub fn command(&self) -> Option<(PathBuf, &[String])> {
        let (program, args) = self.manifest.runtime.command.split_first()?;
        let program = if program.contains('/') {
            self.dir.join(program)
        } else {
            PathBuf::from(program)
        };
        Some((program, args))
    }
}
