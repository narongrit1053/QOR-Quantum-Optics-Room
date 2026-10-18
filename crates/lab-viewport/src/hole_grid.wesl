import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
};

const PI: f32 = 3.14159265;

struct HoleGridParams {
    origin: vec2<f32>,
    pitch: vec2<f32>,
    mask_size: vec2<f32>,
    hole_radius: f32,
    chamfer_radius: f32,
    hole_shade: f32,
    chamfer_shade: f32,
    highlight: f32,
    show_zones: f32,
    accent: vec4<f32>,
    zone_count: u32,
    zones: array<vec4<f32>, 16>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> grid: HoleGridParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var mask_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var mask_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Derivatives first, in uniform control flow.
    let p = in.uv;                                     // platform-local mm
    let dx = dpdx(p);
    let dy = dpdy(p);
    let px = max(max(length(dx), length(dy)), 1e-4);   // mm per pixel, worst axis
    let g = (p - grid.origin) / grid.pitch;            // label lattice coordinates
    let g_fw = max(length(dx / grid.pitch), length(dy / grid.pitch));

    // Near field: analytic, box-filtered coverage of the nearest lattice hole.
    let cell = floor(g + vec2<f32>(0.5));
    let d = length((g - cell) * grid.pitch);
    let size = vec2<i32>(grid.mask_size);
    let texel = vec2<i32>(cell) + vec2<i32>(1);
    let inside = all(texel >= vec2<i32>(0)) && all(texel < size);
    let present = select(0.0, textureLoad(mask_texture, clamp(texel, vec2<i32>(0), size - vec2<i32>(1)), 0).r, inside);
    // A ramp one pixel wide centred on radius r covers pi*(r^2 + px^2/12) in total. Shrinking
    // the edge radius by that excess keeps every disc's integral exact (no brightness pop
    // where the near field takes over).
    let edge2 = px * px / 12.0;
    let r_hole = sqrt(max(grid.hole_radius * grid.hole_radius - edge2, 0.0));
    let r_ring = sqrt(max(grid.chamfer_radius * grid.chamfer_radius - edge2, 0.0));
    let hole_cov = clamp((r_hole - d) / px + 0.5, 0.0, 1.0);
    let ring_cov = max(clamp((r_ring - d) / px + 0.5, 0.0, 1.0) - hole_cov, 0.0);
    let near = present * (hole_cov * grid.hole_shade + ring_cov * grid.chamfer_shade);
    let near_hole = present * hole_cov;

    // Far field: area-weighted mean × presence averaged over the pixel footprint (mip chain).
    let cell_area = grid.pitch.x * grid.pitch.y;
    let a_hole = PI * grid.hole_radius * grid.hole_radius / cell_area;
    let a_ring = PI * (grid.chamfer_radius * grid.chamfer_radius - grid.hole_radius * grid.hole_radius) / cell_area;
    let uv = (g + vec2<f32>(1.5)) / grid.mask_size;
    let lod = max(log2(max(g_fw, 1e-6)), 0.0);
    let mean = textureSampleLevel(mask_texture, mask_sampler, uv, lod).r;
    let far = mean * (a_hole * grid.hole_shade + a_ring * grid.chamfer_shade);
    let far_hole = mean * a_hole;

    // Near field once a hole radius spans >= 1.6 px, far field below 0.6 px.
    let t = smoothstep(0.6, 1.6, grid.hole_radius / px);
    let shade = mix(far, near, t);
    let hole_amount = mix(far_hole, near_hole, t);

    // Exclusion zones: tint + 1 px outline, only while the platform is selected.
    var tint = 0.0;
    var line = 0.0;
    if (grid.show_zones > 0.0) {
        for (var k = 0u; k < min(grid.zone_count, 16u); k++) {
            let z = grid.zones[k];
            var sd: f32;
            if (z.w < 0.0) {
                sd = length(p - z.xy) - z.z;
            } else {
                let q = abs(p - z.xy) - z.zw;
                sd = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
            }
            tint = max(tint, 1.0 - smoothstep(-0.5 * px, 0.5 * px, sd));
            line = max(line, 1.0 - smoothstep(0.5 * px, 1.5 * px, abs(sd)));
        }
    }

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var base = pbr_input.material.base_color.rgb * (1.0 - shade);
    base = mix(base, grid.accent.rgb, 0.15 * tint);
    base = mix(base, grid.accent.rgb, 0.85 * line);
    base = mix(base, grid.accent.rgb, 0.18 * grid.highlight);
    pbr_input.material.base_color = vec4<f32>(base, pbr_input.material.base_color.a);
    pbr_input.material.metallic = mix(pbr_input.material.metallic, 0.0, hole_amount);
    pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, 1.0, hole_amount);
    pbr_input.material.base_color = alpha_discard(pbr_input.material.flags, pbr_input.material.alpha_cutoff, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
