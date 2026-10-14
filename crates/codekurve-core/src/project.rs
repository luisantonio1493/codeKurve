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

    create_private_dir(&dir)?;
    write_private_file(&file, config.to_toml()?.as_bytes())?;
    Ok(file)
}

/// Create `dir` (and missing parents) with owner-only access on Unix. Existing
/// directories are left untouched.
#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// Write a new config file with owner-only access on Unix.
#[cfg(unix)]
fn write_private_file(file: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(file)?;
    handle.write_all(contents)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(file: &Path, contents: &[u8]) -> Result<()> {
    std::fs::write(file, contents)?;
    Ok(())
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

#[cfg(all(test, unix))]
mod unix_permission_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn init_creates_owner_only_dir_and_config() {
        let tmp = std::env::temp_dir().join(format!("codekurve-init-perms-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();

        let file = init(&tmp).expect("init");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&file), 0o600);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
