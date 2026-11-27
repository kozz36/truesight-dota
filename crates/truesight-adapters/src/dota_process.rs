/// Returns `true` when a Linux `/proc/<pid>/comm` value identifies the Dota 2 client (`dota2`).
pub fn parse_proc_comm_is_dota2(comm: &str) -> bool {
    comm.trim() == "dota2"
}

/// Returns `true` when Windows `tasklist /FO CSV /NH` output includes a `dota2.exe` row.
pub fn parse_tasklist_has_dota2(csv_output: &str) -> bool {
    for raw_line in csv_output.lines() {
        let line = raw_line.trim();
        let Some(after_quote) = line.strip_prefix('"') else {
            continue;
        };
        let Some((exe, _)) = after_quote.split_once('"') else {
            continue;
        };
        let basename = exe.trim().rsplit(['\\', '/']).next().unwrap_or("");
        if basename.eq_ignore_ascii_case("dota2.exe") {
            return true;
        }
    }
    false
}

/// Passively detects whether Dota 2 (`dota2` on Linux, `dota2.exe` on Windows) is currently
/// running without reading process memory or attaching graphics hooks.
pub fn is_dota2_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        is_dota2_running_linux()
    }
    #[cfg(target_os = "windows")]
    {
        is_dota2_running_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn is_dota2_running_linux() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let bytes = name.as_encoded_bytes();
        if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm"))
            && parse_proc_comm_is_dota2(&comm)
        {
            return true;
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn is_dota2_running_windows() -> bool {
    let sys_root = std::env::var("SystemRoot").ok();
    let windir = std::env::var("WINDIR").ok();
    let tasklist_path = crate::vision::resolve_windows_system32_exe(
        sys_root.as_deref(),
        windir.as_deref(),
        "tasklist.exe",
    );
    let mut cmd = std::process::Command::new(tasklist_path);
    crate::platform::configure_background_command(&mut cmd);
    let Ok(out) = cmd
        .args(["/FI", "IMAGENAME eq dota2.exe", "/FO", "CSV", "/NH"])
        .output()
    else {
        return false;
    };
    out.status.success() && parse_tasklist_has_dota2(&String::from_utf8_lossy(&out.stdout))
}
