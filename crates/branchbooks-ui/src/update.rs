//! Updates on Windows (Velopack; release.yml publishes them). The newest release downloads in the
//! background and the person chooses when to restart into it; if they never do, it applies the next
//! time the app starts (`VelopackApp` in main.rs). Elsewhere, and in a build that was not installed
//! by Setup, there is nothing to update.

/// Where a released app looks. `BRANCHBOOKS_UPDATE_FEED` at build time points a test build elsewhere.
#[cfg(windows)]
const FEED: &str = match option_env!("BRANCHBOOKS_UPDATE_FEED") {
    Some(feed) => feed,
    None => "https://github.com/BranchBooks/branchbooks/releases/latest/download",
};

#[cfg(windows)]
fn manager() -> Result<velopack::UpdateManager, velopack::Error> {
    velopack::UpdateManager::new(velopack::sources::HttpSource::new(FEED), None, None)
}

/// Downloads the newest release, if there is one. The version to restart into, or `None`.
///
/// # Errors
/// When the feed or the download fails, as it does on every start while offline.
#[cfg(windows)]
pub fn download() -> Result<Option<String>, String> {
    use velopack::{Error, UpdateCheck};
    let run = || {
        let manager = manager()?;
        let UpdateCheck::UpdateAvailable(update) = manager.check_for_updates()? else {
            return Ok(None);
        };
        manager.download_updates(&update, None)?;
        Ok(Some(update.TargetFullRelease.Version.clone()))
    };
    match run() {
        Err(Error::NotInstalled(_)) => Ok(None),
        other => other.map_err(|e: Error| e.to_string()),
    }
}

/// Closes the app, applies the downloaded release and starts the new version.
///
/// # Errors
/// When the update cannot start; the app then keeps running.
#[cfg(windows)]
pub fn restart() -> Result<(), String> {
    let run = || {
        let manager = manager()?;
        match manager.get_update_pending_restart() {
            Some(asset) => manager.apply_updates_and_restart(&asset),
            None => Ok(()),
        }
    };
    run().map_err(|e: velopack::Error| e.to_string())
}

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
