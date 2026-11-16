use truesight_core::domain::HeroId;

/// Embedded binary RGB atlas for official Dota 2 Arcana and Persona top-bar portraits (`TSAA` v1).
pub const HERO_ALT_ATLAS_BYTES: &[u8] = include_bytes!("../assets/hero_alt_atlas.bin");

#[derive(Debug, Clone)]
pub struct DecodedHeroAltAtlas {
    pub width: usize,
    pub height: usize,
    pub entries: Vec<(HeroId, Vec<u8>)>,
}

/// Decodes the embedded binary RGB alternate portrait atlas (`HERO_ALT_ATLAS_BYTES`).
pub fn parse_hero_alt_atlas() -> Option<DecodedHeroAltAtlas> {
    parse_hero_alt_atlas_from_slice(HERO_ALT_ATLAS_BYTES)
}

/// Parses a `TSAA` alternate-portrait atlas payload (`[TSAA][count:u16][w:u16][h:u16]` + entries).
pub fn parse_hero_alt_atlas_from_slice(bytes: &[u8]) -> Option<DecodedHeroAltAtlas> {
    if bytes.len() < 10 || &bytes[0..4] != b"TSAA" {
        return None;
    }

    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let frame_len = width.checked_mul(height)?.checked_mul(3)?;
    let entry_len = 2usize.checked_add(frame_len)?;
    let expected_total = 10usize.checked_add(count.checked_mul(entry_len)?)?;
    if bytes.len() != expected_total {
        return None;
    }

    let mut offset = 10usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let hid = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        offset += 2;
        entries.push((HeroId(hid), bytes[offset..offset + frame_len].to_vec()));
        offset += frame_len;
    }

    Some(DecodedHeroAltAtlas {
        width,
        height,
        entries,
    })
}
