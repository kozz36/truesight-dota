use super::BoundedVisionWorker;

/// Platform capture backend classification for live top-bar vision recognition.
/// Extension contract: `#12` wires `WindowsNative`; `#13` wires `WaylandPortal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackendKind {
    X11,
    WaylandPortal,
    WindowsNative,
    Unsupported,
}

/// Startup capability state returned by the cross-platform capture factory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureAvailability {
    Available {
        backend: CaptureBackendKind,
        detail: String,
    },
    Unavailable {
        backend: CaptureBackendKind,
        reason: String,
    },
}

impl CaptureAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn backend(&self) -> CaptureBackendKind {
        match self {
            Self::Available { backend, .. } | Self::Unavailable { backend, .. } => *backend,
        }
    }

    pub fn badge_label(&self) -> &'static str {
        match self {
            Self::Available {
                backend: CaptureBackendKind::X11,
                ..
            } => "CAP: X11",
            Self::Available {
                backend: CaptureBackendKind::WaylandPortal,
                ..
            } => "CAP: WAYLAND",
            Self::Available {
                backend: CaptureBackendKind::WindowsNative,
                ..
            } => "CAP: WIN32",
            Self::Available {
                backend: CaptureBackendKind::Unsupported,
                ..
            }
            | Self::Unavailable { .. } => "CAP: MANUAL",
        }
    }

    pub fn summary(&self) -> &str {
        match self {
            Self::Available { detail, .. } => detail.as_str(),
            Self::Unavailable { reason, .. } => reason.as_str(),
        }
    }
}

fn command_on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Pure OS/capability classifier used by `detect_capture_availability` and unit tests.
pub fn detect_capture_availability_for(
    os: &str,
    wayland_session: bool,
    has_x11_display: bool,
    has_xprop: bool,
    has_import: bool,
) -> CaptureAvailability {
    match os {
        "linux" if has_x11_display && has_xprop && has_import => CaptureAvailability::Available {
            backend: CaptureBackendKind::X11,
            detail: "X11/XWayland (xprop + import)".into(),
        },
        "linux" if wayland_session && !has_x11_display => CaptureAvailability::Unavailable {
            backend: CaptureBackendKind::WaylandPortal,
            reason: "Native Wayland capture is not implemented yet (#13); manual pick mode & GSI remain active.".into(),
        },
        "linux" => {
            let mut missing = Vec::new();
            if !has_x11_display {
                missing.push("DISPLAY");
            }
            if !has_xprop {
                missing.push("xprop");
            }
            if !has_import {
                missing.push("import (ImageMagick)");
            }
            CaptureAvailability::Unavailable {
                backend: CaptureBackendKind::X11,
                reason: format!("X11 capture unavailable (missing {}); manual pick mode & GSI remain active.", missing.join(", ")),
            }
        }
        "windows" => CaptureAvailability::Unavailable {
            backend: CaptureBackendKind::WindowsNative,
            reason: "Native Windows capture backend is not implemented yet (#12); manual pick mode & GSI remain active.".into(),
        },
        other => CaptureAvailability::Unavailable {
            backend: CaptureBackendKind::Unsupported,
            reason: format!("Screen capture is unsupported on `{other}`; manual pick mode & GSI remain active."),
        },
    }
}

/// Inspects current runtime environment and OS tools to select the capture backend.
pub fn detect_capture_availability() -> CaptureAvailability {
    let wayland = std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let has_display = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    let has_xprop = cfg!(target_os = "linux") && command_on_path("xprop");
    let has_import = cfg!(target_os = "linux") && command_on_path("import");
    detect_capture_availability_for(
        std::env::consts::OS,
        wayland,
        has_display,
        has_xprop,
        has_import,
    )
}

/// Spawns the OS-appropriate capture worker when supported, returning `(None, availability)` otherwise.
pub fn spawn_platform_worker() -> (Option<BoundedVisionWorker>, CaptureAvailability) {
    let availability = detect_capture_availability();
    spawn_platform_worker_for(&availability)
}

/// Spawns a platform worker matching `availability`; downgrades unimplemented backends to `Unavailable`.
pub fn spawn_platform_worker_for(
    availability: &CaptureAvailability,
) -> (Option<BoundedVisionWorker>, CaptureAvailability) {
    #[cfg(target_os = "linux")]
    if matches!(
        availability,
        CaptureAvailability::Available {
            backend: CaptureBackendKind::X11,
            ..
        }
    ) {
        if let Some(worker) =
            BoundedVisionWorker::spawn_with_source(super::X11TopBarFrameSource::new())
        {
            return (Some(worker), availability.clone());
        }
        return (
            None,
            CaptureAvailability::Unavailable {
                backend: CaptureBackendKind::X11,
                reason:
                    "Failed to initialize X11 vision worker; manual pick mode & GSI remain active."
                        .into(),
            },
        );
    }
    match availability {
        CaptureAvailability::Available { backend, .. } => (
            None,
            CaptureAvailability::Unavailable {
                backend: *backend,
                reason: format!(
                    "Capture backend `{backend:?}` is unavailable on `{}`; manual pick mode & GSI remain active.",
                    std::env::consts::OS
                ),
            },
        ),
        CaptureAvailability::Unavailable { .. } => (None, availability.clone()),
    }
}
