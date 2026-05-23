use std::fs;
use std::path::Path;

pub mod create;
pub mod get;
pub mod list;
pub mod rm;
pub mod root;

// ghq cmd_create.go / cmd_rm.go: isNotExistOrEmpty.
pub(crate) fn is_not_exist_or_empty(path: &Path) -> anyhow::Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Err(anyhow::anyhow!(
            "{} exists but is not a directory",
            path.display()
        )),
        Err(e) => Err(anyhow::Error::from(e).context(format!("inspect {}", path.display()))),
    }
}
