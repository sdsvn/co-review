// co-review: Co-Review's update feed: its GitHub releases and their assets (the ones install.sh installs), in place
// of Lapce's. Only release builds check for updates; development builds have no feed.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use lapce_core::{directory::Directory, meta};
use serde::Deserialize;

const LATEST_RELEASE: &str =
    "https://api.github.com/repos/sdsvn/co-review/releases/latest";

#[derive(Clone, Deserialize, Debug)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub target_commitish: String,
    pub assets: Vec<ReleaseAsset>,
    #[serde(skip)]
    pub version: String,
}

#[derive(Clone, Deserialize, Debug)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

pub fn get_latest_release() -> Result<ReleaseInfo> {
    if meta::RELEASE != meta::ReleaseType::Stable {
        return Err(anyhow!("no update feed for development builds"));
    }

    let resp = lapce_proxy::get_url(LATEST_RELEASE, Some("Co-Review"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("get release info failed {}", resp.text()?));
    }
    let mut release: ReleaseInfo = serde_json::from_str(&resp.text()?)?;
    release.version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_owned();
    Ok(release)
}

/// The release asset for this platform, as `.github/workflows/release.yml` names it.
fn asset_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("Co-Review-mac-arm64.zip"),
        ("linux", "x86_64") => Ok("Co-Review-linux-x64.tar.gz"),
        ("linux", "aarch64") => Ok("Co-Review-linux-arm64.tar.gz"),
        ("windows", "x86_64") => Ok("Co-Review-win-x64.exe"),
        (os, arch) => Err(anyhow!("no Co-Review release for {os} {arch}")),
    }
}

pub fn download_release(release: &ReleaseInfo) -> Result<PathBuf> {
    let dir =
        Directory::updates_directory().ok_or_else(|| anyhow!("no directory"))?;
    let name = asset_name()?;
    let file_path = dir.join(name);

    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| anyhow!("the release has no {name}"))?;
    let mut resp = lapce_proxy::get_url(&asset.browser_download_url, None)?;
    if !resp.status().is_success() {
        return Err(anyhow!("download file error {}", resp.text()?));
    }
    let mut out = std::fs::File::create(&file_path)?;
    resp.copy_to(&mut out)?;
    Ok(file_path)
}

/// An empty directory beside the download to unpack it into.
#[cfg(not(target_os = "windows"))]
fn staging_directory(src: &Path) -> Result<PathBuf> {
    let staging = src
        .parent()
        .ok_or_else(|| anyhow!("no parent"))?
        .join("unpacked");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    Ok(staging)
}

/// Puts `new` where `current` is, keeping `current` until `new` is in place.
#[cfg(not(target_os = "windows"))]
fn replace_directory(current: &Path, new: &Path) -> Result<()> {
    let old = current.with_extension("old");
    if old.exists() {
        std::fs::remove_dir_all(&old)?;
    }
    std::fs::rename(current, &old).map_err(|err| {
        anyhow!(err).context(format!("moving {} aside", current.display()))
    })?;
    if let Err(err) = std::fs::rename(new, current) {
        std::fs::rename(&old, current).ok();
        return Err(anyhow!(err).context(format!("installing {}", current.display())));
    }
    if let Err(err) = std::fs::remove_dir_all(&old) {
        tracing::error!("{:?}", err);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn extract(src: &Path, process_path: &Path) -> Result<PathBuf> {
    // <bundle>/Contents/MacOS/<executable>
    let bundle = process_path
        .ancestors()
        .nth(3)
        .filter(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
        .ok_or_else(|| anyhow!("Co-Review is not running from an app bundle"))?
        .to_path_buf();
    let staging = staging_directory(src)?;
    let status = std::process::Command::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(src)
        .arg(&staging)
        .status()?;
    if !status.success() {
        return Err(anyhow!("could not unpack {}", src.display()));
    }
    replace_directory(&bundle, &staging.join("Co-Review.app"))?;
    // Releases are not notarized: sign ad hoc so Apple silicon runs it, as install.sh does.
    std::process::Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(&bundle)
        .status()
        .ok();
    std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&bundle)
        .status()
        .ok();
    Ok(bundle)
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
pub fn extract(src: &Path, process_path: &Path) -> Result<PathBuf> {
    // <install dir>/co-review-app
    let install_dir = process_path.parent().ok_or_else(|| anyhow!("no parent"))?;
    let staging = staging_directory(src)?;
    let tar_gz = std::fs::File::open(src)?;
    let tar = flate2::read::GzDecoder::new(tar_gz);
    tar::Archive::new(tar).unpack(&staging)?;
    replace_directory(install_dir, &staging.join("Co-Review"))?;
    Ok(process_path.to_path_buf())
}

#[cfg(target_os = "windows")]
pub fn extract(src: &Path, _process_path: &Path) -> Result<PathBuf> {
    // The installer itself: restart runs it.
    Ok(src.to_path_buf())
}

#[cfg(target_os = "macos")]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("open")
        .arg("-n")
        .arg(path)
        .arg("--args")
        .arg("-n")
        .exec();
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new(path).arg("-n").exec();
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x00000008;
    let process_id = std::process::id();
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("can't get path to str"))?;

    let exe = std::env::current_exe()
        .map_err(|err| anyhow!("can't get path to exe").context(err))?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow!("can't convert exe path to str"))?;

    std::process::Command::new("cmd")
        .raw_arg(format!(
            r#"/C taskkill /PID {process_id} & "{path}" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART & start "" "{exe}""#,
        ))
        .creation_flags(DETACHED_PROCESS)
        .spawn()?;
    Ok(())
}

pub fn cleanup() {
    // Nothing to do: an update replaces the whole app.
}
