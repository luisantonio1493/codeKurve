//! Project root resolution and initialization. See CODEKURVE_MASTER_PLAN.md
//! §14.1 (root canonicalization) and §15 (discovery order). Phase 1 implements
//! explicit-path/cwd init; upward search for existing projects lands with the
//! `index`/`search` commands.

use std::path::{Path, PathBuf};

use crate::config::{Config, CONFIG_DIR, CONFIG_FILE};
use crate::error::{Error, Result};

/// Initialize a CodeKurve project at `root`.
///
/// Canonicalizes `root` (which must exist), creates `<root>/.codekurve/`, and
/// writes a default `config.toml`. Fails if the project is already
/// initialized. Returns the path to the written config file.
pub fn init(root: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|_| Error::PathNotFound(root.to_path_buf()))?;

    let dir = root.join(CONFIG_DIR);
    let file = dir.join(CONFIG_FILE);
    if file.exists() {
        return Err(Error::AlreadyInitialized(file));
    }

    let mut config = Config::default();
    if let Some(name) = root.file_name().and_then(|n| n.to_str()) {
        config.project.name = name.to_string();
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::write(&file, config.to_toml()?)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent_guarded() {
        let tmp = std::env::temp_dir().join(format!("codekurve-init-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let file = init(&tmp).expect("first init");
        assert!(file.exists());
        assert!(Config::from_toml(&std::fs::read_to_string(&file).unwrap()).is_ok());

        // Second init must refuse to overwrite.
        assert!(matches!(init(&tmp), Err(Error::AlreadyInitialized(_))));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn init_reports_missing_root() {
        let missing = Path::new("/nonexistent/codekurve/root/xyz");
        assert!(matches!(init(missing), Err(Error::PathNotFound(_))));
    }
}
