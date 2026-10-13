//! `co-review setup <pi|omp>`: installs a bundled agent package with the harness's own installer.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, bail};

use crate::debug_log::log;
use crate::installation;

/// Agent packages shipped with Co-Review, each in `integrations/<harness>`.
const PACKAGES: [&str; 2] = ["pi", "omp"];

/// Where this Co-Review's package for `harness` is.
fn bundled_package(harness: &str) -> Option<PathBuf> {
    Some(
        installation::distribution_root()?
            .join("integrations")
            .join(harness),
    )
}

/// Runs the installer; returns its exit code.
pub fn run(harness: Option<&str>) -> Result<i32> {
    let harness = harness.unwrap_or_default();
    if !PACKAGES.contains(&harness) {
        bail!("usage: co-review setup <{}>", PACKAGES.join("|"));
    }
    let Some(directory) =
        bundled_package(harness).filter(|directory| directory.join("package.json").is_file())
    else {
        let location = bundled_package(harness)
            .map(|directory| directory.display().to_string())
            .unwrap_or_else(|| "not found".to_string());
        bail!("this Co-Review has no {harness} package ({location}); update Co-Review");
    };
    log(&format!("{harness} install {}", directory.display()));
    match Command::new(harness)
        .arg("install")
        .arg(&directory)
        .status()
    {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("{harness} is not installed or not on PATH")
        }
        Err(error) => bail!("{error}"),
    }
}
