//! Over-the-internet updates from GitHub Releases.
//!
//! - **Check:** one HTTPS request to the GitHub Releases API (WinHTTP, built
//!   into Windows, so no TLS library is bundled). Runs on a short-lived thread.
//! - **Download:** the release's `sigma5.exe` into `%LOCALAPPDATA%\SIGMA5\updates`,
//!   through the API's asset endpoint. The github.com download link is only a
//!   fallback: it has been seen returning HTTP 504 while the API worked.
//! - **Verify:** SHA-256 (Windows BCrypt) must match the digest GitHub publishes
//!   for the asset; otherwise the file is deleted and nothing runs.
//! - **Apply:** the new exe's built-in installer (`--install`) stops this copy,
//!   swaps the files, keeps settings and relaunches.

use std::path::PathBuf;

use serde::Deserialize;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::{PCWSTR, w};

use crate::install::{VERSION, parse_version};
use crate::util::wide;

const REPO: &str = "Euroserus/SIGMA5";
const ASSET: &str = "sigma5.exe";

/// A newer release that can be installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    /// e.g. "Bernoulli" (from the release title "SIGMA5 0.3.0 “Bernoulli”").
    pub codename: String,
    pub download_url: String,
    /// The same file through the API (`api.github.com/.../releases/assets/<id>`).
    pub api_url: String,
    /// Hex SHA-256 published by GitHub for the asset.
    pub sha256: String,
    pub size: u64,
    /// Release page (notes).
    pub page_url: String,
}

impl UpdateInfo {
    /// "0.3.0 “Bernoulli”"
    pub fn label(&self) -> String {
        if self.codename.is_empty() { self.version.clone() } else { format!("{} “{}”", self.version, self.codename) }
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    /// API endpoint for the asset; serves the file with `Accept: application/octet-stream`.
    #[serde(default)]
    url: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    size: u64,
}

/// The codename in a release title: text inside “…” or "…".
pub fn codename_from_title(title: &str) -> String {
    for (open, close) in [('“', '”'), ('"', '"')] {
        if let Some(a) = title.find(open) {
            let rest = &title[a + open.len_utf8()..];
            if let Some(b) = rest.find(close) {
                return rest[..b].trim().to_string();
            }
        }
    }
    String::new()
}

/// Parses the API response; returns an update only if it's newer than `current`.
pub fn parse_latest(json: &str, current: &str) -> Result<Option<UpdateInfo>, String> {
    let r: ApiRelease = serde_json::from_str(json).map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let version = r.tag_name.trim_start_matches('v').to_string();
    if parse_version(&version).is_none() || parse_version(&version) <= parse_version(current) {
        return Ok(None);
    }
    let asset = r.assets.iter().find(|a| a.name.eq_ignore_ascii_case(ASSET)).ok_or("The new release has no sigma5.exe")?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or("The new release has no published checksum")?
        .to_ascii_lowercase();
    Ok(Some(UpdateInfo {
        codename: codename_from_title(r.name.as_deref().unwrap_or("")),
        version,
        download_url: asset.browser_download_url.clone(),
        api_url: asset.url.clone(),
        sha256,
        size: asset.size,
        page_url: r.html_url,
    }))
}

// ---------------------------------------------------------------------------
// HTTPS (WinHTTP)

struct Handle(*mut core::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// HTTPS GET with redirects followed. `progress(received, total)` is called as
/// data arrives (total = 0 when unknown).
pub fn https_get(url: &str, accept: &str, mut progress: impl FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("Only HTTPS is allowed")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let err = |what: &str| format!("Network error ({what}). Are you online?");
    unsafe {
        let agent = wide(&format!("SIGMA5-updater/{VERSION}"));
        let session = Handle(WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(err("open"));
        }
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let h = wide(host);
        let connect = Handle(WinHttpConnect(session.0, PCWSTR(h.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if connect.0.is_null() {
            return Err(err("connect"));
        }
        let p = wide(&path);
        let req = Handle(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            PCWSTR(p.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(err("request"));
        }
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        let _ = WinHttpAddRequestHeaders(req.0, &headers, WINHTTP_ADDREQ_FLAG_ADD);
        WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|_| err("send"))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|_| err("receive"))?;

        let query_num = |flag: u32| -> u64 {
            let mut v = 0u32;
            let mut len = 4u32;
            let mut idx = 0u32;
            let ok = WinHttpQueryHeaders(req.0, flag | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut v as *mut u32 as *mut _), &mut len, &mut idx).is_ok();
            if ok { v as u64 } else { 0 }
        };
        let status = query_num(WINHTTP_QUERY_STATUS_CODE);
        if status != 200 {
            return Err(match status {
                403 | 429 => "GitHub is rate-limiting update checks; try again later".into(),
                404 => "No releases found".into(),
                s => format!("GitHub returned HTTP {s}"),
            });
        }
        let total = query_num(WINHTTP_QUERY_CONTENT_LENGTH);
        let mut body = Vec::with_capacity(total as usize);
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail).map_err(|_| err("read"))?;
            if avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(req.0, body[start..].as_mut_ptr() as *mut _, avail, &mut read).map_err(|_| err("read"))?;
            body.truncate(start + read as usize);
            progress(body.len() as u64, total);
            if body.len() > 200 << 20 {
                return Err("Download is unexpectedly large".into());
            }
        }
        Ok(body)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if status.is_err() {
        return String::new();
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Check / download

/// Asks GitHub for the latest release. Ok(None) = up to date.
pub fn check() -> Result<Option<UpdateInfo>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = https_get(&url, "application/vnd.github+json", |_, _| {})?;
    parse_latest(&String::from_utf8_lossy(&body), VERSION)
}

pub fn updates_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("SIGMA5").join("updates")
}

/// Downloads and verifies the update. Returns the verified exe's path.
pub fn download(info: &UpdateInfo, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    // The API endpoint first, then the github.com link; report the first error.
    let mut data = None;
    let mut first_err = None;
    for url in [&info.api_url, &info.download_url] {
        if !url.starts_with("https://") {
            continue;
        }
        match https_get(url, "application/octet-stream", &mut progress) {
            Ok(d) => {
                data = Some(d);
                break;
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    let data = data.ok_or_else(|| first_err.unwrap_or_else(|| "The new release has no download link".into()))?;
    if sha256_hex(&data) != info.sha256 {
        return Err("The download didn't match its published checksum, so it wasn't installed".into());
    }
    let dir = updates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("sigma5-{}.exe", info.version));
    std::fs::write(&path, &data).map_err(|e| format!("Couldn't save the update: {e}"))?;
    Ok(path)
}

/// Runs the downloaded exe's installer. An installed copy updates silently and
/// relaunches; a portable copy shows the update window instead.
pub fn apply(exe: &PathBuf) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    if crate::install::running_installed() {
        cmd.arg("--install");
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Couldn't start the update: {e}"))
}

/// Removes downloaded update files (called at startup).
pub fn cleanup() {
    let _ = std::fs::remove_dir_all(updates_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "tag_name": "v0.3.0", "name": "SIGMA5 0.3.0 “Bernoulli”", "html_url": "https://github.com/x/y/releases/tag/v0.3.0",
        "draft": false, "prerelease": false,
        "assets": [
            {"name": "notes.txt", "browser_download_url": "https://e/notes", "size": 1},
            {"name": "sigma5.exe", "browser_download_url": "https://github.com/x/y/releases/download/v0.3.0/sigma5.exe",
             "url": "https://api.github.com/repos/x/y/releases/assets/42",
             "digest": "sha256:16588b36f770f28b39ec880a68234b789503b48f3e930e30318b0b3085fd76b5", "size": 784896}
        ]
    }"#;

    #[test]
    fn newer_release_is_offered() {
        let u = parse_latest(SAMPLE, "0.2.0").unwrap().unwrap();
        assert_eq!(u.version, "0.3.0");
        assert_eq!(u.codename, "Bernoulli");
        assert_eq!(u.label(), "0.3.0 “Bernoulli”");
        assert!(u.download_url.ends_with("/sigma5.exe"));
        assert_eq!(u.api_url, "https://api.github.com/repos/x/y/releases/assets/42");
        assert_eq!(u.sha256.len(), 64);
    }

    #[test]
    fn same_or_older_is_not() {
        assert_eq!(parse_latest(SAMPLE, "0.3.0").unwrap(), None);
        assert_eq!(parse_latest(SAMPLE, "0.4.1").unwrap(), None);
    }

    #[test]
    fn missing_checksum_is_refused() {
        let no_digest = SAMPLE.replace(r#""digest": "sha256:16588b36f770f28b39ec880a68234b789503b48f3e930e30318b0b3085fd76b5", "#, "");
        assert!(parse_latest(&no_digest, "0.2.0").is_err());
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        assert_eq!(parse_latest(&SAMPLE.replace(r#""prerelease": false"#, r#""prerelease": true"#), "0.2.0").unwrap(), None);
    }

    #[test]
    fn codenames() {
        assert_eq!(codename_from_title("SIGMA5 0.2.0 “Euler”"), "Euler");
        assert_eq!(codename_from_title("SIGMA5 0.2.0 \"Euler\""), "Euler");
        assert_eq!(codename_from_title("SIGMA5 v0.1.0"), "");
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    /// Live: talks to GitHub and downloads the current release, checking the
    /// checksum. Doesn't install anything.
    /// `cargo test --release live_update -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_update_check_and_verified_download() {
        let body = https_get(&format!("https://api.github.com/repos/{REPO}/releases/latest"), "application/vnd.github+json", |_, _| {}).unwrap();
        let json = String::from_utf8_lossy(&body);
        // Pretend we're older than anything published so an update is offered.
        let info = parse_latest(&json, "0.0.1").unwrap().expect("a release with sigma5.exe and a digest");
        println!("latest: {} ({} bytes) sha256 {}", info.label(), info.size, info.sha256);
        let mut last = 0;
        let path = download(&info, |got, total| last = if total > 0 { got * 100 / total } else { 0 }).unwrap();
        println!("downloaded + verified: {} (progress reached {last}%)", path.display());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), info.size);
        let _ = std::fs::remove_file(path);
        println!("real check() as this build: {:?}", check().map(|o| o.map(|u| u.label())));
    }
}
