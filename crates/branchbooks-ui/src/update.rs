//! Updates on Windows. A release publishes its installer (Inno Setup, `tools/windows/branchbooks.iss`)
//! and `latest.txt`, one line: the version, the installer's SHA-256 and its file name. The app reads
//! that line from the feed, downloads a newer installer in the background and checks its SHA-256;
//! the person chooses when to restart into it, which runs it silently and starts the new version.
//! A download not yet installed is found again at the next start. Elsewhere, in a dev build, and in
//! a build not installed by Setup, there is nothing to update.

/// Where a released app looks: an address on the project's own domain, forwarded to the latest GitLab
/// release, so a move between hosts needs no new app. `BRANCHBOOKS_UPDATE_FEED` at build time points a
/// test build elsewhere; a build for another channel (`BRANCHBOOKS_CHANNEL`) has no feed unless given one.
#[cfg_attr(not(windows), allow(dead_code))]
const FEED: Option<&str> = match option_env!("BRANCHBOOKS_UPDATE_FEED") {
    Some(feed) => Some(feed),
    None => match option_env!("BRANCHBOOKS_CHANNEL") {
        Some(_) => None,
        None => Some("https://branchbooks.dev/updates"),
    },
};

/// A release as `latest.txt` gives it.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Latest {
    version: String,
    sha256: String,
    file: String,
}

/// `latest.txt`'s one line, or `None` for anything else. The file name must be a plain name, so the
/// feed can't point the download outside its folder.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_latest(text: &str) -> Option<Latest> {
    let mut parts = text.split_whitespace();
    let (version, sha256, file) = (parts.next()?, parts.next()?, parts.next()?);
    let plain =
        !file.contains(['/', '\\', ':']) && !file.starts_with('.') && file.ends_with(".exe");
    let hex = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
    (parts.next().is_none() && plain && hex && version_of(version).is_some()).then(|| Latest {
        version: version.to_owned(),
        sha256: sha256.to_ascii_lowercase(),
        file: file.to_owned(),
    })
}

/// "1.2.3" as numbers, to compare.
fn version_of(v: &str) -> Option<(u64, u64, u64)> {
    let mut n = v.split('.').map(|p| p.parse::<u64>().ok());
    let version = (n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some(version)
}

/// Whether `latest` is newer than the running app.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_newer(latest: &str, running: &str) -> bool {
    matches!((version_of(latest), version_of(running)), (Some(l), Some(r)) if l > r)
}

#[cfg(windows)]
mod windows {
    use super::{FEED, Latest, is_newer, parse_latest};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// The installer downloaded and checked, waiting for a restart.
    static READY: Mutex<Option<PathBuf>> = Mutex::new(None);

    /// Whether Setup installed this copy: Inno Setup puts its uninstaller beside the app.
    fn installed() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("unins000.exe")))
            .is_some_and(|u| u.exists())
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn get(url: &str) -> Result<Vec<u8>, String> {
        let mut response = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
        response
            .body_mut()
            .with_config()
            .limit(500 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| format!("{url}: {e}"))
    }

    /// Downloads the newest release, if there is one. The version to restart into, or `None`.
    ///
    /// # Errors
    /// When the feed or the download fails, as it does on every start while offline, or when the
    /// download is not the file the feed names.
    pub fn download() -> Result<Option<String>, String> {
        let Some(feed) = FEED.filter(|_| installed()) else {
            return Ok(None);
        };
        let text = get(&format!("{feed}/latest.txt"))?;
        let Some(Latest {
            version,
            sha256,
            file,
        }) = parse_latest(&String::from_utf8_lossy(&text))
        else {
            return Err(format!(
                "{feed}/latest.txt is not a version, a SHA-256 and a file name"
            ));
        };
        if !is_newer(&version, env!("CARGO_PKG_VERSION")) {
            return Ok(None);
        }
        let dir = std::env::temp_dir().join("BranchBooks-update");
        let path = dir.join(&file);
        // Downloaded at an earlier start and not installed yet: no need to fetch it again.
        let have = std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == sha256);
        if !have {
            let bytes = get(&format!("{feed}/{file}"))?;
            if sha256_hex(&bytes) != sha256 {
                return Err(format!(
                    "{file} is not the file the feed names (SHA-256 differs)"
                ));
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        }
        *READY.lock().map_err(|e| e.to_string())? = Some(path);
        Ok(Some(version))
    }

    /// Runs the downloaded installer silently and closes the app; the installer starts the new
    /// version when it is done.
    ///
    /// # Errors
    /// When the installer cannot start; the app then keeps running.
    pub fn restart() -> Result<(), String> {
        let Some(path) = READY.lock().map_err(|e| e.to_string())?.clone() else {
            return Ok(());
        };
        std::process::Command::new(&path)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
            .spawn()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        std::process::exit(0)
    }
}

#[cfg(windows)]
pub use windows::{download, restart};

/// Nothing to update on this platform.
///
/// # Errors
/// Never.
#[cfg(not(windows))]
pub fn download() -> Result<Option<String>, String> {
    Ok(None)
}

/// Nothing to update on this platform.
///
/// # Errors
/// Never.
#[cfg(not(windows))]
pub fn restart() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feed_line_is_a_version_a_sha256_and_a_plain_installer_name() {
        let sha = "AB".repeat(32);
        let line = format!("0.2.0 {sha} BranchBooks-0.2.0-setup.exe\n");
        assert_eq!(
            parse_latest(&line),
            Some(Latest {
                version: "0.2.0".into(),
                sha256: "ab".repeat(32),
                file: "BranchBooks-0.2.0-setup.exe".into(),
            })
        );
        for bad in [
            format!("0.2 {sha} BranchBooks-setup.exe"),
            format!("0.2.0 {} BranchBooks-setup.exe", "ab".repeat(31)),
            format!("0.2.0 {sha} ..\\evil.exe"),
            format!("0.2.0 {sha} C:evil.exe"),
            format!("0.2.0 {sha} BranchBooks-setup.msi"),
            format!("0.2.0 {sha} BranchBooks-setup.exe extra"),
            String::new(),
        ] {
            assert_eq!(parse_latest(&bad), None, "{bad}");
        }
    }

    #[test]
    fn only_a_newer_version_updates() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("junk", "0.1.0"));
    }
}
