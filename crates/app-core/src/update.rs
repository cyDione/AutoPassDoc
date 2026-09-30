//! Update check (S-2): the latest GitHub release compared with the running
//! version, and the in-app update: download this platform's installer,
//! verify it and hand over to it.

use std::cmp::Ordering;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The latest published (non-draft, non-prerelease) release of the app.
pub const RELEASES_URL: &str = "https://api.github.com/repos/cyDione/AutoPassDoc/releases/latest";

const NO_RELEASE: &str = "还没有发布版";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub name: String,
    /// Direct download link.
    pub url: String,
    /// Bytes.
    pub size: u64,
    /// `sha256:<hex>` as GitHub reports it, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    /// Version of the latest release without the leading `v`; `None` when
    /// nothing has been released yet.
    pub latest: Option<String>,
    pub has_update: bool,
    /// Release title.
    pub name: Option<String>,
    /// Release notes (Markdown).
    pub notes: String,
    /// The release page, to open in the browser.
    pub url: Option<String>,
    /// RFC 3339.
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
    /// The asset the in-app update downloads and runs on this machine;
    /// `None` when the release has nothing for this platform.
    pub installer: Option<ReleaseAsset>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// Asks `api_url` (normally [`RELEASES_URL`]) for the latest release and
/// compares it with `current`.
pub async fn check_update(current: &str, api_url: &str) -> Result<UpdateInfo> {
    let failed = |e: String| Error::Invalid(format!("检查更新失败：{e}"));
    let client = reqwest::Client::builder()
        .user_agent("AutoPassDoc")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let response = client
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            failed(if e.is_timeout() {
                "连接 GitHub 超时，请检查网络".into()
            } else {
                format!("无法连接 GitHub（{e}）")
            })
        })?;
    let current = current.trim().trim_start_matches(['v', 'V']).to_string();
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateInfo {
            current,
            latest: None,
            has_update: false,
            name: None,
            notes: NO_RELEASE.into(),
            url: None,
            published_at: None,
            assets: Vec::new(),
            installer: None,
        });
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(failed("GitHub 限制了访问次数，请稍后再试".into()));
    }
    if !status.is_success() {
        return Err(failed(format!("GitHub 返回 {status}")));
    }
    let release: Release = response
        .json()
        .await
        .map_err(|e| failed(format!("无法读取发布信息（{e}）")))?;
    let latest = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let assets: Vec<ReleaseAsset> = release
        .assets
        .into_iter()
        .map(|a| ReleaseAsset {
            name: a.name,
            url: a.browser_download_url,
            size: a.size,
            digest: a.digest.filter(|d| !d.trim().is_empty()),
        })
        .collect();
    Ok(UpdateInfo {
        has_update: compare_versions(&latest, &current) == Ordering::Greater,
        current,
        latest: Some(latest),
        name: release.name.filter(|n| !n.trim().is_empty()),
        notes: release.body.unwrap_or_default(),
        url: release.html_url,
        published_at: release.published_at,
        installer: pick_installer(&assets, std::env::consts::OS, std::env::consts::ARCH).cloned(),
        assets,
    })
}

/// The installer for `os`/`arch` (as in [`std::env::consts`]): on Windows
/// the NSIS `-setup.exe`, else the `.msi`; on macOS the `.dmg` built for this
/// architecture (or one that names none).
pub fn pick_installer<'a>(
    assets: &'a [ReleaseAsset],
    os: &str,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let named = |a: &ReleaseAsset, suffix: &str| a.name.to_ascii_lowercase().ends_with(suffix);
    let arch_tags: &[&str] = match arch {
        "x86_64" => &["x64", "x86_64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        "x86" => &["x86", "i686"],
        _ => &[],
    };
    let all_tags = ["x64", "x86_64", "amd64", "aarch64", "arm64", "i686", "x86"];
    let lower = |a: &ReleaseAsset| a.name.to_ascii_lowercase();
    let for_arch = |a: &ReleaseAsset| arch_tags.iter().any(|t| lower(a).contains(t));
    let any_arch = |a: &ReleaseAsset| !all_tags.iter().any(|t| lower(a).contains(t));
    let best = |suffix: &str| {
        assets
            .iter()
            .filter(|a| named(a, suffix))
            .find(|a| for_arch(a))
            .or_else(|| {
                assets
                    .iter()
                    .filter(|a| named(a, suffix))
                    .find(|a| any_arch(a))
            })
    };
    match os {
        "windows" => best("-setup.exe").or_else(|| best(".msi")),
        "macos" => best(".dmg"),
        _ => None,
    }
}

/// Downloads `asset` into `dir`, reporting `(downloaded, total)` bytes as it
/// goes, and checks its size and SHA-256 digest. Setting `cancel` stops it.
/// Returns the file's path.
pub async fn download_installer(
    asset: &ReleaseAsset,
    dir: &Path,
    progress: impl Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let failed = |e: String| Error::Invalid(format!("下载更新失败：{e}"));
    let name = Path::new(&asset.name)
        .file_name()
        .ok_or_else(|| failed("安装包名称无效".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| failed(e.to_string()))?;
    let path = dir.join(name);
    let part = dir.join(format!("{}.part", name.to_string_lossy()));
    let client = reqwest::Client::builder()
        .user_agent("AutoPassDoc")
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let mut response = client
        .get(&asset.url)
        .send()
        .await
        .map_err(|e| failed(format!("无法连接 GitHub（{e}）")))?;
    if !response.status().is_success() {
        return Err(failed(format!("GitHub 返回 {}", response.status())));
    }
    let total = response.content_length().unwrap_or(asset.size);
    let mut file = std::fs::File::create(&part).map_err(|e| failed(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    progress(0, total);
    let outcome = async {
        loop {
            if cancel.load(AtomicOrdering::Relaxed) {
                return Err(Error::Invalid("已取消下载".into()));
            }
            let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| failed(format!("网络中断（{e}）")))?
            else {
                break;
            };
            file.write_all(&chunk).map_err(|e| failed(e.to_string()))?;
            hasher.update(&chunk);
            done += chunk.len() as u64;
            progress(done, total);
        }
        file.flush().map_err(|e| failed(e.to_string()))?;
        if asset.size > 0 && done != asset.size {
            return Err(failed(format!(
                "文件不完整（{done} / {} 字节）",
                asset.size
            )));
        }
        if let Some(expected) = asset
            .digest
            .as_deref()
            .and_then(|d| d.trim().strip_prefix("sha256:"))
        {
            let actual: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(failed("校验和不匹配，文件可能已损坏".into()));
            }
        }
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&part, &path).map_err(|e| failed(e.to_string()))?;
    Ok(path)
}

/// Starts the downloaded installer so it replaces this copy of the app; the
/// caller quits right after. Windows runs the NSIS installer in passive mode
/// (progress only) and restarts the app when done, or `msiexec /passive`.
/// macOS waits for this process (`pid`) to exit, copies the app out of the
/// disk image over the running bundle and reopens it; when that is not
/// possible it opens the disk image in Finder instead.
pub fn launch_installer(installer: &Path, pid: u32) -> Result<()> {
    let failed = |e: String| Error::Invalid(format!("无法启动安装程序：{e}"));
    let lower = installer.to_string_lossy().to_ascii_lowercase();
    if cfg!(windows) {
        let mut command = if lower.ends_with(".msi") {
            let mut c = std::process::Command::new("msiexec");
            c.arg("/i").arg(installer).arg("/passive");
            c
        } else {
            let mut c = std::process::Command::new(installer);
            c.args(["/P", "/R", "/UPDATE"]);
            c
        };
        command.spawn().map_err(|e| failed(e.to_string()))?;
        Ok(())
    } else if cfg!(target_os = "macos") && lower.ends_with(".dmg") {
        // …/AutoPassDoc.app/Contents/MacOS/<binary> → …/AutoPassDoc.app
        let bundle = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf))
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(MAC_INSTALL_SCRIPT)
            .arg("sh")
            .arg(pid.to_string())
            .arg(installer)
            .arg(bundle)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| failed(e.to_string()))?;
        Ok(())
    } else {
        Err(failed("当前系统不支持自动安装，请前往发布页下载".into()))
    }
}

/// `$1` pid to wait for, `$2` the .dmg, `$3` the running .app bundle (empty
/// when not running from one).
const MAC_INSTALL_SCRIPT: &str = r#"
PID="$1"; DMG="$2"; APP="$3"
while kill -0 "$PID" 2>/dev/null; do sleep 0.3; done
if [ -n "$APP" ]; then
  MNT=$(mktemp -d "${TMPDIR:-/tmp}/autopassdoc-update.XXXXXX")
  if hdiutil attach -nobrowse -noautoopen -quiet -mountpoint "$MNT" "$DMG"; then
    SRC=$(ls -d "$MNT"/*.app 2>/dev/null | head -n 1)
    OLD="$APP.old-$$"
    if [ -n "$SRC" ] && mv "$APP" "$OLD"; then
      if ditto "$SRC" "$APP"; then
        rm -rf "$OLD"
        hdiutil detach -quiet "$MNT"
        xattr -dr com.apple.quarantine "$APP" 2>/dev/null
        open "$APP"
        exit 0
      fi
      rm -rf "$APP"
      mv "$OLD" "$APP"
    fi
    hdiutil detach -quiet "$MNT"
  fi
fi
open "$DMG"
"#;

/// Compares versions like `1.2.3`, `v1.2`, `1.2.3-beta.2` (semantic
/// versioning precedence; build metadata after `+` is ignored, missing
/// numbers count as 0, a pre-release sorts before its release).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a_core, a_pre) = split_version(a);
    let (b_core, b_pre) = split_version(b);
    let len = a_core.len().max(b_core.len());
    for i in 0..len {
        let (x, y) = (
            a_core.get(i).copied().unwrap_or(0),
            b_core.get(i).copied().unwrap_or(0),
        );
        match x.cmp(&y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => compare_pre(x, y),
    }
}

fn split_version(v: &str) -> (Vec<u64>, Option<&str>) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split_once('+').map_or(v, |(v, _)| v);
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let numbers = core
        .split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect();
    (numbers, pre)
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn compares_versions() {
        use Ordering::*;
        assert_eq!(compare_versions("0.2.0", "0.1.0"), Greater);
        assert_eq!(compare_versions("v0.1.0", "0.1"), Equal);
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Greater);
        assert_eq!(compare_versions("1.0.0-beta.1", "1.0.0"), Less);
        assert_eq!(compare_versions("1.0.0-beta.2", "1.0.0-beta.10"), Less);
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0-alpha.3"), Greater);
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0-rc.1.1"), Less);
        assert_eq!(compare_versions("1.0.1+build5", "1.0.1"), Equal);
    }

    const API: &str = "/repos/cyDione/AutoPassDoc/releases/latest";

    async fn server(status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(API))
            .and(header("user-agent", "AutoPassDoc"))
            .and(header("accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn reports_a_newer_release() {
        let server = server(
            200,
            json!({
                "tag_name": "v0.2.0",
                "name": "AutoPassDoc 0.2.0",
                "body": "### 新增\n- 数据备份",
                "html_url": "https://github.com/cyDione/AutoPassDoc/releases/tag/v0.2.0",
                "published_at": "2026-10-10T08:00:00Z",
                "assets": [{
                    "name": "AutoPassDoc_0.2.0_x64-setup.exe",
                    "browser_download_url": "https://github.com/cyDione/AutoPassDoc/releases/download/v0.2.0/AutoPassDoc_0.2.0_x64-setup.exe",
                    "size": 12345
                }]
            }),
        )
        .await;
        let url = format!("{}{API}", server.uri());
        let info = check_update("0.1.0", &url).await.unwrap();
        assert!(info.has_update);
        assert_eq!(info.latest.as_deref(), Some("0.2.0"));
        assert_eq!(info.current, "0.1.0");
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].size, 12345);
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["hasUpdate"], true);
        assert_eq!(v["publishedAt"], "2026-10-10T08:00:00Z");
        assert!(v["assets"][0]["url"].as_str().unwrap().ends_with(".exe"));

        let info = check_update("v0.2.0", &url).await.unwrap();
        assert!(!info.has_update);
    }

    #[tokio::test]
    async fn no_release_yet_and_errors() {
        let server = server(404, json!({"message": "Not Found"})).await;
        let info = check_update("0.1.0", &format!("{}{API}", server.uri()))
            .await
            .unwrap();
        assert!(!info.has_update);
        assert_eq!(info.latest, None);
        assert_eq!(info.notes, NO_RELEASE);

        let server = server_status(403).await;
        let err = check_update("0.1.0", &format!("{}{API}", server.uri()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("限制"), "{err}");
        let server = server_status(500).await;
        let err = check_update("0.1.0", &format!("{}{API}", server.uri()))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("检查更新失败"), "{err}");
    }

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.into(),
            url: format!("https://example.com/{name}"),
            size: 1,
            digest: None,
        }
    }

    #[test]
    fn picks_the_installer_for_this_platform() {
        let assets = [
            asset("AutoPassDoc_0.2.0_aarch64.dmg"),
            asset("AutoPassDoc_0.2.0_x64_en-US.msi"),
            asset("AutoPassDoc_0.2.0_x64-setup.exe"),
        ];
        let name = |os, arch| pick_installer(&assets, os, arch).map(|a| a.name.as_str());
        assert_eq!(
            name("windows", "x86_64"),
            Some("AutoPassDoc_0.2.0_x64-setup.exe")
        );
        assert_eq!(
            name("macos", "aarch64"),
            Some("AutoPassDoc_0.2.0_aarch64.dmg")
        );
        assert_eq!(name("macos", "x86_64"), None);
        assert_eq!(name("windows", "aarch64"), None);
        assert_eq!(name("linux", "x86_64"), None);
        let msi_only = [asset("AutoPassDoc_0.2.0_x64_en-US.msi")];
        assert_eq!(
            pick_installer(&msi_only, "windows", "x86_64").map(|a| a.name.as_str()),
            Some("AutoPassDoc_0.2.0_x64_en-US.msi")
        );
        let universal = [asset("AutoPassDoc_0.3.0_universal.dmg")];
        assert!(pick_installer(&universal, "macos", "x86_64").is_some());
    }

    #[tokio::test]
    async fn downloads_and_verifies_the_installer() {
        let body = b"installer bytes".to_vec();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/setup.exe"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let digest: String = Sha256::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut good = ReleaseAsset {
            name: "setup.exe".into(),
            url: format!("{}/setup.exe", server.uri()),
            size: body.len() as u64,
            digest: Some(format!("sha256:{digest}")),
        };
        let seen = std::cell::Cell::new(0);
        let cancel = AtomicBool::new(false);
        let path = download_installer(
            &good,
            dir.path(),
            |d, t| {
                assert_eq!(t, body.len() as u64);
                seen.set(d);
            },
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(seen.get(), body.len() as u64);

        good.digest = Some(format!("sha256:{}", "0".repeat(64)));
        let err = download_installer(&good, dir.path(), |_, _| {}, &cancel)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("校验和"), "{err}");
        assert!(!dir.path().join("setup.exe.part").exists());

        cancel.store(true, AtomicOrdering::Relaxed);
        let err = download_installer(&good, dir.path(), |_, _| {}, &cancel)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("取消"), "{err}");
    }

    async fn server_status(status: u16) -> MockServer {
        server(status, json!({"message": "x"})).await
    }
}
