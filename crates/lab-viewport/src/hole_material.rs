//! Hole rendering for platform tops (M1 spec §5.5).
//!
//! The top of a platform is one mesh whose `UV_0` is platform-local millimetres. A
//! [`HoleTopMaterial`] (the standard PBR material extended by `hole_grid.wesl`) draws the
//! tapped holes analytically:
//!
//! - **Near field.** The nearest lattice hole is drawn as a box-filtered disc (one pixel wide,
//!   from the derivatives of the mm-space UVs) with a lighter chamfer ring, so holes stay crisp
//!   at any zoom. Whether that hole exists comes from a presence mask (`textureLoad`).
//! - **Far field.** Once a hole is smaller than about a pixel, the shader switches to the
//!   area-weighted mean shade times the mean presence over the pixel footprint (the mask's mip
//!   chain), so distant grids neither shimmer nor change brightness.
//!
//! The mask is one texel per label cell with a zero border; holes outside the outline, inside
//! the margin or inside exclusion zones are simply absent from it. [`shade_cpu`] mirrors the
//! shader so the energy and no-pop properties are unit-tested without a GPU.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin},
    prelude::*,
    render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat},
    shader::ShaderRef,
};

/// Exclusion zones drawn by the shader (tint and outline while the platform is selected).
pub(crate) const MAX_ZONES: usize = 16;

pub(crate) const HOLE_GRID_SHADER: Handle<Shader> = bevy::asset::uuid_handle!("6c1f0a4e-3b9d-4f7e-9a51-2d8c7e4b1f03");

/// Shade of the hole floor and of the chamfer ring, as a fraction of the base colour removed.
const HOLE_SHADE: f32 = 0.92;
const CHAMFER_SHADE: f32 = 0.30;
/// Chamfer ring radius relative to the hole radius (clamped to half the pitch).
const CHAMFER_FACTOR: f32 = 1.35;

/// Uniform block of `hole_grid.wesl`. The field order must match the WESL struct.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub(crate) struct HoleGridParams {
    /// Platform-local mm of the label (0,0) hole.
    pub origin: Vec2,
    pub pitch: Vec2,
    /// Padded mask texture size in texels.
    pub mask_size: Vec2,
    /// 0 = no holes.
    pub hole_radius: f32,
    /// `min(1.35 * hole_radius, 0.5 * min(pitch))`.
    pub chamfer_radius: f32,
    pub hole_shade: f32,
    pub chamfer_shade: f32,
    /// 0 or 1 (selected).
    pub highlight: f32,
    /// 0 or 1 (selected).
    pub show_zones: f32,
    /// Linear #0A84FF.
    pub accent: Vec4,
    pub zone_count: u32,
    /// Rect: (cx, cy, half_w, half_h); circle: (cx, cy, r, -1).
    pub zones: [Vec4; MAX_ZONES],
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub(crate) struct HoleGridExt {
    #[uniform(100)]
    pub params: HoleGridParams,
    #[texture(101)]
    #[sampler(102)]
    pub mask: Handle<Image>,
}

impl MaterialExtension for HoleGridExt {
    fn fragment_shader() -> ShaderRef {
        HOLE_GRID_SHADER.into()
    }
}

pub(crate) type HoleTopMaterial = ExtendedMaterial<StandardMaterial, HoleGridExt>;

/// Registers the hole shader (synchronously, so the first frame after a spawn can use it) and
/// the [`HoleTopMaterial`] pipeline. Add after `DefaultPlugins`.
pub(crate) struct HoleGridPlugin;

impl Plugin for HoleGridPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(
                HOLE_GRID_SHADER.id(),
                Shader::from_wesl(include_str!("hole_grid.wesl"), "embedded://lab_viewport/hole_grid.wesl"),
            )
            .expect("INVARIANT: uuid asset ids never fail to insert");
        app.add_plugins(MaterialPlugin::<HoleTopMaterial>::default());
    }
}

/// Presence mask with its mip chain; `levels[0]` is the full size.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HoleMask {
    pub size: (u32, u32),
    pub levels: Vec<Vec<u8>>,
}

impl HoleMask {
    /// Size of mip `level` (an axis already at 1 stays 1).
    #[cfg_attr(not(test), allow(dead_code))]
    fn level_size(&self, level: usize) -> (u32, u32) {
        ((self.size.0 >> level).max(1), (self.size.1 >> level).max(1))
    }
}

/// Texel (k,l) = label cell (k-1, l-1); 255 present, 0 absent; one-texel zero border; padded to
/// next_power_of_two(cols+2) × next_power_of_two(rows+2);
/// mips: 2×2 box average down to 1×1 (an axis already at 1 stays 1). cols == 0 || rows == 0 → 1×1 [0].
pub(crate) fn build_mask(cols: u32, rows: u32, present: impl Fn(u32, u32) -> bool) -> HoleMask {
    if cols == 0 || rows == 0 {
        return HoleMask { size: (1, 1), levels: vec![vec![0]] };
    }
    let w = (cols + 2).next_power_of_two();
    let h = (rows + 2).next_power_of_two();

    // Exact (unquantized) averages per level, so every level is within half a step of the true
    // mean presence of its footprint.
    let mut exact = vec![0.0f32; (w * h) as usize];
    for row in 0..rows {
        for col in 0..cols {
            if present(col, row) {
                exact[((row + 1) * w + col + 1) as usize] = 1.0;
            }
        }
    }

    let quantize = |level: &[f32]| level.iter().map(|&v| (v * 255.0).round() as u8).collect::<Vec<u8>>();
    let mut levels = vec![quantize(&exact)];
    let (mut lw, mut lh) = (w, h);
    while lw > 1 || lh > 1 {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let (sx, sy) = (lw / nw, lh / nh); // 2 or 1 per axis
        let mut next = vec![0.0f32; (nw * nh) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = 0.0;
                for dy in 0..sy {
                    for dx in 0..sx {
                        sum += exact[((y * sy + dy) * lw + x * sx + dx) as usize];
                    }
                }
                next[(y * nw + x) as usize] = sum / (sx * sy) as f32;
            }
        }
        levels.push(quantize(&next));
        exact = next;
        (lw, lh) = (nw, nh);
    }
    HoleMask { size: (w, h), levels }
}

/// R8Unorm, all mips concatenated in `data`, RenderAssetUsages::RENDER_WORLD,
/// sampler linear min/mag/mip, ClampToEdge.
pub(crate) fn mask_image(mask: &HoleMask) -> Image {
    let (w, h) = mask.size;
    let mut image = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        mask.levels[0].clone(),
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = mask.levels.len() as u32;
    image.data = Some(mask.levels.concat());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// Lattice geometry in platform-local mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridGeometry {
    /// Position of the label (0,0) hole.
    pub origin: Vec2,
    pub pitch: Vec2,
    pub hole_radius: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ZoneParam {
    Rect { center: Vec2, half: Vec2 },
    Circle { center: Vec2, radius: f32 },
}

/// Shader parameters for a platform top. `highlight` and `show_zones` start at 0; the
/// highlight system sets them while the platform is selected.
pub(crate) fn hole_params(grid: Option<&GridGeometry>, mask: &HoleMask, zones: &[ZoneParam]) -> HoleGridParams {
    let mut params = HoleGridParams {
        pitch: Vec2::ONE,
        mask_size: Vec2::new(mask.size.0 as f32, mask.size.1 as f32),
        hole_shade: HOLE_SHADE,
        chamfer_shade: CHAMFER_SHADE,
        accent: LinearRgba::from(Color::srgb_u8(0x0A, 0x84, 0xFF)).to_vec4(),
        ..default()
    };
    let has_holes = mask.levels[0].iter().any(|&v| v > 0);
    if let Some(grid) = grid
        && grid.pitch.min_element() > 0.0
    {
        params.origin = grid.origin;
        params.pitch = grid.pitch;
        if has_holes && grid.hole_radius > 0.0 {
            params.hole_radius = grid.hole_radius;
            params.chamfer_radius = (CHAMFER_FACTOR * grid.hole_radius).min(0.5 * grid.pitch.min_element());
        }
    }
    for (slot, zone) in params.zones.iter_mut().zip(zones) {
        *slot = match *zone {
            ZoneParam::Rect { center, half } => Vec4::new(center.x, center.y, half.x, half.y),
            ZoneParam::Circle { center, radius } => Vec4::new(center.x, center.y, radius, -1.0),
        };
    }
    params.zone_count = zones.len().min(MAX_ZONES) as u32;
    params
}

// The CPU mirror below exists for the tests; the app itself only uses the shader.
#[cfg_attr(not(test), allow(dead_code))]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear sample of one mip level with ClampToEdge addressing (texel centres at +0.5).
#[cfg_attr(not(test), allow(dead_code))]
fn sample_level(mask: &HoleMask, level: usize, uv: Vec2) -> f32 {
    let (w, h) = mask.level_size(level);
    let data = &mask.levels[level];
    let x = uv.x * w as f32 - 0.5;
    let y = uv.y * h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let texel = |tx: f32, ty: f32| {
        let tx = (tx as i64).clamp(0, w as i64 - 1) as usize;
        let ty = (ty as i64).clamp(0, h as i64 - 1) as usize;
        data[ty * w as usize + tx] as f32 / 255.0
    };
    let top = texel(x0, y0) * (1.0 - fx) + texel(x0 + 1.0, y0) * fx;
    let bottom = texel(x0, y0 + 1.0) * (1.0 - fx) + texel(x0 + 1.0, y0 + 1.0) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// `textureSampleLevel` with a linear mip filter.
#[cfg_attr(not(test), allow(dead_code))]
fn sample_trilinear(mask: &HoleMask, uv: Vec2, lod: f32) -> f32 {
    let max_level = (mask.levels.len() - 1) as f32;
    let lod = lod.clamp(0.0, max_level);
    let l0 = lod.floor();
    let l1 = (l0 + 1.0).min(max_level);
    let f = lod - l0;
    let a = sample_level(mask, l0 as usize, uv);
    if f == 0.0 { a } else { a * (1.0 - f) + sample_level(mask, l1 as usize, uv) * f }
}

/// Rust mirror of the shader's `shade` output (mips sampled bilinearly with linear interpolation between levels).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn shade_cpu(params: &HoleGridParams, mask: &HoleMask, p: Vec2, dpdx: Vec2, dpdy: Vec2) -> f32 {
    let px = dpdx.length().max(dpdy.length()).max(1e-4);
    let g = (p - params.origin) / params.pitch;
    let g_fw = (dpdx / params.pitch).length().max((dpdy / params.pitch).length());

    // Near field.
    let cell = (g + Vec2::splat(0.5)).floor();
    let d = ((g - cell) * params.pitch).length();
    let size = (params.mask_size.x as i32, params.mask_size.y as i32);
    let texel = (cell.x as i32 + 1, cell.y as i32 + 1);
    let inside = texel.0 >= 0 && texel.1 >= 0 && texel.0 < size.0 && texel.1 < size.1;
    let present = if inside {
        let (w, h) = mask.size;
        let tx = texel.0.clamp(0, w as i32 - 1) as usize;
        let ty = texel.1.clamp(0, h as i32 - 1) as usize;
        mask.levels[0][ty * w as usize + tx] as f32 / 255.0
    } else {
        0.0
    };
    let edge2 = px * px / 12.0;
    let r_hole = (params.hole_radius * params.hole_radius - edge2).max(0.0).sqrt();
    let r_ring = (params.chamfer_radius * params.chamfer_radius - edge2).max(0.0).sqrt();
    let hole_cov = ((r_hole - d) / px + 0.5).clamp(0.0, 1.0);
    let ring_cov = (((r_ring - d) / px + 0.5).clamp(0.0, 1.0) - hole_cov).max(0.0);
    let near = present * (hole_cov * params.hole_shade + ring_cov * params.chamfer_shade);

    // Far field.
    let cell_area = params.pitch.x * params.pitch.y;
    let r2 = params.hole_radius * params.hole_radius;
    let a_hole = std::f32::consts::PI * r2 / cell_area;
    let a_ring = std::f32::consts::PI * (params.chamfer_radius * params.chamfer_radius - r2) / cell_area;
    let uv = (g + Vec2::splat(1.5)) / params.mask_size;
    let lod = g_fw.max(1e-6).log2().max(0.0);
    let mean = sample_trilinear(mask, uv, lod);
    let far = mean * (a_hole * params.hole_shade + a_ring * params.chamfer_shade);

    let t = smoothstep(0.6, 1.6, params.hole_radius / px);
    far + (near - far) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_grid(n: u32) -> (HoleMask, HoleGridParams) {
        let mask = build_mask(n, n, |_, _| true);
        let grid = GridGeometry { origin: Vec2::ZERO, pitch: Vec2::splat(25.0), hole_radius: 3.0 };
        let params = hole_params(Some(&grid), &mask, &[]);
        (mask, params)
    }

    /// Mean shade over one lattice cell (centred on hole `cell`), `n`×`n` samples.
    fn cell_average(params: &HoleGridParams, mask: &HoleMask, cell: Vec2, px: f32, n: u32) -> f32 {
        let corner = params.origin + (cell - Vec2::splat(0.5)) * params.pitch;
        let mut sum = 0.0f64;
        for j in 0..n {
            for i in 0..n {
                let f = Vec2::new((i as f32 + 0.5) / n as f32, (j as f32 + 0.5) / n as f32);
                let p = corner + f * params.pitch;
                sum += shade_cpu(params, mask, p, Vec2::new(px, 0.0), Vec2::new(0.0, px)) as f64;
            }
        }
        (sum / (n * n) as f64) as f32
    }

    fn far_value(params: &HoleGridParams) -> f32 {
        let area = params.pitch.x * params.pitch.y;
        let r2 = params.hole_radius * params.hole_radius;
        let a_hole = std::f32::consts::PI * r2 / area;
        let a_ring = std::f32::consts::PI * (params.chamfer_radius * params.chamfer_radius - r2) / area;
        a_hole * params.hole_shade + a_ring * params.chamfer_shade
    }

    #[test]
    fn mask_has_zero_border_and_power_of_two_size() {
        // An L: a 7×5 board with the top-right 3×2 corner missing.
        let (cols, rows) = (7, 5);
        let present = |c: u32, r: u32| !(c >= 4 && r >= 3);
        let mask = build_mask(cols, rows, present);
        assert_eq!(mask.size, (16, 8));
        assert!(mask.size.0.is_power_of_two() && mask.size.1.is_power_of_two());
        let (w, h) = mask.size;
        let level0 = &mask.levels[0];
        assert_eq!(level0.len(), (w * h) as usize);
        for x in 0..w {
            assert_eq!(level0[x as usize], 0, "top border");
        }
        for y in 0..h {
            assert_eq!(level0[(y * w) as usize], 0, "left border");
        }
        for y in 0..h {
            for x in 0..w {
                let v = level0[(y * w + x) as usize];
                let expected = x >= 1 && y >= 1 && x <= cols && y <= rows && present(x - 1, y - 1);
                assert_eq!(v, if expected { 255 } else { 0 }, "texel ({x},{y})");
            }
        }
        // cols + 2 exactly a power of two: the right/bottom border is still zero.
        let tight = build_mask(6, 2, |_, _| true);
        assert_eq!(tight.size, (8, 4));
        let (w, h) = tight.size;
        for y in 0..h {
            assert_eq!(tight.levels[0][(y * w + w - 1) as usize], 0, "right border");
        }
        for x in 0..w {
            assert_eq!(tight.levels[0][((h - 1) * w + x) as usize], 0, "bottom border");
        }
    }

    #[test]
    fn mask_mip_chain_is_complete_and_energy_preserving() {
        for &(cols, rows) in &[(7u32, 5u32), (20, 20), (48, 36), (1, 1), (1, 9), (30, 2), (143, 59)] {
            let present = |c: u32, r: u32| (c * 7 + r * 3) % 5 != 0 && !(c > cols / 2 && r > rows / 2);
            let mask = build_mask(cols, rows, present);
            let (w, h) = mask.size;
            let max = w.max(h);
            assert_eq!(mask.levels.len() as u32, max.ilog2() + 1, "{cols}x{rows}");
            for (l, level) in mask.levels.iter().enumerate() {
                let (lw, lh) = mask.level_size(l);
                assert_eq!(level.len(), (lw * lh) as usize);
            }
            assert_eq!(mask.levels.last().unwrap().len(), 1);
            let count = (0..rows).flat_map(|r| (0..cols).map(move |c| (c, r))).filter(|&(c, r)| present(c, r)).count();
            let expected = count as f32 / (w * h) as f32;
            let last = mask.levels.last().unwrap()[0] as f32 / 255.0;
            let tolerance = mask.levels.len() as f32 / 255.0;
            assert!((last - expected).abs() <= tolerance, "{cols}x{rows}: {last} vs {expected}");
        }
    }

    #[test]
    fn empty_mask_is_one_zero_texel() {
        assert_eq!(build_mask(0, 5, |_, _| true), HoleMask { size: (1, 1), levels: vec![vec![0]] });
        assert_eq!(build_mask(5, 0, |_, _| true), HoleMask { size: (1, 1), levels: vec![vec![0]] });
    }

    #[test]
    fn mask_image_holds_every_mip() {
        let mask = build_mask(20, 20, |_, _| true);
        let image = mask_image(&mask);
        assert_eq!(image.texture_descriptor.format, TextureFormat::R8Unorm);
        assert_eq!(image.texture_descriptor.mip_level_count, 6);
        let texels: usize = (0..6).map(|l| (32usize >> l).pow(2)).sum();
        assert_eq!(image.data.as_ref().unwrap().len(), texels);
    }

    #[test]
    fn near_field_integrates_to_far_value() {
        let (mask, params) = full_grid(20);
        let far = far_value(&params);
        // The far field itself returns that value in the interior.
        let centre = params.origin + Vec2::splat(10.0) * params.pitch;
        let far_shader = shade_cpu(&params, &mask, centre + Vec2::new(3.0, 7.0), Vec2::new(100.0, 0.0), Vec2::new(0.0, 100.0));
        assert!((far_shader - far).abs() / far < 1e-3, "{far_shader} vs {far}");
        for cell in [Vec2::new(10.0, 10.0), Vec2::new(3.0, 15.0), Vec2::new(17.0, 4.0)] {
            let near = cell_average(&params, &mask, cell, 0.01, 64);
            assert!((near - far).abs() / far < 0.01, "cell {cell}: near {near} vs far {far}");
        }
    }

    #[test]
    fn no_brightness_pop_across_zoom() {
        let (mask, params) = full_grid(20);
        let far = far_value(&params);
        let cell = Vec2::new(10.0, 10.0);
        let mut px = 0.05f32;
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        while px <= 20.0 {
            let avg = cell_average(&params, &mask, cell, px, 128);
            lo = lo.min(avg);
            hi = hi.max(avg);
            px *= 1.15;
        }
        let avg = cell_average(&params, &mask, cell, 20.0, 128);
        lo = lo.min(avg);
        hi = hi.max(avg);
        eprintln!("no-pop: cell-averaged shade {lo}..{hi}, far {far}, range {:.2}%", (hi - lo) / far * 100.0);
        assert!((hi - lo) / far < 0.03, "cell-averaged shade ranges {lo}..{hi} (far {far})");
    }

    #[test]
    fn absent_holes_are_not_drawn() {
        // L-shaped board: the top-right corner has no holes.
        let mask = build_mask(8, 8, |c, r| !(c >= 4 && r >= 4));
        let grid = GridGeometry { origin: Vec2::ZERO, pitch: Vec2::splat(25.0), hole_radius: 3.0 };
        let params = hole_params(Some(&grid), &mask, &[]);
        let at = |c: f32, r: f32| shade_cpu(&params, &mask, Vec2::new(c, r) * 25.0, Vec2::new(0.05, 0.0), Vec2::new(0.0, 0.05));
        assert!((at(1.0, 1.0) - HOLE_SHADE).abs() < 1e-6);
        assert_eq!(at(6.0, 6.0), 0.0);
        // Outside the label range (left of column 0) there are no holes either.
        assert_eq!(at(-1.0, 1.0), 0.0);
        assert_eq!(at(1.0, 8.0), 0.0);
        // Between holes the surface is bare.
        assert_eq!(at(1.5, 1.5), 0.0);
    }

    #[test]
    fn hole_params_packs_zones() {
        let mask = build_mask(4, 4, |_, _| true);
        let grid = GridGeometry { origin: Vec2::new(-37.5, -12.5), pitch: Vec2::new(25.0, 20.0), hole_radius: 3.0 };
        let zones = [
            ZoneParam::Rect { center: Vec2::new(10.0, 20.0), half: Vec2::new(30.0, 40.0) },
            ZoneParam::Circle { center: Vec2::new(-5.0, 6.0), radius: 12.0 },
        ];
        let params = hole_params(Some(&grid), &mask, &zones);
        assert_eq!(params.zone_count, 2);
        assert_eq!(params.zones[0], Vec4::new(10.0, 20.0, 30.0, 40.0));
        assert_eq!(params.zones[1], Vec4::new(-5.0, 6.0, 12.0, -1.0));
        assert_eq!(params.zones[2], Vec4::ZERO);
        assert_eq!(params.origin, grid.origin);
        assert_eq!(params.pitch, grid.pitch);
        assert_eq!(params.mask_size, Vec2::new(8.0, 8.0));
        assert_eq!(params.hole_radius, 3.0);
        assert!((params.chamfer_radius - 4.05).abs() < 1e-6);
        assert_eq!((params.hole_shade, params.chamfer_shade), (0.92, 0.30));
        assert_eq!((params.highlight, params.show_zones), (0.0, 0.0));
        let accent = Color::srgb_u8(0x0A, 0x84, 0xFF).to_linear();
        assert_eq!(params.accent, Vec4::new(accent.red, accent.green, accent.blue, 1.0));

        let many: Vec<ZoneParam> = (0..20).map(|i| ZoneParam::Circle { center: Vec2::splat(i as f32), radius: 1.0 }).collect();
        let params = hole_params(Some(&grid), &mask, &many);
        assert_eq!(params.zone_count, MAX_ZONES as u32);
        assert_eq!(params.zones[MAX_ZONES - 1], Vec4::new(15.0, 15.0, 1.0, -1.0));
    }

    #[test]
    fn hole_params_clamps_chamfer_to_half_pitch() {
        let mask = build_mask(4, 4, |_, _| true);
        let grid = GridGeometry { origin: Vec2::ZERO, pitch: Vec2::new(12.0, 5.0), hole_radius: 2.0 };
        let params = hole_params(Some(&grid), &mask, &[]);
        assert_eq!(params.chamfer_radius, 2.5);
        let grid = GridGeometry { pitch: Vec2::new(25.4, 25.4), hole_radius: 3.175, ..grid };
        let params = hole_params(Some(&grid), &mask, &[]);
        assert!((params.chamfer_radius - 1.35 * 3.175).abs() < 1e-5);
    }

    #[test]
    fn hole_params_without_holes_has_zero_radius() {
        let empty = build_mask(0, 0, |_, _| true);
        let params = hole_params(None, &empty, &[]);
        assert_eq!(params.hole_radius, 0.0);
        assert_eq!(params.chamfer_radius, 0.0);
        assert!(params.pitch.min_element() > 0.0);
        assert_eq!(params.mask_size, Vec2::ONE);

        // A grid whose holes were all removed (margin or zones) draws nothing either.
        let grid = GridGeometry { origin: Vec2::ZERO, pitch: Vec2::splat(25.0), hole_radius: 3.0 };
        let none_present = build_mask(5, 5, |_, _| false);
        assert_eq!(hole_params(Some(&grid), &none_present, &[]).hole_radius, 0.0);
        assert_eq!(hole_params(Some(&grid), &empty, &[]).hole_radius, 0.0);

        // The shader output is zero everywhere.
        let params = hole_params(None, &empty, &[]);
        for p in [Vec2::ZERO, Vec2::new(0.4, 0.2), Vec2::new(-3.0, 9.0)] {
            for px in [0.01, 1.0, 50.0] {
                assert_eq!(shade_cpu(&params, &empty, p, Vec2::new(px, 0.0), Vec2::new(0.0, px)), 0.0);
            }
        }
    }
}
