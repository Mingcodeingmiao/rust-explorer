//! GitHub stable-release checks and SHA-256 verified ZIP downloads.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const REPOSITORY: &str = "Mingcodeingmiao/rust-explorer";
const LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Control {
    cancelled: Arc<AtomicBool>,
    received: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}
impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    fn checkpoint(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            anyhow::bail!(Cancelled);
        }
        Ok(())
    }
    pub fn progress(&self) -> (u64, Option<u64>) {
        let total = self.total.load(Ordering::Relaxed);
        (
            self.received.load(Ordering::Relaxed),
            (total != 0).then_some(total),
        )
    }
}
#[derive(Debug)]
struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Update cancelled")
    }
}
impl std::error::Error for Cancelled {}
#[derive(Debug)]
struct RateLimited;
impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitHub request rate limited")
    }
}
impl std::error::Error for RateLimited {}

/// Keep diagnostic URLs and low-level transport errors out of the settings text.
pub fn error_message(error: &anyhow::Error) -> String {
    crate::i18n::text(if error.is::<Cancelled>() {
        "已取消更新操作"
    } else if error.is::<RateLimited>() {
        "GitHub 请求暂时受限，请稍后重试"
    } else if error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_timeout())
    }) {
        "更新请求超时，请检查网络后重试"
    } else if error
        .chain()
        .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some())
    {
        "无法连接更新服务器，请检查网络后重试"
    } else {
        "更新失败，发行包或校验文件可能无效，请稍后重试"
    })
    .into()
}

fn checked_response(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    if matches!(response.status().as_u16(), 403 | 429) {
        anyhow::bail!(RateLimited);
    }
    Ok(response.error_for_status()?)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Available {
    pub version: String,
    pub filename: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn version(text: &str) -> Result<[u64; 3]> {
    let parts: Vec<_> = text.trim_start_matches('v').split('.').collect();
    ensure!(
        parts.len() == 3
            && parts
                .iter()
                .all(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())),
        "Invalid stable version"
    );
    Ok([parts[0].parse()?, parts[1].parse()?, parts[2].parse()?])
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("RustExplorer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("Too many redirects");
            }
            if attempt.url().scheme() == "https"
                && matches!(
                    attempt.url().host_str(),
                    Some(
                        "github.com"
                            | "api.github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.error("Unexpected update download host")
            }
        }))
        .build()?)
}

fn bounded_text(response: reqwest::blocking::Response) -> Result<String> {
    let mut bytes = Vec::new();
    checked_response(response)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "Update response is too large"
    );
    Ok(String::from_utf8(bytes)?)
}

fn url(tag: &str, filename: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{filename}")
}

fn choose(release: Release, current: &str) -> Result<Option<Available>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let parsed = version(&release.tag_name)?;
    if parsed <= version(current)? {
        return Ok(None);
    }
    let v = format!("{}.{}.{}", parsed[0], parsed[1], parsed[2]);
    let tag = format!("v{v}");
    ensure!(release.tag_name == tag, "Unexpected release tag");
    ensure!(
        release
            .assets
            .iter()
            .any(|a| a.name == "SHA256SUMS.txt"
                && a.browser_download_url == url(&tag, "SHA256SUMS.txt")),
        "Release has no checksum file"
    );
    let name = format!("RustExplorer-{v}-windows-x64.zip");
    if release.assets.iter().any(|a| {
        a.name == name
            && a.browser_download_url == url(&tag, &name)
            && a.size > 0
            && a.size <= LIMIT
    }) {
        return Ok(Some(Available {
            version: v,
            filename: name,
        }));
    }
    anyhow::bail!("Release has no supported Windows x64 package")
}

pub fn check() -> Result<Option<Available>> {
    check_with(&Control::default())
}

pub fn check_with(control: &Control) -> Result<Option<Available>> {
    control.checkpoint()?;
    let response = client()?
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(25))
        .send()?;
    let selected = choose(
        serde_json::from_str(&bounded_text(response)?)?,
        env!("CARGO_PKG_VERSION"),
    )?;
    control.checkpoint()?;
    Ok(selected)
}

fn checksum(text: &str, name: &str) -> Result<String> {
    let hashes: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (hash, filename) = line.split_once(char::is_whitespace)?;
            (filename.trim().trim_start_matches('*') == name).then_some(hash)
        })
        .collect();
    ensure!(
        hashes.len() == 1
            && hashes[0].len() == 64
            && hashes[0].bytes().all(|c| c.is_ascii_hexdigit()),
        "Missing or ambiguous package checksum"
    );
    Ok(hashes[0].to_ascii_lowercase())
}

pub fn download(update: &Available) -> Result<PathBuf> {
    download_with(update, &Control::default())
}

fn copy_checked(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected: &str,
    control: &Control,
) -> Result<()> {
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        control.checkpoint()?;
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        control.checkpoint()?;
        count += n as u64;
        ensure!(count <= LIMIT, "Package is too large");
        hash.update(&buffer[..n]);
        writer.write_all(&buffer[..n])?;
        control.received.store(count, Ordering::Relaxed);
    }
    control.checkpoint()?;
    ensure!(
        count > 0 && format!("{:x}", hash.finalize()) == expected,
        "Update checksum verification failed"
    );
    Ok(())
}

pub fn download_with(update: &Available, control: &Control) -> Result<PathBuf> {
    control.checkpoint()?;
    version(&update.version)?;
    let expected_name = format!("RustExplorer-{}-windows-x64.zip", update.version);
    ensure!(update.filename == expected_name, "Invalid update filename");
    let client = client()?;
    let tag = format!("v{}", update.version);
    let expected = checksum(
        &bounded_text(
            client
                .get(url(&tag, "SHA256SUMS.txt"))
                .timeout(Duration::from_secs(25))
                .send()?,
        )?,
        &update.filename,
    )?;
    control.checkpoint()?;
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA is unavailable")?
        .join("RustExplorer/updates")
        .join(&update.version);
    std::fs::create_dir_all(&directory)?;
    // A unique directory avoids concurrent windows overwriting each other's
    // verified package, or opening a file while it is still being written.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = directory.join(format!("{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory)?;
    let partial = directory.join("download.part");
    let target = directory.join(&update.filename);
    let result = (|| -> Result<()> {
        let mut response = checked_response(client.get(url(&tag, &update.filename)).send()?)?;
        ensure!(
            response.content_length().is_none_or(|n| n <= LIMIT),
            "Package is too large"
        );
        control.checkpoint()?;
        control
            .total
            .store(response.content_length().unwrap_or(0), Ordering::Relaxed);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        copy_checked(&mut response, &mut file, &expected, control)?;
        file.sync_all()?;
        drop(file);
        control.checkpoint()?;
        std::fs::rename(&partial, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
        let _ = std::fs::remove_dir(&directory);
    }
    result?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_stream_rejects_corruption_and_cancellation_after_a_read() {
        let data = b"verified package";
        let expected = format!("{:x}", Sha256::digest(data));
        let control = Control::default();
        let mut output = Vec::new();
        copy_checked(&mut data.as_slice(), &mut output, &expected, &control).unwrap();
        assert_eq!(output, data);
        assert_eq!(control.progress().0, data.len() as u64);
        assert!(
            copy_checked(
                &mut b"corrupt".as_slice(),
                &mut Vec::new(),
                &expected,
                &Control::default()
            )
            .is_err()
        );
        struct CancelOnRead(Control);
        impl Read for CancelOnRead {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                bytes[0] = 1;
                self.0.cancel();
                Ok(1)
            }
        }
        let control = Control::default();
        let mut reader = CancelOnRead(control.clone());
        let mut output = Vec::new();
        assert!(
            copy_checked(&mut reader, &mut output, &expected, &control)
                .unwrap_err()
                .is::<Cancelled>()
        );
        assert!(output.is_empty(), "cancelled chunks must not be written");
    }
    #[test]
    fn cancellation_is_shared_and_checked_before_any_network_access() {
        let control = Control::default();
        let clone = control.clone();
        control.cancel();
        assert!(check_with(&clone).unwrap_err().is::<Cancelled>());
        assert!(
            download_with(
                &Available {
                    version: "0.8.7".into(),
                    filename: "RustExplorer-0.8.7-windows-x64.zip".into()
                },
                &clone
            )
            .unwrap_err()
            .is::<Cancelled>()
        );
        assert!(!error_message(&anyhow::Error::new(RateLimited)).contains("https://"));
    }
    #[test]
    #[ignore = "Downloads a published package from GitHub; run explicitly with network access"]
    fn published_package_passes_download_verification() {
        let control = Control::default();
        let path = download_with(
            &Available {
                version: "0.8.6".into(),
                filename: "RustExplorer-0.8.6-windows-x64.zip".into(),
            },
            &control,
        )
        .unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        assert!(length > 0);
        assert_eq!(control.progress().0, length);
        if let Some(total) = control.progress().1 {
            assert_eq!(total, length);
        }
        // Only remove the unique file/directory created by this test.
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn stable_versions_and_checksums_are_strict() {
        assert!(version("0.8.10").unwrap() > version("v0.8.9").unwrap());
        for value in ["0.9", "../0.8.7", "v0.8.7-beta", "0.8.-1"] {
            assert!(version(value).is_err());
        }
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{hash}  test.zip\n"), "test.zip").unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash}  test.zip\n{hash}  test.zip"), "test.zip").is_err());
        assert!(checksum("bogus  test.zip", "test.zip").is_err());
    }

    #[test]
    fn release_selection_accepts_only_official_zip_packages() {
        let version = "0.8.7";
        let tag = "v0.8.7";
        let name = format!("RustExplorer-{version}-windows-x64.zip");
        let make = |download_url: String| Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: vec![
                Asset {
                    name: name.clone(),
                    browser_download_url: download_url,
                    size: 123,
                },
                Asset {
                    name: "SHA256SUMS.txt".into(),
                    browser_download_url: url(tag, "SHA256SUMS.txt"),
                    size: 100,
                },
            ],
        };
        let selected = choose(make(url(tag, &name)), "0.8.6").unwrap().unwrap();
        assert_eq!(selected.filename, name);
        assert!(choose(make("https://example.com/setup.exe".into()), "0.8.6").is_err());
        assert!(choose(make(url(tag, &name)), "0.8.8").unwrap().is_none());
    }
}
