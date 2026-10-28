//! A plugin directory on disk: manifest plus the files it references.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::ConfigValidator;
use crate::event::EventValidator;
use crate::manifest::{MANIFEST_FILE, Manifest, ManifestError};

/// Largest `events_schema` / `config.schema` file read (bytes).
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
    /// `config.schema` is unreadable or not a JSON Schema.
    #[error("config.schema: {0}")]
    ConfigSchema(String),
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
    /// Configuration checker built from `config.schema`; `None` when the manifest names none.
    pub config_validator: Option<ConfigValidator>,
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
        let read_schema = |rel: &str, wrap: fn(String) -> PackageError| {
            let path = dir.join(rel);
            let meta = std::fs::metadata(&path).map_err(io(&path))?;
            if meta.len() > MAX_SCHEMA_BYTES {
                return Err(wrap(format!("{rel} is larger than 1 MiB")));
            }
            let raw = std::fs::read_to_string(&path).map_err(io(&path))?;
            serde_json::from_str::<Value>(&raw).map_err(|err| wrap(format!("{rel}: {err}")))
        };
        let schema = match &manifest.output.events_schema {
            Some(rel) => Some(read_schema(rel, PackageError::Schema)?),
            None => None,
        };
        let config_validator = match &manifest.config {
            Some(section) => {
                let doc = read_schema(&section.schema, PackageError::ConfigSchema)?;
                Some(
                    ConfigValidator::new(&doc)
                        .map_err(|err| PackageError::ConfigSchema(err.to_string()))?,
                )
            }
            None => None,
        };
        let validator = EventValidator::new(schema.as_ref())
            .map_err(|err| PackageError::Schema(err.to_string()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            manifest,
            validator,
            config_validator,
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
