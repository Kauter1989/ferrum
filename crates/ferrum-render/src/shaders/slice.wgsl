// 2D slice view rendered straight from the 3D volume texture: changing the
// slice, orientation or window costs one uniform update and no CPU work.

struct SliceUniforms {
    rect: vec4<f32>,       // image rect min.xy, max.xy in target pixels
    window: vec4<f32>,     // window lo, hi (normalised), slice position, axis id
    options: vec4<f32>,    // x: nearest sampling, y: segment overlay, z: style (0 outline, 1 fill, 2 both), w: fill opacity
    background: vec4<f32>,
};

@group(0) @binding(0) var<uniform> S: SliceUniforms;
@group(0) @binding(1) var volume_tex: texture_3d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var nearest: sampler;
@group(0) @binding(4) var label_tex: texture_3d<u32>;
@group(0) @binding(5) var seg_lut: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

// Mirrors ferrum_domain::SliceAxis::tex_coord.
fn slice_tex(uv: vec2<f32>, s: f32, axis: u32) -> vec3<f32> {
    if (axis == 0u) {
        return vec3<f32>(s, uv.x, 1.0 - uv.y);
    }
    if (axis == 1u) {
        return vec3<f32>(uv.x, s, 1.0 - uv.y);
    }
    return vec3<f32>(uv.x, uv.y, s);
}

// Label of the voxel containing texture coordinate t (nearest).
fn label_at(t: vec3<f32>) -> u32 {
    let dims = vec3<i32>(textureDimensions(label_tex));
    let v = clamp(vec3<i32>(floor(t * vec3<f32>(dims))), vec3<i32>(0), dims - vec3<i32>(1));
    return textureLoad(label_tex, v, 0).r;
}

fn label_at_pixel(frag: vec2<f32>, axis: u32) -> u32 {
    let lo = S.rect.xy;
    let hi = S.rect.zw;
    let uv = clamp((frag - lo) / max(hi - lo, vec2<f32>(1e-6)), vec2<f32>(0.0), vec2<f32>(1.0));
    return label_at(slice_tex(uv, S.window.z, axis));
}

// Segment fill (colour blended with its opacity × the fill opacity) and/or a
// one-pixel outline in full colour where the label changes between
// neighbouring screen pixels. Mirrors ferrum_domain::SegmentStyle::blend.
fn overlay(grey: vec3<f32>, frag: vec2<f32>, t: vec3<f32>, axis: u32) -> vec3<f32> {
    let lbl = label_at(t);
    if (lbl == 0u) {
        return grey;
    }
    let c = textureLoad(seg_lut, vec2<i32>(i32(lbl), 0), 0);
    if (c.a <= 0.0) {
        return grey;
    }
    let edge = label_at_pixel(frag + vec2<f32>(1.0, 0.0), axis) != lbl
        || label_at_pixel(frag - vec2<f32>(1.0, 0.0), axis) != lbl
        || label_at_pixel(frag + vec2<f32>(0.0, 1.0), axis) != lbl
        || label_at_pixel(frag - vec2<f32>(0.0, 1.0), axis) != lbl;
    let style = u32(S.options.z + 0.5);
    if (edge && style != 1u) {
        return c.rgb;
    }
    if (style == 0u) {
        return grey;
    }
    return mix(grey, c.rgb, clamp(c.a * S.options.w, 0.0, 1.0));
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let lo = S.rect.xy;
    let hi = S.rect.zw;
    let uv = (frag.xy - lo) / max(hi - lo, vec2<f32>(1e-6));
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return S.background;
    }
    let axis = u32(S.window.w + 0.5);
    let t = slice_tex(uv, S.window.z, axis);
    let v_lin = textureSampleLevel(volume_tex, lin, t, 0.0).r;
    let v_near = textureSampleLevel(volume_tex, nearest, t, 0.0).r;
    let v = select(v_lin, v_near, S.options.x > 0.5);
    let g = clamp((v - S.window.x) / max(S.window.y - S.window.x, 1e-6), 0.0, 1.0);
    var rgb = vec3<f32>(g);
    if (S.options.y > 0.5) {
        rgb = overlay(rgb, frag.xy, t, axis);
    }
    return vec4<f32>(rgb, 1.0);
}
