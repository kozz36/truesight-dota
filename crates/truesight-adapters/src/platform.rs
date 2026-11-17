use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use truesight_core::ports::PortError;

/// Snapshot of OS environment variables and temp directory used for pure path resolution tests
/// without mutating process-global environment variables across parallel test threads.
#[derive(Debug, Clone, Default)]
pub(crate) struct PlatformEnv {
    pub home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub localappdata: Option<PathBuf>,
    pub userprofile: Option<PathBuf>,
    pub program_files_x86: Option<PathBuf>,
    pub program_files: Option<PathBuf>,
    pub temp_dir: PathBuf,
}

fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    let val = std::env::var_os(key)?;
    if val.is_empty() {
        return None;
    }
    Some(PathBuf::from(val))
}

impl PlatformEnv {
    /// Captures process environment paths, discarding empty env vars and anchoring fallbacks on
    /// `std::env::temp_dir()`.
    pub(crate) fn from_process() -> Self {
        Self {
            home: non_empty_env_path("HOME"),
            appdata: non_empty_env_path("APPDATA"),
            localappdata: non_empty_env_path("LOCALAPPDATA"),
            userprofile: non_empty_env_path("USERPROFILE"),
            program_files_x86: non_empty_env_path("ProgramFiles(x86)"),
            program_files: non_empty_env_path("ProgramFiles"),
            temp_dir: std::env::temp_dir(),
        }
    }
}

/// Applies platform-specific background execution flags so helper processes (`curl`, `reg`)
/// do not flash transient console windows when launched from the Windows GUI subsystem build.
pub(crate) fn configure_background_command(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cmd;
}

/// Resolves the persisted account-link `config.toml` path for the target OS.
///
/// Precedence on Windows avoids `$HOME` and `/tmp`: `%APPDATA%` -> `%LOCALAPPDATA%` ->
/// `%USERPROFILE%\AppData\Roaming` -> `temp_dir`, always under `truesight-dota\config.toml`.
/// Non-Windows targets preserve `$HOME/.config/truesight-dota/config.toml` (or `temp_dir`).
pub(crate) fn resolve_config_file_path_for(os: &str, env: &PlatformEnv) -> PathBuf {
    if os == "windows" {
        if let Some(base) = env.appdata.as_deref().or(env.localappdata.as_deref()) {
            return base.join("truesight-dota").join("config.toml");
        }
        if let Some(profile) = env.userprofile.as_deref().or(env.home.as_deref()) {
            return profile
                .join("AppData")
                .join("Roaming")
                .join("truesight-dota")
                .join("config.toml");
        }
        return env.temp_dir.join("truesight-dota").join("config.toml");
    }
    let base = env.home.as_deref().unwrap_or(&env.temp_dir);
    base.join(".config")
        .join("truesight-dota")
        .join("config.toml")
}

/// Resolves the local OpenDota/STRATZ cache directory for the target OS.
///
/// Precedence on Windows avoids `$HOME` and `/tmp`: `%LOCALAPPDATA%` -> `%APPDATA%` ->
/// `%USERPROFILE%\AppData\Local` -> `temp_dir`. If only `temp_dir` is available, cache TTL
/// persistence across reboots is degraded until local AppData paths exist.
pub(crate) fn resolve_cache_dir_for(os: &str, env: &PlatformEnv) -> PathBuf {
    if os == "windows" {
        if let Some(base) = env.localappdata.as_deref().or(env.appdata.as_deref()) {
            return base.join("truesight-dota").join("cache");
        }
        if let Some(profile) = env.userprofile.as_deref().or(env.home.as_deref()) {
            return profile
                .join("AppData")
                .join("Local")
                .join("truesight-dota")
                .join("cache");
        }
        return env.temp_dir.join("truesight-dota").join("cache");
    }
    let base = env.home.as_deref().unwrap_or(&env.temp_dir);
    base.join(".local")
        .join("share")
        .join("truesight-dota")
        .join("cache")
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn parse_windows_reg_steam_path(output: &str) -> Option<PathBuf> {
    for line in output.lines() {
        let trimmed = line.trim();
        let Some(after_key) = trimmed.strip_prefix("SteamPath") else {
            continue;
        };
        if !after_key.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = after_key.trim_start();
        if !rest.starts_with("REG_") {
            continue;
        }
        let Some((_, value)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        let cleaned = value.trim().trim_matches('"');
        if !cleaned.is_empty() && !cleaned.contains('\u{FFFD}') {
            return Some(PathBuf::from(cleaned));
        }
    }
    None
}

/// Strictly decodes `reg.exe query` stdout as UTF-8, rejecting lossy OEM bytes.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn parse_windows_reg_steam_path_bytes(stdout: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(stdout).ok()?;
    parse_windows_reg_steam_path(text)
}

/// Strictly decodes a raw UTF-8 `SteamPath` string emitted without OEM code-page translation.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn decode_windows_steam_path_utf8(stdout: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(stdout).ok()?;
    let cleaned = text.trim().trim_matches('"');
    if cleaned.is_empty() || cleaned.contains('\u{FFFD}') {
        return None;
    }
    Some(PathBuf::from(cleaned))
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn build_windows_steam_path_powershell_args() -> [&'static str; 4] {
    [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $p = (Get-ItemProperty -LiteralPath 'Registry::HKEY_CURRENT_USER\\Software\\Valve\\Steam' -Name 'SteamPath' -ErrorAction SilentlyContinue).SteamPath; if ($p) { [Console]::Out.Write($p) }",
    ]
}

/// Builds ordered `loginusers.vdf` lookup paths for the target OS without touching the registry.
///
/// On Windows, standard filesystem directories (`%ProgramFiles(x86)%`, `%ProgramFiles%`,
/// `%LOCALAPPDATA%`) precede any `reg_steam_path` override so the common case avoids spawning
/// `reg.exe`. Deduplicates identical paths while preserving priority order.
pub(crate) fn steam_vdf_candidates_for(
    os: &str,
    env: &PlatformEnv,
    reg_steam_path: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    };
    if os == "windows" {
        for base in [
            env.program_files_x86.as_deref(),
            env.program_files.as_deref(),
            env.localappdata.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            push_unique(base.join("Steam").join("config").join("loginusers.vdf"));
        }
        if let Some(reg_root) = reg_steam_path {
            push_unique(reg_root.join("config").join("loginusers.vdf"));
        }
    } else if let Some(home) = env.home.as_deref() {
        push_unique(
            home.join(".local")
                .join("share")
                .join("Steam")
                .join("config")
                .join("loginusers.vdf"),
        );
        push_unique(
            home.join(".steam")
                .join("steam")
                .join("config")
                .join("loginusers.vdf"),
        );
    }
    candidates
}

pub(crate) fn detect_steam_vdf_in_candidates(candidates: &[PathBuf]) -> Option<u64> {
    for candidate in candidates {
        if let Ok(content) = fs::read_to_string(candidate) {
            if let Some(steam32) = crate::sync::parse_loginusers_vdf(&content) {
                return Some(steam32);
            }
        }
    }
    None
}

/// Fallback Steam install discovery via `HKCU\Software\Valve\Steam\SteamPath`.
///
/// Queries PowerShell with `[Console]::OutputEncoding = UTF8Encoding` so non-ASCII install
/// paths are emitted losslessly regardless of the console OEM code page, and falls back to
/// strict UTF-8 `reg query` parsing (never `from_utf8_lossy`).
#[cfg(target_os = "windows")]
pub(crate) fn query_windows_steam_path() -> Option<PathBuf> {
    let mut ps_cmd = Command::new("powershell");
    configure_background_command(&mut ps_cmd);
    if let Ok(out) = ps_cmd
        .args(build_windows_steam_path_powershell_args())
        .output()
    {
        if out.status.success() {
            if let Some(path) = decode_windows_steam_path_utf8(&out.stdout) {
                return Some(path);
            }
        }
    }

    let mut reg_cmd = Command::new("reg");
    configure_background_command(&mut reg_cmd);
    let out = reg_cmd
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_windows_reg_steam_path_bytes(&out.stdout)
}

pub(crate) fn build_curl_args<'a>(url: &'a str, extra_args: &'a [&'a str]) -> Vec<&'a str> {
    let mut args = Vec::with_capacity(3 + extra_args.len() + 1);
    args.extend(["-fsSL", "--max-time", "6"]);
    args.extend_from_slice(extra_args);
    args.push(url);
    args
}

/// Executes external `curl` (`curl.exe` on Windows `PATH` / `System32`) with a 6s timeout
/// and `CREATE_NO_WINDOW` applied on Windows to prevent GUI console flashes.
pub(crate) fn run_curl(url: &str, extra_args: &[&str], context: &str) -> Result<String, PortError> {
    let mut cmd = Command::new("curl");
    configure_background_command(&mut cmd);
    let args = build_curl_args(url, extra_args);
    let out = cmd.args(&args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PortError(format!(
                "Failed to execute curl for {context}: executable not found on PATH ({e})"
            ))
        } else {
            PortError(format!("Failed to execute curl for {context}: {e}"))
        }
    })?;
    if !out.status.success() {
        return Err(PortError(format!(
            "{context} failed (status {:?})",
            out.status.code()
        )));
    }
    String::from_utf8(out.stdout).map_err(|e| PortError(e.to_string()))
}
