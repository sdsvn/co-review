//! `co-review setup <pi|omp>`: installs a bundled agent package with the harness's own installer.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, bail};

use crate::debug_log::log;
use crate::installation;

/// Agent packages shipped with Co-Review: harness → directory under `integrations/`.
pub const PACKAGES: [(&str, &str); 2] = [("pi", "pi"), ("omp", "omp")];

/// Where this Co-Review's package for `harness` is.
pub fn bundled_package(harness: &str) -> Option<PathBuf> {
    let (_, directory) = PACKAGES.iter().find(|(name, _)| *name == harness)?;
    Some(
        installation::distribution_root()?
            .join("integrations")
            .join(directory),
    )
}

/// Runs the installer; returns its exit code.
pub fn run(harness: Option<&str>) -> Result<i32> {
    let harness = harness.unwrap_or_default();
    if !PACKAGES.iter().any(|(name, _)| *name == harness) {
        let names: Vec<&str> = PACKAGES.iter().map(|(name, _)| *name).collect();
        bail!("usage: co-review setup <{}>", names.join("|"));
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
