//! Windows paired maintenance. Store identity and signatures establish the
//! desktop channel; the standalone CLI keeps the vendor updater/package layout.
use super::{Operations, Release, Target, Version};
use crate::windows_deployment::{self as deployment, maintenance};
use std::{
    cell::Cell,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const CLI_RELEASE: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const STORE_PRODUCT: &str = "9PLM9XGG6VKS";

pub(super) struct System {
    store_verified: Cell<bool>,
}
impl System {
    pub fn new() -> Result<Self, String> {
        deployment::primary_home().map_err(|_| "无法定位 Windows 用户目录。")?;
        Ok(Self {
            store_verified: Cell::new(false),
        })
    }
    pub fn lock(&self) -> Result<File, String> {
        maintenance::lock()
    }
    fn cli_version(&self, path: &Path, home: &Path) -> Result<Version, String> {
        let mut command = Command::new(path);
        agent_companion_core::process_environment::isolate_command(&mut command);
        command.env("CODEX_HOME", home).arg("--version");
        let output = maintenance::run(&mut command, Duration::from_secs(20))?;
        let text = String::from_utf8_lossy(&output);
        let version = text
            .trim()
            .strip_prefix("codex-cli ")
            .ok_or("TUI 版本输出无效。")?;
        semver::Version::parse(version).map_err(|_| "TUI 版本无效。")?;
        Ok(Version {
            version: version.into(),
            build: None,
        })
    }
    fn app_version(&self, runtime: &Path) -> Result<Version, String> {
        let version = maintenance::runtime_version(runtime)?;
        Ok(Version {
            build: Some(version.clone()),
            version,
        })
    }
}

impl Operations for System {
    fn always_update_primary(&self, app: bool) -> bool {
        app
    }
    fn confirm_updated_version(&self, app: bool, observed: &Version) -> Result<(), String> {
        // Microsoft Store does not expose a reliable public latest-version
        // response. A successful exact Store upgrade, re-location and signature
        // check establish the new desktop version; no prerelease feed is used.
        if app
            && self.store_verified.get()
            && self.installed(Target::CodexApp)?.as_ref() == Some(observed)
        {
            return Ok(());
        }
        if !app && self.latest(false)?.version == *observed {
            return Ok(());
        }
        Err("更新后的实际版本尚无法确认为官方稳定渠道，未同步副实例。".into())
    }
    fn installed(&self, target: Target) -> Result<Option<Version>, String> {
        match target {
            Target::CodexApp => self
                .app_version(&maintenance::official_desktop()?)
                .map(Some),
            Target::CodexTui => {
                let Some(package) = maintenance::optional_official_tui()? else {
                    // An npm-managed primary is outside standalone version
                    // maintenance; it does not make a working Dodex invalid.
                    return Ok(None);
                };
                self.cli_version(
                    &package.package.join("bin/codex.exe"),
                    &deployment::primary_home().map_err(|_| "无法定位主账号目录。")?,
                )
                .map(Some)
            }
            Target::DodexApp | Target::DodexTui => {
                let Some(instance) = maintenance::verified_instance()? else {
                    return Ok(None);
                };
                if target == Target::DodexApp {
                    self.app_version(
                        instance
                            .runtime_app
                            .parent()
                            .ok_or("Dodex App 路径无效。")?,
                    )
                    .map(Some)
                } else if instance.cli_path.exists() {
                    self.cli_version(&instance.cli_path, &instance.codex_home)
                        .map(Some)
                } else {
                    Ok(None)
                }
            }
        }
    }
    fn latest(&self, app: bool) -> Result<Release, String> {
        if app {
            // A Store latest-version placeholder is only a planning floor. The
            // explicit update action always invokes WinGet then replaces this
            // floor with the re-observed signed desktop version.
            return Ok(Release {
                version: self.app_version(&maintenance::official_desktop()?)?,
                url: format!("msstore:{STORE_PRODUCT}"),
                length: 0,
            });
        }
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .user_agent(concat!("agent-companion/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        let mut response = agent
            .get(CLI_RELEASE)
            .call()
            .map_err(|_| "无法检查官方 TUI 稳定版。")?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法读取官方 TUI 发布信息。")?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("官方发布信息过大。".into());
        }
        parse_cli_release(&bytes)
    }
    fn preflight(&self, cli: bool, app: bool) -> Result<(), String> {
        maintenance::preflight()?;
        if cli {
            maintenance::official_tui()?;
        }
        if app {
            maintenance::official_desktop()?;
        }
        Ok(())
    }
    fn cli_needs_sync(&self) -> bool {
        maintenance::needs_sync(false)
    }
    fn app_needs_sync(&self) -> bool {
        maintenance::needs_sync(true)
    }
    fn require_stopped(&self) -> Result<(), String> {
        maintenance::require_stopped()
    }
    fn update_primary(&self, app: bool, release: &Release) -> Result<(), String> {
        self.require_stopped()?;
        if release.length != 0
            || (app && release.url != format!("msstore:{STORE_PRODUCT}"))
            || (!app && !release.url.is_empty())
        {
            return Err("Windows 维护来源不受支持；未运行更新器。".into());
        }
        if app {
            let winget = winget()?;
            let mut command = Command::new(winget);
            agent_companion_core::process_environment::isolate_command(&mut command);
            command.args(winget_arguments());
            maintenance::run(&mut command, Duration::from_secs(1200))?;
            // WinGet exit code alone is insufficient: rediscover the exact
            // package identity and recheck OpenAI Authenticode after updating.
            let runtime = maintenance::official_desktop()?;
            self.app_version(&runtime)?;
            self.store_verified.set(true);
        } else {
            let package = maintenance::official_tui()?;
            let mut command = Command::new(package.package.join("bin/codex.exe"));
            agent_companion_core::process_environment::isolate_command(&mut command);
            command
                .env(
                    "CODEX_HOME",
                    deployment::primary_home().map_err(|_| "无法定位主账号目录。")?,
                )
                .arg("update");
            maintenance::run(&mut command, Duration::from_secs(1200))?;
            maintenance::official_tui()?;
        }
        Ok(())
    }
    fn align_secondary(&self, app: bool, expected: &Version) -> Result<(), String> {
        if app {
            maintenance::align_desktop(&expected.version)
        } else {
            maintenance::align_tui(&expected.version)
        }
    }
}

fn winget_arguments() -> [&'static str; 8] {
    [
        "upgrade",
        "--id",
        STORE_PRODUCT,
        "--exact",
        "--source",
        "msstore",
        "--silent",
        "--disable-interactivity",
    ]
}

fn winget() -> Result<PathBuf, String> {
    let path = deployment::powershell(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $p=@(Get-AppxPackage -Name Microsoft.DesktopAppInstaller | Where-Object { $_.PackageFamilyName -eq 'Microsoft.DesktopAppInstaller_8wekyb3d8bbwe' -and $_.Status -eq 'Ok' }); if ($p.Count -ne 1) { exit 1 }; $exe=Join-Path $p[0].InstallLocation 'winget.exe'; $s=Get-AuthenticodeSignature -LiteralPath $exe; if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation,') { exit 1 }; $exe",
        None,
    )?;
    Ok(PathBuf::from(path))
}

fn parse_cli_release(bytes: &[u8]) -> Result<Release, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "官方 TUI 发布信息无效。")?;
    let version = value["tag_name"]
        .as_str()
        .and_then(|tag| tag.strip_prefix("rust-v"))
        .ok_or("官方 TUI 标签无效。")?;
    let parsed = semver::Version::parse(version).map_err(|_| "官方 TUI 标签无效。")?;
    if value["draft"] != false || value["prerelease"] != false || !parsed.pre.is_empty() {
        return Err("官方 TUI 发布不是稳定版。".into());
    }
    Ok(Release {
        version: Version {
            version: version.into(),
            build: None,
        },
        url: String::new(),
        length: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_upgrade_is_exact_noninteractive_and_never_forces_or_accepts_agreements() {
        let args = winget_arguments();
        assert!(args.windows(2).any(|pair| pair == ["--id", "9PLM9XGG6VKS"]));
        assert!(args.windows(2).any(|pair| pair == ["--source", "msstore"]));
        for required in ["upgrade", "--exact", "--silent", "--disable-interactivity"] {
            assert!(args.contains(&required));
        }
        for forbidden in [
            "--force",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ] {
            assert!(!args.contains(&forbidden));
        }
    }
    #[test]
    fn not_applicable_store_exit_cannot_confirm_latest() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "exit", "/B", "-1978335189"]);
        assert!(maintenance::run(&mut command, Duration::from_secs(10)).is_err());
    }
    #[test]
    fn only_official_stable_cli_release_metadata_is_accepted() {
        assert_eq!(
            parse_cli_release(br#"{"tag_name":"rust-v0.160.0","draft":false,"prerelease":false}"#)
                .unwrap()
                .version
                .version,
            "0.160.0"
        );
        for fixture in [
            br#"{"tag_name":"rust-v0.161.0-alpha.1","draft":false,"prerelease":false}"#.as_slice(),
            br#"{"tag_name":"rust-v0.161.0","draft":true,"prerelease":false}"#,
        ] {
            assert!(parse_cli_release(fixture).is_err());
        }
    }
}
