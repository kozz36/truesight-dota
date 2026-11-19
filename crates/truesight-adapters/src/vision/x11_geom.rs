#[cfg(target_os = "linux")]
use std::io::{ErrorKind, Read, Write};
#[cfg(target_os = "linux")]
use std::os::linux::net::SocketAddrExt;
#[cfg(target_os = "linux")]
use std::os::unix::net::{SocketAddr, UnixStream};
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(target_os = "linux")]
use std::time::Duration;

/// Root-relative window geometry and map visibility for an X11 window.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct X11WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub viewable: bool,
}

/// Parsed local Unix X11 `DISPLAY` target. Remote/TCP displays are rejected so TCP stays on system tools.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct X11LocalDisplay {
    pub path: String,
    pub display_num: String,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_x11_local_display(display: &str) -> Option<X11LocalDisplay> {
    let trimmed = display.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (host_part, screen_part) = trimmed.rsplit_once(':')?;
    if !host_part.is_empty() && host_part != "unix" {
        return None;
    }
    let disp_num_str = screen_part.split('.').next().filter(|s| !s.is_empty())?;
    let disp_num = disp_num_str.parse::<u16>().ok()?;
    Some(X11LocalDisplay {
        path: format!("/tmp/.X11-unix/X{disp_num}"),
        display_num: disp_num.to_string(),
    })
}

#[cfg(any(target_os = "linux", test))]
fn read_xauth_be_slice<'a>(raw: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let hdr = raw.get(*pos..pos.checked_add(2)?)?;
    let len = usize::from(u16::from_be_bytes([hdr[0], hdr[1]]));
    let start = pos.checked_add(2)?;
    let end = start.checked_add(len)?;
    let slice = raw.get(start..end)?;
    *pos = end;
    Some(slice)
}

/// Parses an `.Xauthority` file for `MIT-MAGIC-COOKIE-1` strictly bound to local Unix display `display_num`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_xauth_cookie_for_local(
    raw: &[u8],
    display_num: &str,
    local_hostname: Option<&str>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    const FAMILY_LOCAL: u16 = 256;
    const FAMILY_WILD: u16 = 65535;
    const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";

    let mut pos = 0_usize;
    let mut wild_fallback: Option<(Vec<u8>, Vec<u8>)> = None;

    while pos.checked_add(2).is_some_and(|end| end <= raw.len()) {
        let fam = u16::from_be_bytes([raw[pos], raw[pos + 1]]);
        pos += 2;
        let addr = read_xauth_be_slice(raw, &mut pos)?;
        let num = read_xauth_be_slice(raw, &mut pos)?;
        let name = read_xauth_be_slice(raw, &mut pos)?;
        let data = read_xauth_be_slice(raw, &mut pos)?;

        if name != COOKIE_NAME {
            continue;
        }
        let matches_display = num == display_num.as_bytes();
        if fam == FAMILY_LOCAL {
            if matches_display
                && local_hostname.is_some_and(|host| !host.is_empty() && addr == host.as_bytes())
            {
                return Some((name.to_vec(), data.to_vec()));
            }
        } else if fam == FAMILY_WILD
            && (matches_display || num.is_empty())
            && wild_fallback.is_none()
        {
            wild_fallback = Some((name.to_vec(), data.to_vec()));
        }
    }

    wild_fallback
}

/// Parses `_NET_FRAME_EXTENTS(CARDINAL) = left, right, top, bottom`, defaulting to `[0, 0, 0, 0]`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_xprop_frame_extents(props_str: &str) -> [u32; 4] {
    for line in props_str.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("_NET_FRAME_EXTENTS") {
            continue;
        }
        let Some((_, rhs)) = trimmed.split_once('=') else {
            continue;
        };
        let mut vals = rhs
            .split(',')
            .map(str::trim)
            .filter_map(|tok| tok.parse::<u32>().ok());
        if let (Some(l), Some(r), Some(t), Some(b)) =
            (vals.next(), vals.next(), vals.next(), vals.next())
        {
            return [l, r, t, b];
        }
    }
    [0, 0, 0, 0]
}

/// Checks whether an occluding window (including WM frame extents `[l, r, t, b]`) overlaps Dota 2's `100%x8%+0+0` top bar.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn top_bar_intersects_window(
    dota: X11WindowGeometry,
    occluder: X11WindowGeometry,
    occluder_frame_extents: [u32; 4],
) -> bool {
    if !dota.viewable
        || !occluder.viewable
        || dota.width == 0
        || dota.height == 0
        || occluder.width == 0
        || occluder.height == 0
    {
        return false;
    }

    let top_bar_h = ((i64::from(dota.height) * 8 + 99) / 100).max(1);
    let dota_left = i64::from(dota.x);
    let dota_right = dota_left + i64::from(dota.width);
    let dota_top = i64::from(dota.y);
    let dota_bottom = dota_top + top_bar_h;

    let [ext_l, ext_r, ext_t, ext_b] = occluder_frame_extents;
    let occ_left = i64::from(occluder.x) - i64::from(ext_l);
    let occ_right = i64::from(occluder.x) + i64::from(occluder.width) + i64::from(ext_r);
    let occ_top = i64::from(occluder.y) - i64::from(ext_t);
    let occ_bottom = i64::from(occluder.y) + i64::from(occluder.height) + i64::from(ext_b);

    dota_left < occ_right && occ_left < dota_right && dota_top < occ_bottom && occ_top < dota_bottom
}

/// Parses `xwininfo -id <wid>` output into `X11WindowGeometry`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_xwininfo_geometry(output: &str) -> Option<X11WindowGeometry> {
    let mut x = None;
    let mut y = None;
    let mut width = None;
    let mut height = None;
    let mut viewable = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("Absolute upper-left X:") {
            x = val.trim().parse::<i32>().ok();
        } else if let Some(val) = trimmed.strip_prefix("Absolute upper-left Y:") {
            y = val.trim().parse::<i32>().ok();
        } else if let Some(val) = trimmed.strip_prefix("Width:") {
            width = val.trim().parse::<u32>().ok();
        } else if let Some(val) = trimmed.strip_prefix("Height:") {
            height = val.trim().parse::<u32>().ok();
        } else if let Some(val) = trimmed.strip_prefix("Map State:") {
            viewable = Some(val.trim() == "IsViewable");
        }
    }

    Some(X11WindowGeometry {
        x: x?,
        y: y?,
        width: width?,
        height: height?,
        viewable: viewable.unwrap_or(true),
    })
}

#[cfg(target_os = "linux")]
fn discard_exact<R: Read>(reader: &mut R, mut remaining: usize) -> std::io::Result<()> {
    let mut scratch = [0_u8; 512];
    while remaining > 0 {
        let n = remaining.min(scratch.len());
        reader.read_exact(&mut scratch[..n])?;
        remaining -= n;
    }
    Ok(())
}

/// Minimal pure-Rust local Unix-socket X11 wire client (`GetGeometry`, `TranslateCoordinates`, `GetWindowAttributes`).
#[cfg(target_os = "linux")]
pub(crate) struct X11GeometryClient<S: Read + Write> {
    stream: S,
    next_seq: u16,
    closed: bool,
}

#[cfg(target_os = "linux")]
impl X11GeometryClient<UnixStream> {
    pub(crate) fn connect() -> Option<Self> {
        let display = std::env::var("DISPLAY").ok()?;
        let target = parse_x11_local_display(&display)?;
        let timeout = Duration::from_millis(250);

        let stream = UnixStream::connect(&target.path).or_else(|_| {
            let addr = SocketAddr::from_abstract_name(target.path.as_bytes())?;
            UnixStream::connect_addr(&addr)
        });
        let stream = stream.ok()?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));

        let (auth_name, auth_data) = load_xauth_cookie(&target.display_num).unwrap_or_default();
        Self::connect_stream(stream, &auth_name, &auth_data)
    }
}

#[cfg(target_os = "linux")]
impl<S: Read + Write> X11GeometryClient<S> {
    pub(crate) fn connect_stream(
        mut stream: S,
        auth_name: &[u8],
        auth_data: &[u8],
    ) -> Option<Self> {
        let name_len = u16::try_from(auth_name.len()).ok()?;
        let data_len = u16::try_from(auth_data.len()).ok()?;
        let name_pad = (4 - (auth_name.len() % 4)) % 4;
        let data_pad = (4 - (auth_data.len() % 4)) % 4;

        let mut req =
            Vec::with_capacity(12 + auth_name.len() + name_pad + auth_data.len() + data_pad);
        req.push(b'l');
        req.push(0);
        req.extend_from_slice(&11_u16.to_le_bytes());
        req.extend_from_slice(&0_u16.to_le_bytes());
        req.extend_from_slice(&name_len.to_le_bytes());
        req.extend_from_slice(&data_len.to_le_bytes());
        req.extend_from_slice(&0_u16.to_le_bytes());
        req.extend_from_slice(auth_name);
        req.resize(req.len() + name_pad, 0);
        req.extend_from_slice(auth_data);
        req.resize(req.len() + data_pad, 0);

        stream.write_all(&req).ok()?;
        stream.flush().ok()?;

        let mut hdr = [0_u8; 8];
        stream.read_exact(&mut hdr).ok()?;
        if hdr[0] != 1 {
            return None;
        }
        let additional_bytes = usize::from(u16::from_le_bytes([hdr[6], hdr[7]])) * 4;
        if additional_bytes > 256 * 1024 {
            return None;
        }
        discard_exact(&mut stream, additional_bytes).ok()?;

        Some(Self {
            stream,
            next_seq: 1,
            closed: false,
        })
    }

    fn alloc_seq(&mut self) -> u16 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    fn discard_extra_words(&mut self, pkt: &[u8; 32]) -> bool {
        let extra_words = u32::from_le_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
        let Some(extra_bytes) = usize::try_from(extra_words)
            .ok()
            .and_then(|n| n.checked_mul(4))
        else {
            self.closed = true;
            return false;
        };
        if extra_bytes > 64 * 1024 || discard_exact(&mut self.stream, extra_bytes).is_err() {
            self.closed = true;
            return false;
        }
        true
    }

    fn send_and_read_reply(&mut self, req: &[u8]) -> Option<[u8; 32]> {
        if self.closed {
            return None;
        }
        let seq = self.alloc_seq();
        if self.stream.write_all(req).is_err() || self.stream.flush().is_err() {
            self.closed = true;
            return None;
        }

        for _ in 0..16 {
            let mut pkt = [0_u8; 32];
            if self.stream.read_exact(&mut pkt).is_err() {
                self.closed = true;
                return None;
            }
            let pkt_seq = u16::from_le_bytes([pkt[2], pkt[3]]);
            match pkt[0] {
                0 => {
                    if pkt_seq == seq {
                        return None;
                    }
                }
                1 => {
                    if !self.discard_extra_words(&pkt) {
                        return None;
                    }
                    if pkt_seq == seq {
                        return Some(pkt);
                    }
                }
                35 if !self.discard_extra_words(&pkt) => return None,
                _ => {}
            }
        }
        None
    }

    /// Queries root-relative position, client size, and `IsViewable` state for `wid`.
    pub(crate) fn query_window_geometry(&mut self, wid: u32) -> Option<X11WindowGeometry> {
        let mut req_geom = [0_u8; 8];
        req_geom[0] = 14; // GetGeometry
        req_geom[2..4].copy_from_slice(&2_u16.to_le_bytes());
        req_geom[4..8].copy_from_slice(&wid.to_le_bytes());
        let r_geom = self.send_and_read_reply(&req_geom)?;
        let root = u32::from_le_bytes([r_geom[8], r_geom[9], r_geom[10], r_geom[11]]);
        let width = u16::from_le_bytes([r_geom[16], r_geom[17]]);
        let height = u16::from_le_bytes([r_geom[18], r_geom[19]]);

        let mut req_trans = [0_u8; 16];
        req_trans[0] = 40; // TranslateCoordinates
        req_trans[2..4].copy_from_slice(&4_u16.to_le_bytes());
        req_trans[4..8].copy_from_slice(&wid.to_le_bytes());
        req_trans[8..12].copy_from_slice(&root.to_le_bytes());
        let r_trans = self.send_and_read_reply(&req_trans)?;
        if r_trans[1] == 0 {
            return None;
        }
        let x = i32::from(i16::from_le_bytes([r_trans[12], r_trans[13]]));
        let y = i32::from(i16::from_le_bytes([r_trans[14], r_trans[15]]));

        let mut req_attrs = [0_u8; 8];
        req_attrs[0] = 3; // GetWindowAttributes
        req_attrs[2..4].copy_from_slice(&2_u16.to_le_bytes());
        req_attrs[4..8].copy_from_slice(&wid.to_le_bytes());
        let r_attrs = self.send_and_read_reply(&req_attrs)?;
        let viewable = r_attrs[26] == 2; // IsViewable

        Some(X11WindowGeometry {
            x,
            y,
            width: u32::from(width),
            height: u32::from(height),
            viewable,
        })
    }
}

#[cfg(target_os = "linux")]
fn load_xauth_cookie(display_num: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let path = std::env::var_os("XAUTHORITY")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".Xauthority"))
        })?;
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > 1024 * 1024 {
        return None;
    }
    let raw = std::fs::read(&path).ok()?;
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string());
    parse_xauth_cookie_for_local(&raw, display_num, hostname.as_deref())
}

#[cfg(target_os = "linux")]
pub(crate) enum XwininfoLookup {
    Geometry(X11WindowGeometry),
    NotInstalled,
    Failed,
}

/// Optional CLI fallback if local X11 wire client lookup is unavailable or fails.
#[cfg(target_os = "linux")]
pub(crate) fn query_xwininfo_geometry(wid: &str) -> XwininfoLookup {
    let out = match Command::new("xwininfo").args(["-id", wid]).output() {
        Ok(out) => out,
        Err(err) if err.kind() == ErrorKind::NotFound => return XwininfoLookup::NotInstalled,
        Err(_) => return XwininfoLookup::Failed,
    };
    if !out.status.success() {
        return XwininfoLookup::Failed;
    }
    match parse_xwininfo_geometry(&String::from_utf8_lossy(&out.stdout)) {
        Some(geom) => XwininfoLookup::Geometry(geom),
        None => XwininfoLookup::Failed,
    }
}
