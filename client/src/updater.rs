//! 应用内自动更新引擎：基于 GitHub Releases API 检测新版本、流式下载、
//! SHA-256 与 Apple 代码签名校验，以及 macOS `.app` 原子替换重启。
//!
//! 零新增 Cargo 依赖：全部网络请求与打包校验均通过系统原生工具（`curl` /
//! `shasum` / `hdiutil` / `ditto` / `codesign`）在后台线程异步执行，天然复用系统代理与 TLS 证书链。

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// GitHub 仓库最新发布接口。
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/chobitsX/githalo/releases/latest";
/// GitHub Releases 页面（用于兜底或查看详情）。
pub const RELEASES_PAGE_URL: &str = "https://github.com/chobitsX/githalo/releases";
/// 自动检查更新的默认冷却间隔（6 小时）。
pub const AUTO_CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;
/// 本项目 Apple Developer ID 团队标识，用于核验已签名更新包的来源一致性。
#[cfg(target_os = "macos")]
const EXPECTED_TEAM_ID: &str = "6L8C6235P9";

/// 语义化版本号（支持 `v0.8.0` 与预发布后缀）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Option<String>,
}

impl SemVer {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim().trim_start_matches(['v', 'V']);
        if trimmed.is_empty() {
            return None;
        }
        // 剥离构建元数据 `+...`
        let without_build = trimmed.split('+').next().unwrap_or(trimmed);
        let (core, pre) = match without_build.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_owned())),
            Some((c, _)) => (c, None),
            None => (without_build, None),
        };
        let mut nums = core.split('.');
        let major = nums.next()?.parse::<u64>().ok()?;
        let minor = nums.next().unwrap_or("0").parse::<u64>().ok()?;
        let patch = nums.next().unwrap_or("0").parse::<u64>().ok()?;
        if nums.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            prerelease: pre,
        })
    }

    pub fn display_clean(&self) -> String {
        match &self.prerelease {
            Some(pre) => format!("{}.{}.{}-{pre}", self.major, self.minor, self.patch),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                // 正式版高于同主次补丁号的预发布版
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 从 GitHub Release 解析出的新版本元信息与下载地址。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseInfo {
    /// 规范化版本号（不带 `v` 前缀，例如 `0.8.1`）。
    pub version: String,
    /// 原始 GitHub Tag（例如 `v0.8.1`）。
    pub tag_name: String,
    /// 发布标题。
    pub name: String,
    /// Release Notes 正文。
    pub notes: String,
    /// Release 页面链接。
    pub html_url: String,
    /// 发布时间（ISO 8601 字符串，可选）。
    pub published_at: Option<String>,
    /// 当前平台对应的安装包直链（macOS 为 `.dmg`）。
    pub asset_url: Option<String>,
    /// 安装包文件名。
    pub asset_name: Option<String>,
    /// 安装包总字节数（用于精准进度条）。
    pub asset_size: Option<u64>,
    /// 对应的 `.sha256` 校验文件直链。
    pub sha256_url: Option<String>,
}

/// 检测更新的结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckUpdateOutcome {
    UpToDate { current: String, latest: String },
    Available(ReleaseInfo),
}

/// 下载与校验阶段的实时进度回报。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: Option<u8>,
    pub stage: DownloadStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadStage {
    Downloading,
    VerifyingChecksum,
    ExtractingApp,
    VerifyingSignature,
}

/// 下载并校验完成的就绪更新包。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedUpdate {
    pub release: ReleaseInfo,
    pub dmg_path: PathBuf,
    /// macOS 下从 `.dmg` 提取并完成签名校验的 `GitHalo.app` 临时路径。
    pub staged_app_path: Option<PathBuf>,
    /// 若当前正从可写的 `.app` 包运行，记录其目标替换路径（如 `/Applications/GitHalo.app`）。
    pub install_target: Option<PathBuf>,
}

impl PreparedUpdate {
    pub fn can_in_place_restart(&self) -> bool {
        self.staged_app_path.is_some() && self.install_target.is_some()
    }
}

/// 执行更新安装后需要主进程配合的动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallAction {
    /// 更新守护脚本已启动，主进程应立即退出以完成原子替换并自动重启。
    RestartNow,
    /// 无法原地替换（如开发运行或目录只读），已改为打开下载好的 `.dmg` 安装包。
    OpenedInstaller,
}

/// 当前 Unix 秒时间戳。
pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 判断是否已超过自动检测冷却间隔（从未检查过时始终返回 `true`）。
pub fn should_auto_check(last_check_epoch: u64, now_epoch: u64) -> bool {
    last_check_epoch == 0 || now_epoch.saturating_sub(last_check_epoch) >= AUTO_CHECK_INTERVAL_SECS
}

/// 请求 GitHub Releases API 检测是否有更新版本。
pub fn check_latest_release(current_version: &str) -> Result<CheckUpdateOutcome, String> {
    let user_agent = format!("GitHalo/{current_version}");
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--connect-timeout",
            "8",
            "--max-time",
            "15",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-GitHub-Api-Version: 2022-11-28",
            "-H",
        ])
        .arg(format!("User-Agent: {user_agent}"))
        .arg(LATEST_RELEASE_API)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法调用系统 curl 检查更新（{e}）"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.contains("403") {
            return Err("检查更新请求过于频繁，请稍后再试或选择手动下载".into());
        }
        return Err(if detail.is_empty() {
            format!("检查更新失败（{}）", output.status)
        } else {
            format!("检查更新失败（{detail}）")
        });
    }

    let body =
        String::from_utf8(output.stdout).map_err(|_| "更新服务返回了无效的响应数据".to_owned())?;
    evaluate_release_json(&body, current_version)
}

/// 解析 GitHub Release JSON 并与当前版本比对。
pub fn evaluate_release_json(
    json: &str,
    current_version: &str,
) -> Result<CheckUpdateOutcome, String> {
    let release = parse_github_release_json(json)?;
    let current = SemVer::parse(current_version)
        .ok_or_else(|| format!("无效的当前版本号：{current_version}"))?;
    let latest = SemVer::parse(&release.version)
        .ok_or_else(|| format!("更新服务返回了无效版本号：{}", release.tag_name))?;

    if latest > current {
        Ok(CheckUpdateOutcome::Available(release))
    } else {
        Ok(CheckUpdateOutcome::UpToDate {
            current: current.display_clean(),
            latest: latest.display_clean(),
        })
    }
}

/// 后台下载新版本安装包并完成校验与准备。
pub fn download_and_prepare_update(
    release: &ReleaseInfo,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PreparedUpdate, String> {
    let Some(asset_url) = release.asset_url.as_deref() else {
        return Err("该版本暂未提供当前平台的安装包，请选择手动下载".into());
    };
    let asset_name = release
        .asset_name
        .clone()
        .unwrap_or_else(|| format!("GitHalo-{}-macos-arm64.dmg", release.version));

    let updates_root = updates_cache_root()?;
    fs::create_dir_all(&updates_root).map_err(|e| format!("无法创建更新缓存目录（{e}）"))?;
    cleanup_old_update_dirs(&updates_root, &release.version);

    let version_dir = updates_root.join(&release.version);
    fs::create_dir_all(&version_dir).map_err(|e| format!("无法创建版本缓存目录（{e}）"))?;
    let dmg_path = version_dir.join(&asset_name);
    let tmp_dmg_path = version_dir.join(format!("{asset_name}.part"));
    let _ = fs::remove_file(&tmp_dmg_path);

    let expected_sha256 = if let Some(sha_url) = release.sha256_url.as_deref() {
        fetch_expected_sha256(sha_url).ok()
    } else {
        None
    };

    // 若缓存中已存在完整且通过 SHA-256 校验的安装包，直接复用跳过重复下载。
    let already_cached = dmg_path.is_file()
        && match expected_sha256.as_deref() {
            Some(expected) => compute_file_sha256(&dmg_path)
                .ok()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected)),
            None => release
                .asset_size
                .is_some_and(|size| fs::metadata(&dmg_path).is_ok_and(|m| m.len() == size)),
        };

    if !already_cached {
        on_progress(DownloadProgress {
            downloaded: 0,
            total: release.asset_size,
            percent: Some(0),
            stage: DownloadStage::Downloading,
        });
        download_file_with_progress(
            asset_url,
            &tmp_dmg_path,
            release.asset_size,
            cancel,
            &mut on_progress,
        )?;
        if cancel.load(AtomicOrdering::Relaxed) {
            let _ = fs::remove_file(&tmp_dmg_path);
            return Err("已取消下载".into());
        }
        fs::rename(&tmp_dmg_path, &dmg_path).map_err(|e| format!("无法保存下载的安装包（{e}）"))?;
    }

    if let Some(expected) = expected_sha256.as_deref() {
        on_progress(DownloadProgress {
            downloaded: release.asset_size.unwrap_or(0),
            total: release.asset_size,
            percent: Some(100),
            stage: DownloadStage::VerifyingChecksum,
        });
        let actual = compute_file_sha256(&dmg_path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&dmg_path);
            return Err("安装包 SHA-256 校验未通过，文件可能已损坏，请重试下载".into());
        }
    }

    #[cfg(target_os = "macos")]
    let (staged_app_path, install_target) = {
        on_progress(DownloadProgress {
            downloaded: release.asset_size.unwrap_or(0),
            total: release.asset_size,
            percent: Some(100),
            stage: DownloadStage::ExtractingApp,
        });
        let staged = extract_app_from_dmg(&dmg_path, &version_dir, cancel)?;
        on_progress(DownloadProgress {
            downloaded: release.asset_size.unwrap_or(0),
            total: release.asset_size,
            percent: Some(100),
            stage: DownloadStage::VerifyingSignature,
        });
        verify_macos_app_signature(&staged)?;
        let target = detect_writable_running_app_bundle();
        (Some(staged), target)
    };

    #[cfg(not(target_os = "macos"))]
    let (staged_app_path, install_target) = (None, None);

    Ok(PreparedUpdate {
        release: release.clone(),
        dmg_path,
        staged_app_path,
        install_target,
    })
}

/// 应用已校验就绪的更新包：支持原地原子替换 `.app` 并重启，或平滑降级打开 `.dmg`。
pub fn apply_prepared_update_and_relaunch(
    prepared: &PreparedUpdate,
) -> Result<InstallAction, String> {
    #[cfg(target_os = "macos")]
    if let (Some(staged_app), Some(target_app)) =
        (&prepared.staged_app_path, &prepared.install_target)
    {
        if !staged_app.is_dir() {
            return Err("已下载的更新包不存在，请重新下载".into());
        }
        spawn_macos_replace_and_relaunch(staged_app, target_app, &prepared.dmg_path)?;
        return Ok(InstallAction::RestartNow);
    }

    open_installer_file(&prepared.dmg_path)?;
    Ok(InstallAction::OpenedInstaller)
}

fn open_installer_file(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("无法打开安装包（{e}）"))?;

    #[cfg(not(target_os = "macos"))]
    let status = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("无法打开安装包（{e}）"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("无法打开安装包（{status}）"))
    }
}

fn updates_cache_root() -> Result<PathBuf, String> {
    dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("githalo").join("updates"))
        .ok_or_else(|| "无法定位系统缓存目录".to_owned())
}

fn cleanup_old_update_dirs(root: &Path, keep_version: &str) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy() != keep_version {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

fn fetch_expected_sha256(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--connect-timeout",
            "8",
            "--max-time",
            "15",
            url,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法下载校验文件（{e}）"))?;
    if !output.status.success() {
        return Err("无法下载 SHA-256 校验文件".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let token = text
        .split_whitespace()
        .next()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| "校验文件格式无效".to_owned())?;
    Ok(token.to_ascii_lowercase())
}

fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法计算 SHA-256（{e}）"))?;
    if !output.status.success() {
        return Err("计算安装包 SHA-256 失败".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .next()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "SHA-256 输出格式异常".to_owned())
}

fn download_file_with_progress(
    url: &str,
    dest: &Path,
    expected_total: Option<u64>,
    cancel: &Arc<AtomicBool>,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), String> {
    let mut child = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--connect-timeout",
            "15",
            "--max-time",
            "600",
            "--output",
        ])
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动下载进程（{e}）"))?;

    let mut last_reported = 0u64;
    loop {
        if cancel.load(AtomicOrdering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(dest);
            return Err("已取消下载".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr_buf = String::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut err, &mut stderr_buf);
                }
                if !status.success() {
                    let _ = fs::remove_file(dest);
                    let detail = stderr_buf.trim();
                    return Err(if detail.is_empty() {
                        format!("下载安装包失败（{status}）")
                    } else {
                        format!("下载安装包失败（{detail}）")
                    });
                }
                let final_size = fs::metadata(dest)
                    .map(|m| m.len())
                    .ok()
                    .or(expected_total)
                    .unwrap_or(0);
                on_progress(DownloadProgress {
                    downloaded: final_size,
                    total: expected_total.or(Some(final_size)),
                    percent: Some(100),
                    stage: DownloadStage::Downloading,
                });
                return Ok(());
            }
            Ok(None) => {
                let written = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                if written != last_reported {
                    last_reported = written;
                    let percent = expected_total
                        .filter(|&t| t > 0)
                        .map(|total| ((written.saturating_mul(100)) / total).min(99) as u8);
                    on_progress(DownloadProgress {
                        downloaded: written,
                        total: expected_total,
                        percent,
                        stage: DownloadStage::Downloading,
                    });
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = fs::remove_file(dest);
                return Err(format!("监控下载进程失败（{e}）"));
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct MountGuard(PathBuf);

#[cfg(target_os = "macos")]
impl Drop for MountGuard {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "macos")]
fn extract_app_from_dmg(
    dmg_path: &Path,
    version_dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let mount_dir = version_dir.join(format!("mount-{}", std::process::id()));
    let _ = fs::remove_dir_all(&mount_dir);
    fs::create_dir_all(&mount_dir).map_err(|e| format!("无法创建挂载目录（{e}）"))?;

    let attach = Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noverify",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mount_dir)
        .arg(dmg_path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法挂载 DMG 镜像（{e}）"))?;
    if !attach.status.success() {
        let _ = fs::remove_dir_all(&mount_dir);
        let err = String::from_utf8_lossy(&attach.stderr);
        return Err(format!("挂载 DMG 镜像失败（{}）", err.trim()));
    }
    let _guard = MountGuard(mount_dir.clone());

    if cancel.load(AtomicOrdering::Relaxed) {
        return Err("已取消更新".into());
    }

    let source_app = mount_dir.join("GitHalo.app");
    if !source_app.is_dir() {
        return Err("DMG 镜像中未找到 GitHalo.app".into());
    }

    let staged_app = version_dir.join("GitHalo.app");
    if staged_app.exists() {
        fs::remove_dir_all(&staged_app).map_err(|e| format!("无法清理旧临时应用包（{e}）"))?;
    }

    let copy = Command::new("ditto")
        .arg(&source_app)
        .arg(&staged_app)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法解压应用包（{e}）"))?;
    if !copy.status.success() {
        let _ = fs::remove_dir_all(&staged_app);
        let err = String::from_utf8_lossy(&copy.stderr);
        return Err(format!("提取 GitHalo.app 失败（{}）", err.trim()));
    }
    Ok(staged_app)
}

#[cfg(target_os = "macos")]
fn verify_macos_app_signature(app_path: &Path) -> Result<(), String> {
    let verify = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app_path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法校验代码签名（{e}）"))?;
    if !verify.status.success() {
        let err = String::from_utf8_lossy(&verify.stderr);
        return Err(format!("新版本代码签名校验失败（{}）", err.trim()));
    }

    let info = Command::new("codesign")
        .args(["-dvv"])
        .arg(app_path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("无法读取签名证书信息（{e}）"))?;
    let details = format!(
        "{}\n{}",
        String::from_utf8_lossy(&info.stdout),
        String::from_utf8_lossy(&info.stderr)
    );
    let expected_line = format!("TeamIdentifier={EXPECTED_TEAM_ID}");
    if !details.lines().any(|line| line.trim() == expected_line) {
        return Err("新版本签名团队标识不匹配，已中止安装以保障安全".into());
    }
    Ok(())
}

/// 判断当前进程是否正从某个 `.app/Contents/MacOS/<bin>` 运行且目标父目录可写。
#[cfg(target_os = "macos")]
pub fn detect_writable_running_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app_bundle = contents_dir.parent()?.to_path_buf();
    if app_bundle.extension()?.to_str()? != "app" {
        return None;
    }
    // 排除位于只读卷（如直接从挂载的 DMG / AppTranslocation 运行）的场景
    let path_str = app_bundle.to_string_lossy();
    if path_str.starts_with("/Volumes/") || path_str.contains("/AppTranslocation/") {
        return None;
    }
    let parent = app_bundle.parent()?;
    let probe = parent.join(format!(".githalo-update-probe-{}", std::process::id()));
    if fs::write(&probe, b"").is_ok() {
        let _ = fs::remove_file(&probe);
        Some(app_bundle)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn spawn_macos_replace_and_relaunch(
    staged_app: &Path,
    target_app: &Path,
    dmg_path: &Path,
) -> Result<(), String> {
    let pid = std::process::id();
    // 独立守护脚本：等待当前进程完全退出 -> 备份原 .app -> 替换新 .app -> 失败自动回滚 -> 重启新版本。
    let script = r#"
PID="$1"
STAGED="$2"
TARGET="$3"
DMG="$4"
BACKUP="${TARGET}.githalo-backup-${PID}"

# 1. 等待当前 GitHalo 进程完全退出（最长 15 秒）
for _ in $(seq 1 150); do
  if ! kill -0 "$PID" 2>/dev/null; then
    break
  fi
  sleep 0.1
done

# 2. 原子备份当前版本并安装新版本
rm -rf "$BACKUP"
if mv "$TARGET" "$BACKUP"; then
  if ditto "$STAGED" "$TARGET"; then
    rm -rf "$BACKUP" "$STAGED" "$DMG"
    open "$TARGET"
    exit 0
  else
    rm -rf "$TARGET"
    mv "$BACKUP" "$TARGET"
    open "$TARGET"
    exit 1
  fi
else
  # 若无法移动目标应用，回退为打开 DMG 供用户手动拖入
  open "$DMG"
  exit 1
fi
"#;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("githalo-updater")
        .arg(pid.to_string())
        .arg(staged_app)
        .arg(target_app)
        .arg(dmg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("无法启动更新安装守护进程（{e}）"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 轻量零依赖 JSON 解析器（专为 GitHub Releases API 响应设计）
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    fn get(&self, key: &str) -> Option<&JsonValue> {
        let Self::Object(entries) = self else {
            return None;
        };
        entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(n) if *n >= 0.0 => Some(*n as u64),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, String> {
        if depth > 64 {
            return Err("JSON 嵌套层级过深".into());
        }
        match self.peek() {
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b't') => self.parse_literal(b"true", JsonValue::Bool(true)),
            Some(b'f') => self.parse_literal(b"false", JsonValue::Bool(false)),
            Some(b'n') => self.parse_literal(b"null", JsonValue::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err("JSON 格式无效".into()),
        }
    }

    fn parse_literal(&mut self, lit: &[u8], val: JsonValue) -> Result<JsonValue, String> {
        if self.bytes.get(self.pos..self.pos + lit.len()) == Some(lit) {
            self.pos += lit.len();
            Ok(val)
        } else {
            Err("JSON 字面量无效".into())
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        while self.pos < self.bytes.len()
            && matches!(
                self.bytes[self.pos],
                b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'
            )
        {
            self.pos += 1;
        }
        let slice =
            std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| "JSON 数字编码无效")?;
        slice
            .parse::<f64>()
            .map(JsonValue::Number)
            .map_err(|_| "JSON 数字格式无效".into())
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err("期望 JSON 字符串起始引号".into());
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let esc = *self.bytes.get(self.pos).ok_or("JSON 转义字符不完整")?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code)
                            && self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u")
                        {
                            self.pos += 2;
                            let low = self.parse_hex4()?;
                            if (0xDC00..=0xDFFF).contains(&low) {
                                let combined = 0x10000
                                    + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                                if let Some(ch) = char::from_u32(combined) {
                                    out.push(ch);
                                }
                            }
                        } else if let Some(ch) = char::from_u32(code as u32) {
                            out.push(ch);
                        }
                    }
                    _ => return Err("不支持的 JSON 转义字符".into()),
                }
            } else {
                // 拷贝一段普通 UTF-8 切片（直到引号或反斜杠）
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && self.bytes[self.pos] != b'"'
                    && self.bytes[self.pos] != b'\\'
                {
                    self.pos += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| "JSON 字符串含非法 UTF-8")?;
                out.push_str(text);
            }
        }
        Err("JSON 字符串未闭合".into())
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let hex = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or("JSON \\u 转义长度不足")?;
        self.pos += 4;
        let text = std::str::from_utf8(hex).map_err(|_| "JSON \\u 转义编码无效")?;
        u16::from_str_radix(text, 16).map_err(|_| "JSON \\u 转义十六进制无效".into())
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.pos += 1; // skip '['
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value(depth)?);
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => return Err("JSON 数组格式错误".into()),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.pos += 1; // skip '{'
        let mut entries = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(entries));
        }
        loop {
            let key = self.parse_string()?;
            if self.peek() != Some(b':') {
                return Err("JSON 对象缺少冒号".into());
            }
            self.pos += 1;
            let value = self.parse_value(depth)?;
            entries.push((key, value));
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(entries));
                }
                _ => return Err("JSON 对象格式错误".into()),
            }
        }
    }
}

pub fn parse_github_release_json(json: &str) -> Result<ReleaseInfo, String> {
    let mut parser = JsonParser::new(json);
    let root = parser.parse_value(0)?;

    if root.get("draft").and_then(JsonValue::as_bool) == Some(true) {
        return Err("最新 Release 为草稿状态".into());
    }

    let tag_name = root
        .get("tag_name")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("Release 响应缺少 tag_name 字段")?
        .to_owned();
    let version = SemVer::parse(&tag_name)
        .map(|v| v.display_clean())
        .unwrap_or_else(|| tag_name.trim_start_matches(['v', 'V']).to_owned());
    let name = root
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&tag_name)
        .to_owned();
    let notes = root
        .get("body")
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let html_url = root
        .get("html_url")
        .and_then(JsonValue::as_str)
        .unwrap_or(RELEASES_PAGE_URL)
        .to_owned();
    let published_at = root
        .get("published_at")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let mut asset_url: Option<String> = None;
    let mut asset_name: Option<String> = None;
    let mut asset_size: Option<u64> = None;
    let mut sha256_url: Option<String> = None;

    if let Some(assets) = root.get("assets").and_then(JsonValue::as_array) {
        let mut fallback_dmg: Option<(String, String, Option<u64>)> = None;
        for asset in assets {
            let Some(aname) = asset.get("name").and_then(JsonValue::as_str) else {
                continue;
            };
            let Some(url) = asset
                .get("browser_download_url")
                .and_then(JsonValue::as_str)
            else {
                continue;
            };
            let size = asset.get("size").and_then(JsonValue::as_u64);
            let lower = aname.to_ascii_lowercase();
            if lower.ends_with(".dmg") {
                if lower.contains("arm64") || lower.contains("aarch64") {
                    asset_name = Some(aname.to_owned());
                    asset_url = Some(url.to_owned());
                    asset_size = size;
                } else if fallback_dmg.is_none() {
                    fallback_dmg = Some((aname.to_owned(), url.to_owned(), size));
                }
            } else if lower.ends_with(".dmg.sha256")
                && (sha256_url.is_none() || lower.contains("arm64"))
            {
                sha256_url = Some(url.to_owned());
            }
        }
        if asset_url.is_none()
            && let Some((aname, url, size)) = fallback_dmg
        {
            asset_name = Some(aname);
            asset_url = Some(url);
            asset_size = size;
        }
    }

    Ok(ReleaseInfo {
        version,
        tag_name,
        name,
        notes,
        html_url,
        published_at,
        asset_url,
        asset_name,
        asset_size,
        sha256_url,
    })
}

/// 将字节大小格式化为易读字符串（例如 `18.4 MB`）。
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing_and_ordering_handle_tags_and_prereleases() {
        let v080 = SemVer::parse("v0.8.0").unwrap();
        let v081 = SemVer::parse("0.8.1").unwrap();
        let v081_beta = SemVer::parse("v0.8.1-beta.1").unwrap();
        let v0100 = SemVer::parse("v0.10.0").unwrap();

        assert!(v081 > v080);
        assert!(v081 > v081_beta);
        assert!(v081_beta > v080);
        assert!(v0100 > v081);
        assert_eq!(SemVer::parse("v0.8.0").unwrap().display_clean(), "0.8.0");
        assert!(SemVer::parse("not-a-version").is_none());
    }

    #[test]
    fn parses_github_release_json_with_assets_unicode_and_escapes() {
        let sample = r###"{
            "tag_name": "v0.8.1",
            "name": "GitHalo v0.8.1",
            "draft": false,
            "prerelease": false,
            "published_at": "2026-11-28T09:00:00Z",
            "html_url": "https://github.com/chobitsX/githalo/releases/tag/v0.8.1",
            "body": "## 更新内容\r\n- 新增应用内自动更新 \u6d4b\u8bd5\n- 修复 \"引号\" 转义",
            "assets": [
                {
                    "name": "GitHalo-0.8.1-macos-arm64.dmg",
                    "size": 18874368,
                    "browser_download_url": "https://github.com/chobitsX/githalo/releases/download/v0.8.1/GitHalo-0.8.1-macos-arm64.dmg"
                },
                {
                    "name": "GitHalo-0.8.1-macos-arm64.dmg.sha256",
                    "size": 98,
                    "browser_download_url": "https://github.com/chobitsX/githalo/releases/download/v0.8.1/GitHalo-0.8.1-macos-arm64.dmg.sha256"
                }
            ]
        }"###;

        let outcome = evaluate_release_json(sample, "0.8.0").expect("valid release json");
        let CheckUpdateOutcome::Available(info) = outcome else {
            panic!("expected Available outcome");
        };
        assert_eq!(info.version, "0.8.1");
        assert_eq!(info.tag_name, "v0.8.1");
        assert!(info.notes.contains("测试"));
        assert!(info.notes.contains("\"引号\""));
        assert_eq!(
            info.asset_name.as_deref(),
            Some("GitHalo-0.8.1-macos-arm64.dmg")
        );
        assert_eq!(info.asset_size, Some(18_874_368));
        assert!(
            info.sha256_url
                .as_deref()
                .is_some_and(|u| u.ends_with(".sha256"))
        );

        let same = evaluate_release_json(sample, "0.8.1").expect("same version");
        assert_eq!(
            same,
            CheckUpdateOutcome::UpToDate {
                current: "0.8.1".into(),
                latest: "0.8.1".into()
            }
        );
    }

    #[test]
    fn auto_check_interval_respects_six_hour_cooldown() {
        assert!(should_auto_check(0, 1_000));
        assert!(!should_auto_check(
            10_000,
            10_000 + AUTO_CHECK_INTERVAL_SECS - 1
        ));
        assert!(should_auto_check(10_000, 10_000 + AUTO_CHECK_INTERVAL_SECS));
    }
}
