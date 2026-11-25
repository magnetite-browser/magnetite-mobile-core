//! Files the core keeps in the app's folder.

use std::io::Write;
use std::path::Path;

/// Write `data` to `path` whole, readable by its owner only: written beside
/// it and moved over it, so a reader never sees half of it.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(&part)?.write_all(data)?;
    std::fs::rename(part, path)
}
