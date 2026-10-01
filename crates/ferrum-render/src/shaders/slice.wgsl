// 2D slice view rendered straight from the 3D volume texture: changing the
// slice, orientation or window costs one uniform update and no CPU work.

struct SliceUniforms {
    rect: vec4<f32>,       // image rect min.xy, max.xy in target pixels
    window: vec4<f32>,     // window lo, hi (normalised), slice position, axis id
    options: vec4<f32>,    // x: nearest sampling
    background: vec4<f32>,
};

@group(0) @binding(0) var<uniform> S: SliceUniforms;
@group(0) @binding(1) var volume_tex: texture_3d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var nearest: sampler;

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

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let lo = S.rect.xy;
    let hi = S.rect.zw;
    let uv = (frag.xy - lo) / max(hi - lo, vec2<f32>(1e-6));
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return S.background;
    }
    let t = slice_tex(uv, S.window.z, u32(S.window.w + 0.5));
    let v_lin = textureSampleLevel(volume_tex, lin, t, 0.0).r;
    let v_near = textureSampleLevel(volume_tex, nearest, t, 0.0).r;
    let v = select(v_lin, v_near, S.options.x > 0.5);
    let g = clamp((v - S.window.x) / max(S.window.y - S.window.x, 1e-6), 0.0, 1.0);
    return vec4<f32>(g, g, g, 1.0);
}
