use eframe::egui::{self, Color32, TextureFilter, TextureId, TexturesDelta, epaint::Primitive};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

struct RasterTexture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
    linear: bool,
    repeat: bool,
}

impl RasterTexture {
    fn sample(&self, u: f32, v: f32) -> Color32 {
        if self.width == 0 || self.height == 0 || self.pixels.is_empty() {
            return Color32::WHITE;
        }
        let (u, v) = if self.repeat {
            (u.rem_euclid(1.0), v.rem_euclid(1.0))
        } else {
            (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
        };
        if !self.linear || (self.width == 1 && self.height == 1) {
            let tx = ((u * self.width as f32).floor() as usize).min(self.width - 1);
            let ty = ((v * self.height as f32).floor() as usize).min(self.height - 1);
            return self.pixels[ty * self.width + tx];
        }

        let fx = if self.repeat {
            (u * self.width as f32 - 0.5).rem_euclid(self.width as f32)
        } else {
            (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32)
        };
        let fy = if self.repeat {
            (v * self.height as f32 - 0.5).rem_euclid(self.height as f32)
        } else {
            (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32)
        };
        let x0 = (fx.floor() as usize).min(self.width - 1);
        let y0 = (fy.floor() as usize).min(self.height - 1);
        let (x1, y1) = if self.repeat {
            ((x0 + 1) % self.width, (y0 + 1) % self.height)
        } else {
            ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1))
        };
        let wx = fx - x0 as f32;
        let wy = fy - y0 as f32;

        let p00 = self.pixels[y0 * self.width + x0];
        let p10 = self.pixels[y0 * self.width + x1];
        let p01 = self.pixels[y1 * self.width + x0];
        let p11 = self.pixels[y1 * self.width + x1];

        let bilerp = |c00: u8, c10: u8, c01: u8, c11: u8| -> u8 {
            let top = c00 as f32 + (c10 as f32 - c00 as f32) * wx;
            let bot = c01 as f32 + (c11 as f32 - c01 as f32) * wx;
            (top + (bot - top) * wy).round().clamp(0.0, 255.0) as u8
        };

        Color32::from_rgba_premultiplied(
            bilerp(p00[0], p10[0], p01[0], p11[0]),
            bilerp(p00[1], p10[1], p01[1], p11[1]),
            bilerp(p00[2], p10[2], p01[2], p11[2]),
            bilerp(p00[3], p10[3], p01[3], p11[3]),
        )
    }

    fn uniform_single_pixel(&self) -> Option<Color32> {
        if self.width == 1 && self.height == 1 {
            self.pixels.first().copied()
        } else {
            None
        }
    }
}

/// Pure-safe-Rust software rasterizer for tessellated `egui` meshes into Wayland `wl_shm`
/// premultiplied `ARGB8888` little-endian (`[B, G, R, A]`) pixel buffers.
pub(super) struct OverlayRasterizer {
    textures: HashMap<TextureId, RasterTexture>,
    texture_epoch: u64,
    last_frame_hash: Option<u64>,
    buffer: Vec<u8>,
    width_px: u32,
    height_px: u32,
}

impl Default for OverlayRasterizer {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayRasterizer {
    /// Creates an empty software rasterizer with no uploaded textures or cached frame.
    pub(super) fn new() -> Self {
        Self {
            textures: HashMap::new(),
            texture_epoch: 0,
            last_frame_hash: None,
            buffer: Vec::new(),
            width_px: 0,
            height_px: 0,
        }
    }

    /// Returns the count of resident textures (used by deterministic rasterizer tests).
    #[cfg(test)]
    pub(super) fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Invalidates the cached dirty-frame hash so the next frame always re-rasterizes.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(super) fn invalidate(&mut self) {
        self.last_frame_hash = None;
    }

    /// Applies incremental `TexturesDelta` uploads, sub-rectangle patches, and evictions, clearing
    /// `delta` afterwards as required by `epaint`'s drop invariant.
    pub(super) fn apply_textures_delta(&mut self, delta: &mut TexturesDelta) {
        if delta.set.is_empty() && delta.free.is_empty() {
            return;
        }
        for (id, deltas) in &delta.set {
            for image_delta in deltas {
                let egui::ImageData::Color(img) = &image_delta.image;
                let linear = image_delta.options.magnification == TextureFilter::Linear;
                let repeat = image_delta.options.wrap_mode == egui::TextureWrapMode::Repeat;
                if let Some([pos_x, pos_y]) = image_delta.pos
                    && let Some(existing) = self.textures.get_mut(id)
                {
                    let patch_w = img.size[0];
                    let patch_h = img.size[1];
                    for sy in 0..patch_h {
                        let dy = pos_y + sy;
                        if dy >= existing.height {
                            break;
                        }
                        for sx in 0..patch_w {
                            let dx = pos_x + sx;
                            if dx >= existing.width {
                                break;
                            }
                            if let Some(&px) = img.pixels.get(sy * patch_w + sx) {
                                existing.pixels[dy * existing.width + dx] = px;
                            }
                        }
                    }
                    existing.linear = linear;
                    existing.repeat = repeat;
                } else {
                    self.textures.insert(
                        *id,
                        RasterTexture {
                            width: img.size[0],
                            height: img.size[1],
                            pixels: img.pixels.clone(),
                            linear,
                            repeat,
                        },
                    );
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
        delta.clear();
        self.texture_epoch = self.texture_epoch.wrapping_add(1);
    }

    /// Rasterizes `primitives` into a premultiplied little-endian `ARGB8888` buffer when the
    /// dimensions, texture epoch, or display list changed since the previous call.
    pub(super) fn rasterize_if_dirty(
        &mut self,
        width_px: u32,
        height_px: u32,
        pixels_per_point: f32,
        primitives: &[egui::ClippedPrimitive],
    ) -> Option<&[u8]> {
        if width_px == 0 || height_px == 0 {
            return None;
        }
        let hash = compute_frame_hash(
            width_px,
            height_px,
            pixels_per_point,
            self.texture_epoch,
            primitives,
        );
        if self.last_frame_hash == Some(hash)
            && self.width_px == width_px
            && self.height_px == height_px
        {
            return None;
        }

        let total_bytes = (width_px as usize)
            .saturating_mul(height_px as usize)
            .saturating_mul(4);
        self.buffer.clear();
        self.buffer.resize(total_bytes, 0);
        self.width_px = width_px;
        self.height_px = height_px;
        self.last_frame_hash = Some(hash);

        let scale = pixels_per_point.max(0.5);
        for clipped in primitives {
            let Primitive::Mesh(mesh) = &clipped.primitive else {
                continue;
            };
            if mesh.indices.len() < 3 || mesh.vertices.is_empty() {
                continue;
            }
            let clip_min_x = (clipped.clip_rect.min.x * scale).floor().max(0.0) as i32;
            let clip_min_y = (clipped.clip_rect.min.y * scale).floor().max(0.0) as i32;
            let clip_max_x = (clipped.clip_rect.max.x * scale)
                .ceil()
                .min(width_px as f32) as i32;
            let clip_max_y = (clipped.clip_rect.max.y * scale)
                .ceil()
                .min(height_px as f32) as i32;
            if clip_min_x >= clip_max_x || clip_min_y >= clip_max_y {
                continue;
            }

            let tex = self.textures.get(&mesh.texture_id);
            let mut idx_pos = 0;
            while idx_pos + 2 < mesh.indices.len() {
                // Fast-path consecutive triangle pairs forming an axis-aligned quad:
                // either constant-UV solid fill (including font-atlas solids) or separable textured fill.
                if idx_pos + 5 < mesh.indices.len() {
                    let quad = &mesh.indices[idx_pos..idx_pos + 6];
                    if let Some((rect_min_x, rect_min_y, rect_max_x, rect_max_y, fill)) =
                        try_extract_uniform_quad(&mesh.vertices, quad, scale, tex)
                    {
                        let x0 = rect_min_x.max(clip_min_x);
                        let y0 = rect_min_y.max(clip_min_y);
                        let x1 = rect_max_x.min(clip_max_x);
                        let y1 = rect_max_y.min(clip_max_y);
                        if x0 < x1 && y0 < y1 {
                            fill_rect_premultiplied(
                                &mut self.buffer,
                                width_px,
                                x0,
                                y0,
                                x1,
                                y1,
                                fill,
                            );
                        }
                        idx_pos += 6;
                        continue;
                    }
                    if let Some(t) = tex
                        && let Some(tquad) = try_extract_textured_quad(&mesh.vertices, quad, scale)
                    {
                        fill_textured_quad_premultiplied(
                            &mut self.buffer,
                            width_px,
                            (clip_min_x, clip_min_y, clip_max_x, clip_max_y),
                            tquad,
                            t,
                        );
                        idx_pos += 6;
                        continue;
                    }
                }

                let i0 = mesh.indices[idx_pos] as usize;
                let i1 = mesh.indices[idx_pos + 1] as usize;
                let i2 = mesh.indices[idx_pos + 2] as usize;
                idx_pos += 3;

                let (Some(&v0), Some(&v1), Some(&v2)) = (
                    mesh.vertices.get(i0),
                    mesh.vertices.get(i1),
                    mesh.vertices.get(i2),
                ) else {
                    continue;
                };

                rasterize_triangle(
                    &mut self.buffer,
                    width_px,
                    scale,
                    (clip_min_x, clip_min_y, clip_max_x, clip_max_y),
                    v0,
                    v1,
                    v2,
                    tex,
                );
            }
        }

        Some(&self.buffer)
    }
}

fn try_extract_uniform_quad(
    vertices: &[egui::epaint::Vertex],
    quad_indices: &[u32],
    scale: f32,
    tex: Option<&RasterTexture>,
) -> Option<(i32, i32, i32, i32, [u8; 4])> {
    let mut uniq = [usize::MAX; 4];
    let mut uniq_len = 0_usize;
    for &raw_idx in quad_indices {
        let idx = raw_idx as usize;
        if !uniq[..uniq_len].contains(&idx) {
            if uniq_len == 4 {
                return None;
            }
            uniq[uniq_len] = idx;
            uniq_len += 1;
        }
    }
    if uniq_len != 4 {
        return None;
    }

    let first_v = vertices.get(uniq[0])?;
    let first_col = first_v.color;
    let first_uv = first_v.uv;
    let uniform_uv_needed = tex.and_then(RasterTexture::uniform_single_pixel).is_none();
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &idx in &uniq {
        let v = vertices.get(idx)?;
        if v.color != first_col {
            return None;
        }
        if uniform_uv_needed
            && ((v.uv.x - first_uv.x).abs() > 1e-5 || (v.uv.y - first_uv.y).abs() > 1e-5)
        {
            return None;
        }
        let px = v.pos.x * scale;
        let py = v.pos.y * scale;
        min_x = min_x.min(px);
        min_y = min_y.min(py);
        max_x = max_x.max(px);
        max_y = max_y.max(py);
    }
    for &idx in &uniq {
        let v = vertices.get(idx)?;
        let px = v.pos.x * scale;
        let py = v.pos.y * scale;
        let on_x = (px - min_x).abs() < 0.01 || (px - max_x).abs() < 0.01;
        let on_y = (py - min_y).abs() < 0.01 || (py - max_y).abs() < 0.01;
        if !on_x || !on_y {
            return None;
        }
    }

    let x0 = min_x.round() as i32;
    let y0 = min_y.round() as i32;
    let x1 = max_x.round() as i32;
    let y1 = max_y.round() as i32;
    if (min_x - x0 as f32).abs() > 0.05
        || (min_y - y0 as f32).abs() > 0.05
        || (max_x - x1 as f32).abs() > 0.05
        || (max_y - y1 as f32).abs() > 0.05
    {
        return None;
    }

    let tex_color = match tex {
        Some(t) => t
            .uniform_single_pixel()
            .unwrap_or_else(|| t.sample(first_uv.x, first_uv.y)),
        None => Color32::WHITE,
    };
    let modulated = modulate_premultiplied(tex_color, first_col);
    Some((x0, y0, x1, y1, modulated))
}

struct TexturedQuad {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    u_left: f32,
    u_right: f32,
    v_top: f32,
    v_bot: f32,
    vert_col: Color32,
}

fn try_extract_textured_quad(
    vertices: &[egui::epaint::Vertex],
    quad_indices: &[u32],
    scale: f32,
) -> Option<TexturedQuad> {
    let mut uniq = [usize::MAX; 4];
    let mut uniq_len = 0_usize;
    for &raw_idx in quad_indices {
        let idx = raw_idx as usize;
        if !uniq[..uniq_len].contains(&idx) {
            if uniq_len == 4 {
                return None;
            }
            uniq[uniq_len] = idx;
            uniq_len += 1;
        }
    }
    if uniq_len != 4 {
        return None;
    }

    let first_col = vertices.get(uniq[0])?.color;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &idx in &uniq {
        let v = vertices.get(idx)?;
        if v.color != first_col {
            return None;
        }
        let px = v.pos.x * scale;
        let py = v.pos.y * scale;
        min_x = min_x.min(px);
        min_y = min_y.min(py);
        max_x = max_x.max(px);
        max_y = max_y.max(py);
    }
    if (max_x - min_x) <= 0.0 || (max_y - min_y) <= 0.0 {
        return None;
    }

    let mut lt = None;
    let mut rt = None;
    let mut rb = None;
    let mut lb = None;
    for &idx in &uniq {
        let v = vertices.get(idx)?;
        let px = v.pos.x * scale;
        let py = v.pos.y * scale;
        let is_l = (px - min_x).abs() < 0.01;
        let is_r = (px - max_x).abs() < 0.01;
        let is_t = (py - min_y).abs() < 0.01;
        let is_b = (py - max_y).abs() < 0.01;
        match (is_l, is_r, is_t, is_b) {
            (true, false, true, false) => lt = Some(v.uv),
            (false, true, true, false) => rt = Some(v.uv),
            (false, true, false, true) => rb = Some(v.uv),
            (true, false, false, true) => lb = Some(v.uv),
            _ => return None,
        }
    }
    let (lt, rt, rb, lb) = (lt?, rt?, rb?, lb?);
    if (lt.x - lb.x).abs() > 1e-4
        || (rt.x - rb.x).abs() > 1e-4
        || (lt.y - rt.y).abs() > 1e-4
        || (lb.y - rb.y).abs() > 1e-4
    {
        return None;
    }

    Some(TexturedQuad {
        min_x,
        min_y,
        max_x,
        max_y,
        u_left: lt.x,
        u_right: rt.x,
        v_top: lt.y,
        v_bot: lb.y,
        vert_col: first_col,
    })
}

fn fill_textured_quad_premultiplied(
    buffer: &mut [u8],
    stride_px: u32,
    clip: (i32, i32, i32, i32),
    quad: TexturedQuad,
    tex: &RasterTexture,
) {
    if tex.width == 0 || tex.height == 0 || tex.pixels.is_empty() || quad.vert_col[3] == 0 {
        return;
    }
    let (clip_min_x, clip_min_y, clip_max_x, clip_max_y) = clip;
    let x0 = ((quad.min_x - 0.5).ceil() as i32).max(clip_min_x);
    let y0 = ((quad.min_y - 0.5).ceil() as i32).max(clip_min_y);
    let x1 = ((quad.max_x - 0.5).ceil() as i32).min(clip_max_x);
    let y1 = ((quad.max_y - 0.5).ceil() as i32).min(clip_max_y);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let inv_w = 1.0 / (quad.max_x - quad.min_x);
    let inv_h = 1.0 / (quad.max_y - quad.min_y);
    let du = quad.u_right - quad.u_left;
    let dv = quad.v_bot - quad.v_top;
    let tw = tex.width;
    let th = tex.height;
    let nearest = !tex.linear || (tw == 1 && th == 1);

    for y in y0..y1 {
        let ty = ((y as f32 + 0.5 - quad.min_y) * inv_h).clamp(0.0, 1.0);
        let raw_v = quad.v_top + dv * ty;
        let v = if tex.repeat {
            raw_v.rem_euclid(1.0)
        } else {
            raw_v.clamp(0.0, 1.0)
        };
        let row_offset = y as usize * stride_px as usize;

        if nearest {
            let py_idx = ((v * th as f32).floor() as usize).min(th - 1);
            let row = &tex.pixels[py_idx * tw..(py_idx + 1) * tw];
            for x in x0..x1 {
                let tx = ((x as f32 + 0.5 - quad.min_x) * inv_w).clamp(0.0, 1.0);
                let raw_u = quad.u_left + du * tx;
                let u = if tex.repeat {
                    raw_u.rem_euclid(1.0)
                } else {
                    raw_u.clamp(0.0, 1.0)
                };
                let px_idx = ((u * tw as f32).floor() as usize).min(tw - 1);
                let src_rgba = modulate_premultiplied(row[px_idx], quad.vert_col);
                let off = (row_offset + x as usize) * 4;
                blend_argb8888_pixel(&mut buffer[off..off + 4], src_rgba);
            }
        } else {
            let fy = if tex.repeat {
                (v * th as f32 - 0.5).rem_euclid(th as f32)
            } else {
                (v * th as f32 - 0.5).clamp(0.0, (th - 1) as f32)
            };
            let sy0 = (fy.floor() as usize).min(th - 1);
            let sy1 = if tex.repeat {
                (sy0 + 1) % th
            } else {
                (sy0 + 1).min(th - 1)
            };
            let wy_fp = ((fy - sy0 as f32) * 256.0).round().clamp(0.0, 256.0) as u32;
            let inv_wy = 256 - wy_fp;
            let row0 = &tex.pixels[sy0 * tw..(sy0 + 1) * tw];
            let row1 = &tex.pixels[sy1 * tw..(sy1 + 1) * tw];

            for x in x0..x1 {
                let tx = ((x as f32 + 0.5 - quad.min_x) * inv_w).clamp(0.0, 1.0);
                let raw_u = quad.u_left + du * tx;
                let u = if tex.repeat {
                    raw_u.rem_euclid(1.0)
                } else {
                    raw_u.clamp(0.0, 1.0)
                };
                let fx = if tex.repeat {
                    (u * tw as f32 - 0.5).rem_euclid(tw as f32)
                } else {
                    (u * tw as f32 - 0.5).clamp(0.0, (tw - 1) as f32)
                };
                let sx0 = (fx.floor() as usize).min(tw - 1);
                let sx1 = if tex.repeat {
                    (sx0 + 1) % tw
                } else {
                    (sx0 + 1).min(tw - 1)
                };
                let wx_fp = ((fx - sx0 as f32) * 256.0).round().clamp(0.0, 256.0) as u32;
                let inv_wx = 256 - wx_fp;
                let p00 = row0[sx0];
                let p10 = row0[sx1];
                let p01 = row1[sx0];
                let p11 = row1[sx1];
                let bilerp_ch = |ch: usize| -> u8 {
                    let top = p00[ch] as u32 * inv_wx + p10[ch] as u32 * wx_fp;
                    let bot = p01[ch] as u32 * inv_wx + p11[ch] as u32 * wx_fp;
                    (((top * inv_wy + bot * wy_fp) + 32768) >> 16).min(255) as u8
                };
                let tex_col = Color32::from_rgba_premultiplied(
                    bilerp_ch(0),
                    bilerp_ch(1),
                    bilerp_ch(2),
                    bilerp_ch(3),
                );
                let src_rgba = modulate_premultiplied(tex_col, quad.vert_col);
                if src_rgba[3] != 0 {
                    let off = (row_offset + x as usize) * 4;
                    blend_argb8888_pixel(&mut buffer[off..off + 4], src_rgba);
                }
            }
        }
    }
}

#[inline]
fn modulate_premultiplied(tex: Color32, vert: Color32) -> [u8; 4] {
    let mul = |a: u8, b: u8| -> u8 { ((a as u16 * b as u16 + 127) / 255) as u8 };
    [
        mul(tex[0], vert[0]),
        mul(tex[1], vert[1]),
        mul(tex[2], vert[2]),
        mul(tex[3], vert[3]),
    ]
}

#[inline]
fn blend_argb8888_pixel(dst: &mut [u8], src_rgba: [u8; 4]) {
    let [sr, sg, sb, sa] = src_rgba;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        dst[0] = sb;
        dst[1] = sg;
        dst[2] = sr;
        dst[3] = 255;
        return;
    }
    let inv_a = (255 - sa) as u16;
    dst[0] = sb.saturating_add(((dst[0] as u16 * inv_a + 127) / 255) as u8);
    dst[1] = sg.saturating_add(((dst[1] as u16 * inv_a + 127) / 255) as u8);
    dst[2] = sr.saturating_add(((dst[2] as u16 * inv_a + 127) / 255) as u8);
    dst[3] = sa.saturating_add(((dst[3] as u16 * inv_a + 127) / 255) as u8);
}

fn fill_rect_premultiplied(
    buffer: &mut [u8],
    stride_px: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    src_rgba: [u8; 4],
) {
    if src_rgba[3] == 0 {
        return;
    }
    let argb_bytes = [src_rgba[2], src_rgba[1], src_rgba[0], src_rgba[3]];
    for y in y0..y1 {
        let row_start = (y as usize * stride_px as usize + x0 as usize) * 4;
        let count = (x1 - x0) as usize;
        if src_rgba[3] == 255 {
            for px in buffer[row_start..row_start + count * 4]
                .as_chunks_mut::<4>()
                .0
            {
                px.copy_from_slice(&argb_bytes);
            }
        } else {
            for px in buffer[row_start..row_start + count * 4]
                .as_chunks_mut::<4>()
                .0
            {
                blend_argb8888_pixel(px, src_rgba);
            }
        }
    }
}

#[inline]
fn is_top_left_edge(ax: f32, ay: f32, bx: f32, by: f32) -> bool {
    let dy = by - ay;
    let dx = bx - ax;
    dy < 0.0 || (dy == 0.0 && dx > 0.0)
}

#[inline]
fn edge_function_canonical(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    if ay < by || (ay == by && ax <= bx) {
        (bx - ax) * (py - ay) - (by - ay) * (px - ax)
    } else {
        -((ax - bx) * (py - by) - (ay - by) * (px - bx))
    }
}

#[expect(clippy::too_many_arguments)]
fn rasterize_triangle(
    buffer: &mut [u8],
    stride_px: u32,
    scale: f32,
    clip: (i32, i32, i32, i32),
    v0: egui::epaint::Vertex,
    mut v1: egui::epaint::Vertex,
    mut v2: egui::epaint::Vertex,
    tex: Option<&RasterTexture>,
) {
    let p0x = v0.pos.x * scale;
    let p0y = v0.pos.y * scale;
    let mut p1x = v1.pos.x * scale;
    let mut p1y = v1.pos.y * scale;
    let mut p2x = v2.pos.x * scale;
    let mut p2y = v2.pos.y * scale;

    let mut area = (p1x - p0x) * (p2y - p0y) - (p1y - p0y) * (p2x - p0x);
    if area.abs() < 1e-4 {
        return;
    }
    if area < 0.0 {
        std::mem::swap(&mut v1, &mut v2);
        std::mem::swap(&mut p1x, &mut p2x);
        std::mem::swap(&mut p1y, &mut p2y);
        area = -area;
    }
    let inv_area = 1.0 / area;

    // Top-left edge ownership rule (Y-down CCW winding) ensures shared edges between adjacent
    // triangles are rasterized by exactly one triangle with zero seam overlap or gaps.
    let tl0 = is_top_left_edge(p1x, p1y, p2x, p2y);
    let tl1 = is_top_left_edge(p2x, p2y, p0x, p0y);
    let tl2 = is_top_left_edge(p0x, p0y, p1x, p1y);

    let (clip_min_x, clip_min_y, clip_max_x, clip_max_y) = clip;
    let min_x = (p0x.min(p1x).min(p2x).floor() as i32).max(clip_min_x);
    let min_y = (p0y.min(p1y).min(p2y).floor() as i32).max(clip_min_y);
    let max_x = (p0x.max(p1x).max(p2x).ceil() as i32).min(clip_max_x);
    let max_y = (p0y.max(p1y).max(p2y).ceil() as i32).min(clip_max_y);
    if min_x >= max_x || min_y >= max_y {
        return;
    }

    let uniform_vert = v0.color == v1.color && v1.color == v2.color;
    let uniform_uv = (v0.uv.x - v1.uv.x).abs() < 1e-5
        && (v1.uv.x - v2.uv.x).abs() < 1e-5
        && (v0.uv.y - v1.uv.y).abs() < 1e-5
        && (v1.uv.y - v2.uv.y).abs() < 1e-5;
    let const_tex_col = if uniform_uv {
        Some(tex.map_or(Color32::WHITE, |t| t.sample(v0.uv.x, v0.uv.y)))
    } else {
        None
    };
    let const_src_rgba = match (uniform_vert, const_tex_col) {
        (true, Some(tc)) => {
            let rgba = modulate_premultiplied(tc, v0.color);
            if rgba[3] == 0 {
                return;
            }
            Some(rgba)
        }
        _ => None,
    };

    for y in min_y..max_y {
        let py = y as f32 + 0.5;
        let row_offset = y as usize * stride_px as usize;
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let e0 = edge_function_canonical(p1x, p1y, p2x, p2y, px, py);
            let e1 = edge_function_canonical(p2x, p2y, p0x, p0y, px, py);
            let e2 = edge_function_canonical(p0x, p0y, p1x, p1y, px, py);
            if !(e0 > 0.0 || (e0 == 0.0 && tl0))
                || !(e1 > 0.0 || (e1 == 0.0 && tl1))
                || !(e2 > 0.0 || (e2 == 0.0 && tl2))
            {
                continue;
            }
            let src_rgba = if let Some(rgba) = const_src_rgba {
                rgba
            } else {
                let w0 = (e0 * inv_area).clamp(0.0, 1.0);
                let w1 = (e1 * inv_area).clamp(0.0, 1.0);
                let w2 = (e2 * inv_area).clamp(0.0, 1.0);

                let vert_col = if uniform_vert {
                    v0.color
                } else {
                    let interp = |c0: u8, c1: u8, c2: u8| -> u8 {
                        (c0 as f32 * w0 + c1 as f32 * w1 + c2 as f32 * w2)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    };
                    Color32::from_rgba_premultiplied(
                        interp(v0.color[0], v1.color[0], v2.color[0]),
                        interp(v0.color[1], v1.color[1], v2.color[1]),
                        interp(v0.color[2], v1.color[2], v2.color[2]),
                        interp(v0.color[3], v1.color[3], v2.color[3]),
                    )
                };

                let tex_col = if let Some(tc) = const_tex_col {
                    tc
                } else {
                    match tex {
                        Some(t) => {
                            let u = v0.uv.x * w0 + v1.uv.x * w1 + v2.uv.x * w2;
                            let v = v0.uv.y * w0 + v1.uv.y * w1 + v2.uv.y * w2;
                            t.sample(u, v)
                        }
                        None => Color32::WHITE,
                    }
                };

                modulate_premultiplied(tex_col, vert_col)
            };
            if src_rgba[3] != 0 {
                let off = (row_offset + x as usize) * 4;
                blend_argb8888_pixel(&mut buffer[off..off + 4], src_rgba);
            }
        }
    }
}

struct Fnv1aHasher(u64);

impl Hasher for Fnv1aHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

fn compute_frame_hash(
    width_px: u32,
    height_px: u32,
    pixels_per_point: f32,
    texture_epoch: u64,
    primitives: &[egui::ClippedPrimitive],
) -> u64 {
    let mut h = Fnv1aHasher(0xcbf29ce484222325);
    width_px.hash(&mut h);
    height_px.hash(&mut h);
    pixels_per_point.to_bits().hash(&mut h);
    texture_epoch.hash(&mut h);
    primitives.len().hash(&mut h);
    for clipped in primitives {
        clipped.clip_rect.min.x.to_bits().hash(&mut h);
        clipped.clip_rect.min.y.to_bits().hash(&mut h);
        clipped.clip_rect.max.x.to_bits().hash(&mut h);
        clipped.clip_rect.max.y.to_bits().hash(&mut h);
        if let Primitive::Mesh(mesh) = &clipped.primitive {
            mesh.texture_id.hash(&mut h);
            mesh.indices.hash(&mut h);
            mesh.vertices.len().hash(&mut h);
            for v in &mesh.vertices {
                v.pos.x.to_bits().hash(&mut h);
                v.pos.y.to_bits().hash(&mut h);
                v.uv.x.to_bits().hash(&mut h);
                v.uv.y.to_bits().hash(&mut h);
                v.color.to_array().hash(&mut h);
            }
        }
    }
    h.finish()
}
