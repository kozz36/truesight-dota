use crate::meta_repo::SeededMetaRepository;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use truesight_core::domain::HeroId;
use truesight_core::ports::PortError;

/// OpenDota `/api/heroStats` refreshes its rolling daily window once per 24h UTC.
pub const META_TTL_SECS: u64 = 86_400;
/// OpenDota `/api/players/{id}/heroes` updates within minutes of completing a match; refresh hourly.
pub const PLAYER_TTL_SECS: u64 = 3_600;

pub const DEFAULT_PLAYER_ACCOUNT_ID: u64 = 127_255_741;

#[derive(Debug, Clone)]
pub struct LocalCacheManager {
    pub cache_dir: PathBuf,
    pub player_account_id: u64,
}

impl Default for LocalCacheManager {
    fn default() -> Self {
        Self::new(DEFAULT_PLAYER_ACCOUNT_ID)
    }
}

impl LocalCacheManager {
    pub fn new(player_account_id: u64) -> Self {
        let base = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let cache_dir = base.join(".local/share/truesight-dota/cache");
        Self {
            cache_dir,
            player_account_id,
        }
    }

    pub fn with_dir(cache_dir: PathBuf, player_account_id: u64) -> Self {
        Self {
            cache_dir,
            player_account_id,
        }
    }

    pub fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn metadata_path(&self) -> PathBuf {
        self.cache_dir.join("cache_metadata.txt")
    }

    fn meta_json_path(&self) -> PathBuf {
        self.cache_dir.join("opendota_hero_stats.json")
    }

    fn player_json_path(&self) -> PathBuf {
        self.cache_dir.join("player_heroes.json")
    }

    fn player_matches_json_path(&self) -> PathBuf {
        self.cache_dir.join("player_matches.json")
    }

    pub fn read_timestamps(&self) -> (Option<u64>, Option<u64>) {
        let Ok(content) = fs::read_to_string(self.metadata_path()) else {
            return (None, None);
        };
        let mut meta_ts = None;
        let mut player_ts = None;
        for line in content.lines() {
            if let Some(v) = line.strip_prefix("meta_sync_unix=") {
                meta_ts = v.trim().parse::<u64>().ok();
            } else if let Some(v) = line.strip_prefix("player_sync_unix=") {
                player_ts = v.trim().parse::<u64>().ok();
            }
        }
        (meta_ts, player_ts)
    }

    pub fn is_meta_stale(&self, now_unix: u64) -> bool {
        let (meta_ts, _) = self.read_timestamps();
        match meta_ts {
            Some(ts) if self.meta_json_path().exists() => now_unix.saturating_sub(ts) > META_TTL_SECS,
            _ => true,
        }
    }

    pub fn is_player_stale(&self, now_unix: u64) -> bool {
        let (_, player_ts) = self.read_timestamps();
        match player_ts {
            Some(ts) if self.player_json_path().exists() => {
                now_unix.saturating_sub(ts) > PLAYER_TTL_SECS
            }
            _ => true,
        }
    }

    pub fn write_cached_files(
        &self,
        meta_json: Option<&str>,
        player_json: Option<&str>,
        now_unix: u64,
    ) -> Result<(), PortError> {
        fs::create_dir_all(&self.cache_dir).map_err(|e| PortError(e.to_string()))?;
        let (mut meta_ts, mut player_ts) = self.read_timestamps();

        if let Some(m_body) = meta_json {
            fs::write(self.meta_json_path(), m_body).map_err(|e| PortError(e.to_string()))?;
            meta_ts = Some(now_unix);
        }
        if let Some(p_body) = player_json {
            fs::write(self.player_json_path(), p_body).map_err(|e| PortError(e.to_string()))?;
            player_ts = Some(now_unix);
        }

        let mut meta_txt = String::new();
        if let Some(mts) = meta_ts {
            meta_txt.push_str(&format!("meta_sync_unix={mts}\n"));
        }
        if let Some(pts) = player_ts {
            meta_txt.push_str(&format!("player_sync_unix={pts}\n"));
        }
        fs::write(self.metadata_path(), meta_txt).map_err(|e| PortError(e.to_string()))?;
        Ok(())
    }

    /// Synchronizes stale or forced endpoints from OpenDota with a strict 6-second timeout.
    pub fn sync_from_opendota(&self, force: bool) -> Result<String, PortError> {
        let now = Self::now_unix();
        let need_meta = force || self.is_meta_stale(now);
        let need_player = force || self.is_player_stale(now);

        if !need_meta && !need_player {
            return Ok("DB cache is already fresh (Meta <24h, Player <1h).".into());
        }

        let mut fetched_meta = None;
        let mut fetched_player = None;
        let mut updated_parts = Vec::new();

        if need_meta {
            let body = fetch_url("https://api.opendota.com/api/heroStats")?;
            if body.trim_start().starts_with('[') {
                fetched_meta = Some(body);
                updated_parts.push("heroStats (24h TTL)");
            }
        }

        if need_player {
            let url = format!(
                "https://api.opendota.com/api/players/{}/heroes",
                self.player_account_id
            );
            let body = fetch_url(&url)?;
            if body.trim_start().starts_with('[') {
                fetched_player = Some(body);
                updated_parts.push("player comfort (1h TTL)");
            }
            let matches_url = format!(
                "https://api.opendota.com/api/players/{}/matches?limit=500",
                self.player_account_id
            );
            if let Ok(m_body) = fetch_url(&matches_url) {
                if m_body.trim_start().starts_with('[') {
                    let _ = fs::create_dir_all(&self.cache_dir);
                    let _ = fs::write(self.player_matches_json_path(), m_body);
                }
            }
        }

        self.write_cached_files(
            fetched_meta.as_deref(),
            fetched_player.as_deref(),
            now,
        )?;

        Ok(format!("Synced OpenDota: {}.", updated_parts.join(" + ")))
    }

    pub fn freshness_badge(&self, now_unix: u64) -> String {
        let (meta_ts, player_ts) = self.read_timestamps();
        let meta_str = meta_ts
            .map(|ts| format_age(now_unix.saturating_sub(ts)))
            .unwrap_or_else(|| "embedded".into());
        let player_str = player_ts
            .map(|ts| format_age(now_unix.saturating_sub(ts)))
            .unwrap_or_else(|| "embedded".into());
        format!("DB [Meta: {meta_str} | Player: {player_str}]")
    }

    pub fn build_repository(&self) -> SeededMetaRepository {
        let mut repo = SeededMetaRepository::new();
        if let Ok(m_json) = fs::read_to_string(self.meta_json_path()) {
            repo.custom_hero_meta = parse_hero_stats_json(&m_json);
        }
        if let Ok(p_json) = fs::read_to_string(self.player_json_path()) {
            let matches_json = fs::read_to_string(self.player_matches_json_path()).ok();
            repo.custom_personal_pool =
                parse_player_comfort_with_matches_json(&p_json, matches_json.as_deref());
        }
        repo
    }
}

/// Lightweight parser for OpenDota `/api/heroStats` JSON array.
/// Returns `Vec<(HeroId, bracket_winrates[8], trend_delta)>`.
pub fn parse_hero_stats_json(json: &str) -> Vec<(HeroId, [f32; 8], f32)> {
    let mut rows = Vec::new();
    for obj_chunk in json.split('{').skip(1) {
        let Some(end) = obj_chunk.find('}') else {
            continue;
        };
        let block = &obj_chunk[..end];
        let Some(hid) = extract_u64_field(block, "\"id\"") else {
            continue;
        };
        let mut wrs = [0.50_f32; 8];
        let mut has_any = false;
        for t in 1..=7 {
            let pk = format!("\"{t}_pick\"");
            let wk = format!("\"{t}_win\"");
            if let (Some(p), Some(w)) = (extract_u64_field(block, &pk), extract_u64_field(block, &wk))
            {
                if p > 0 {
                    wrs[t - 1] = ((w as f32 + 25.0 * 0.50) / (p as f32 + 25.0)).clamp(0.35, 0.65);
                    has_any = true;
                }
            }
        }
        if !has_any {
            continue;
        }
        wrs[7] = (wrs[6] + 0.35 * (wrs[6] - wrs[4])).clamp(0.38, 0.60);

        let pt = extract_u64_array(block, "\"pub_pick_trend\"");
        let wt = extract_u64_array(block, "\"pub_win_trend\"");
        let trend_delta = if pt.len() >= 6 && wt.len() >= 6 {
            let p_sum: u64 = pt.iter().sum();
            let w_sum: u64 = wt.iter().sum();
            if p_sum > 0 {
                let week_wr = (w_sum as f32) / (p_sum as f32);
                let rec_p = (pt[pt.len() - 3] + pt[pt.len() - 2]) as f32;
                let rec_w = (wt[wt.len() - 3] + wt[wt.len() - 2]) as f32;
                let rec_wr = if rec_p > 0.0 {
                    (rec_w + 200.0 * week_wr) / (rec_p + 200.0)
                } else {
                    week_wr
                };
                (rec_wr - week_wr).clamp(-0.025, 0.025)
            } else {
                0.0
            }
        } else {
            0.0
        };

        rows.push((HeroId(hid as u16), wrs, trend_delta));
    }
    rows
}

fn extract_u64_array(block: &str, key: &str) -> Vec<u64> {
    let Some(pos) = block.find(key) else {
        return Vec::new();
    };
    let after = &block[pos + key.len()..];
    let Some(open) = after.find('[') else {
        return Vec::new();
    };
    let Some(close) = after[open..].find(']') else {
        return Vec::new();
    };
    let inside = &after[open + 1..open + close];
    inside
        .split(',')
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect()
}

fn format_age(age_secs: u64) -> String {
    if age_secs < 60 {
        format!("{age_secs}s ago")
    } else if age_secs < 3600 {
        format!("{}m ago", age_secs / 60)
    } else if age_secs < 86_400 {
        format!("{}h ago", age_secs / 3600)
    } else {
        format!("{}d ago", age_secs / 86_400)
    }
}

fn fetch_url(url: &str) -> Result<String, PortError> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "6", url])
        .output()
        .map_err(|e| PortError(format!("Failed to execute curl: {e}")))?;
    if !out.status.success() {
        return Err(PortError(format!(
            "HTTP fetch failed for {url} (status {:?})",
            out.status.code()
        )));
    }
    String::from_utf8(out.stdout).map_err(|e| PortError(e.to_string()))
}

/// Lightweight parser for OpenDota `/api/players/{id}/heroes` JSON array.
/// Returns `Vec<(HeroId, games, wins, comfort_decay)>`.
pub fn parse_player_comfort_json(json: &str) -> Vec<(HeroId, u32, u32, f32)> {
    parse_player_comfort_with_matches_json(json, None)
}

/// Parses OpenDota `/api/players/{id}/heroes` combined with optional `/api/players/{id}/matches`
/// to compute per-match exponentially time-decayed effective games and wins (`tau = 75d`, floor `0.0`).
pub fn parse_player_comfort_with_matches_json(
    heroes_json: &str,
    matches_json: Option<&str>,
) -> Vec<(HeroId, u32, u32, f32)> {
    const TAU_DAYS: f32 = 75.0;

    let mut raw_rows: Vec<(u16, u64, u32, u32)> = Vec::new();
    let mut max_ts = 0_u64;

    for obj_chunk in heroes_json.split('{').skip(1) {
        let Some(end) = obj_chunk.find('}') else {
            continue;
        };
        let block = &obj_chunk[..end];
        let Some(hid) = extract_u64_field(block, "\"hero_id\"") else {
            continue;
        };
        let last_played = extract_u64_field(block, "\"last_played\"").unwrap_or(0);
        let games = extract_u64_field(block, "\"games\"").unwrap_or(0) as u32;
        let wins = extract_u64_field(block, "\"win\"").unwrap_or(0) as u32;
        if games > 0 && last_played > max_ts {
            max_ts = last_played;
        }
        raw_rows.push((hid as u16, last_played, games, wins));
    }

    // Optional per-match recency accumulator: (g_eff, w_eff, match_cnt) indexed by hero_id
    let mut match_acc = [(0.0_f32, 0.0_f32, 0_u32); truesight_core::domain::MAX_HEROES];
    let mut has_matches = false;

    if let Some(m_json) = matches_json {
        for obj_chunk in m_json.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            if let Some(st) = extract_u64_field(block, "\"start_time\"") {
                if st > max_ts {
                    max_ts = st;
                }
            }
        }
        for obj_chunk in m_json.split('{').skip(1) {
            let Some(end) = obj_chunk.find('}') else {
                continue;
            };
            let block = &obj_chunk[..end];
            let Some(hid) = extract_u64_field(block, "\"hero_id\"") else {
                continue;
            };
            let idx = hid as usize;
            if idx >= truesight_core::domain::MAX_HEROES {
                continue;
            }
            let Some(st) = extract_u64_field(block, "\"start_time\"") else {
                continue;
            };
            let slot = extract_u64_field(block, "\"player_slot\"").unwrap_or(0);
            let rad_win = extract_bool_field(block, "\"radiant_win\"").unwrap_or(false);
            let won = (slot < 128) == rad_win;
            let age_days = (max_ts.saturating_sub(st)) as f32 / 86_400.0;
            let w = (-age_days / TAU_DAYS).exp();
            match_acc[idx].0 += w;
            if won {
                match_acc[idx].1 += w;
            }
            match_acc[idx].2 += 1;
            has_matches = true;
        }
    }

    raw_rows
        .into_iter()
        .map(|(hid, last_played, games, wins)| {
            if games == 0 {
                return (HeroId(hid), 0, 0, 0.0);
            }
            let idx = hid as usize;
            let (g_eff, w_eff, cnt) = if has_matches && idx < truesight_core::domain::MAX_HEROES {
                match_acc[idx]
            } else {
                (0.0, 0.0, 0)
            };

            if cnt > 0 {
                let decay = (g_eff / (games as f32)).clamp(0.0, 1.0);
                let career_wr = (wins as f32) / (games as f32);
                let rec_wr = if g_eff > 0.01 { w_eff / g_eff } else { career_wr };
                let blended_wr = 0.65 * rec_wr + 0.35 * career_wr;
                let eff_wins = ((games as f32) * blended_wr).round() as u32;
                (HeroId(hid), games, eff_wins, decay)
            } else {
                let decay = if last_played > 0 && max_ts > 0 {
                    let age_days = (max_ts.saturating_sub(last_played)) as f32 / 86_400.0;
                    (-age_days / TAU_DAYS).exp().clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (HeroId(hid), games, wins, decay)
            }
        })
        .collect()
}

fn extract_bool_field(block: &str, key: &str) -> Option<bool> {
    let pos = block.find(key)?;
    let after_key = &block[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if after_colon.starts_with("true") {
        Some(true)
    } else if after_colon.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_u64_field(block: &str, key: &str) -> Option<u64> {
    let pos = block.find(key)?;
    let after_key = &block[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    let digits: String = after_colon
        .chars()
        .skip_while(|c| *c == '"')
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().ok()
}
