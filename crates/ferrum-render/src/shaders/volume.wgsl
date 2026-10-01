// Single-pass GPU ray caster.
//
// One full-screen triangle; every fragment builds its primary ray from the
// inverse view-projection matrix, intersects it analytically with the volume
// box and the clipping half-spaces, then marches through the 3D texture.
// The render mode is a pipeline-overridable constant, so each mode compiles
// to a branch-free specialised shader.
//
// The CPU reference renderer (src/cpu/raycast.rs) implements exactly the
// same model; keep both in sync.

struct Uniforms {
    inv_view_proj: mat4x4<f32>,
    eye_step: vec4<f32>,        // xyz eye (model space), w ray step
    extent_opacity: vec4<f32>,  // xyz model extent, w opacity
    dims_brightness: vec4<f32>, // xyz voxel dims, w brightness
    light_iso: vec4<f32>,       // xyz light travel direction, w iso threshold
    tissue: vec4<f32>,          // low, high, surface threshold, cut-surface opacity
    color_low: vec4<f32>,       // rgb, w: ambient occlusion enabled
    color_high: vec4<f32>,      // rgb, w: empty-space skipping enabled
    surf_lit: vec4<f32>,        // rgb, w: number of clip half-spaces
    surf_shadow: vec4<f32>,     // rgb, w: brick size in voxels
    bricks_jitter: vec4<f32>,   // xyz brick grid dims, w: jitter enabled
    ao_scale: vec4<f32>,        // xyz AO texcoord scale, w: reference step
    viewport: vec4<f32>,        // x, y, width, height in pixels
    segments: vec4<f32>,        // x: segment overlay enabled, y: segment ambient term
    planes: array<vec4<f32>, 8>,
};

@group(0) @binding(0) var<uniform> U: Uniforms;
@group(0) @binding(1) var volume_tex: texture_3d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var mask_tex: texture_3d<f32>;
@group(0) @binding(4) var ao_tex: texture_3d<f32>;
@group(0) @binding(5) var occ_tex: texture_3d<f32>;
@group(0) @binding(6) var tf_tex: texture_2d<f32>;
@group(0) @binding(7) var label_tex: texture_3d<u32>;
@group(0) @binding(8) var seg_lut: texture_2d<f32>;

override MODE: u32 = 0u;
const MODE_TISSUE: u32 = 0u;
const MODE_ISO: u32 = 1u;
const MODE_MIP: u32 = 2u;
const MODE_TF: u32 = 3u;

const MAX_STEPS: u32 = 4096u;
const EARLY_EXIT: f32 = 0.97;
const REFINE_STEPS: u32 = 6u;
const TISSUE_DENSITY: f32 = 175.0;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

fn pcg_hash(x: u32) -> u32 {
    let state = x * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn to_tex(p: vec3<f32>) -> vec3<f32> {
    return p / U.extent_opacity.xyz + vec3<f32>(0.5);
}

fn density(p: vec3<f32>) -> f32 {
    let t = to_tex(p);
    let v = textureSampleLevel(volume_tex, lin, t, 0.0).r;
    let m = textureSampleLevel(mask_tex, lin, t, 0.0).r;
    return v * m;
}

// Gradient of the density field in model space (up to a constant factor).
fn gradient(p: vec3<f32>) -> vec3<f32> {
    let d = U.extent_opacity.xyz / U.dims_brightness.xyz;
    let gx = density(p + vec3<f32>(d.x, 0.0, 0.0)) - density(p - vec3<f32>(d.x, 0.0, 0.0));
    let gy = density(p + vec3<f32>(0.0, d.y, 0.0)) - density(p - vec3<f32>(0.0, d.y, 0.0));
    let gz = density(p + vec3<f32>(0.0, 0.0, d.z)) - density(p - vec3<f32>(0.0, 0.0, d.z));
    return vec3<f32>(gx / d.x, gy / d.y, gz / d.z);
}

// Outward surface normal (towards decreasing density).
fn normal_at(p: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let g = gradient(p);
    let len = length(g);
    if (len < 1e-6) {
        return -dir;
    }
    return -g / len;
}

fn ambient_occlusion(p: vec3<f32>) -> f32 {
    if (U.color_low.w < 0.5) {
        return 1.0;
    }
    return textureSampleLevel(ao_tex, lin, to_tex(p) * U.ao_scale.xyz, 0.0).r;
}

fn shade_surface(p: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let n = normal_at(p, dir);
    let l = -U.light_iso.xyz;
    let diff = max(dot(n, l), 0.0);
    let r = reflect(-l, n);
    let spec = pow(max(dot(r, -dir), 0.0), 40.0);
    let ao = ambient_occlusion(p);
    let base = mix(U.surf_shadow.rgb, U.surf_lit.rgb, diff);
    let b = 0.5 * (U.dims_brightness.w + 1.5);
    return base * b * (0.4 * ao + 0.6 * diff) + vec3<f32>(0.25 * spec * ao);
}

// Bisection between a sample below (a) and above (b) the threshold.
fn refine(eye: vec3<f32>, dir: vec3<f32>, a: f32, b: f32, thr: f32) -> f32 {
    var lo = a;
    var hi = b;
    for (var i = 0u; i < REFINE_STEPS; i = i + 1u) {
        let mid = 0.5 * (lo + hi);
        if (density(eye + dir * mid) > thr) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    return 0.5 * (lo + hi);
}

fn brick_of(p: vec3<f32>) -> vec3<f32> {
    let bs = U.surf_shadow.w;
    let b = floor(to_tex(p) * U.dims_brightness.xyz / bs);
    return clamp(b, vec3<f32>(0.0), U.bricks_jitter.xyz - vec3<f32>(1.0));
}

fn brick_empty(p: vec3<f32>) -> bool {
    let b = vec3<i32>(brick_of(p));
    return textureLoad(occ_tex, b, 0).r < 0.5;
}

// Distance from p along dir to the exit of p's brick.
fn brick_exit(p: vec3<f32>, dir: vec3<f32>) -> f32 {
    let bs = U.surf_shadow.w;
    let dims = U.dims_brightness.xyz;
    let ext = U.extent_opacity.xyz;
    let b = brick_of(p);
    let lo = (b * bs / dims - vec3<f32>(0.5)) * ext;
    let hi = ((b + vec3<f32>(1.0)) * bs / dims - vec3<f32>(0.5)) * ext;
    let safe = select(dir, vec3<f32>(1e-7), abs(dir) < vec3<f32>(1e-7));
    let t1 = (lo - p) / safe;
    let t2 = (hi - p) / safe;
    let tmax = max(t1, t2);
    return max(min(tmax.x, min(tmax.y, tmax.z)), 0.0);
}

// If empty-space skipping is enabled and the brick at parameter t is empty,
// returns the first sampling position after the brick; otherwise t.
fn skip_empty(eye: vec3<f32>, dir: vec3<f32>, t: f32, base: f32, step: f32) -> f32 {
    if (U.color_high.w < 0.5) {
        return t;
    }
    let p = eye + dir * t;
    if (!brick_empty(p)) {
        return t;
    }
    let exit = t + brick_exit(p, dir);
    let k = ceil((exit - base) / step);
    return max(base + k * step, t + step);
}

// ---- segment overlay ------------------------------------------------------
// Segments are drawn as shaded surfaces: when a ray enters a voxel of a
// visible label (different from the previous sample's label) the segment
// colour is composited once with the segment's opacity.

fn label_at(p: vec3<f32>) -> u32 {
    let dims = vec3<i32>(textureDimensions(label_tex));
    let v = clamp(vec3<i32>(floor(to_tex(p) * vec3<f32>(dims))), vec3<i32>(0), dims - vec3<i32>(1));
    return textureLoad(label_tex, v, 0).r;
}

// Label at p, or 0 if it is background or erased by the eraser mask.
fn visible_label(p: vec3<f32>) -> u32 {
    let lbl = label_at(p);
    if (lbl == 0u) {
        return 0u;
    }
    if (textureSampleLevel(mask_tex, lin, to_tex(p), 0.0).r < 0.5) {
        return 0u;
    }
    return lbl;
}

fn seg_indicator(p: vec3<f32>, lbl: u32) -> f32 {
    return select(0.0, 1.0, label_at(p) == lbl);
}

// Outward normal of the segment `lbl` at p (towards leaving the segment).
fn seg_normal(p: vec3<f32>, lbl: u32, dir: vec3<f32>) -> vec3<f32> {
    let d = U.extent_opacity.xyz / vec3<f32>(textureDimensions(label_tex));
    let gx = seg_indicator(p + vec3<f32>(d.x, 0.0, 0.0), lbl) - seg_indicator(p - vec3<f32>(d.x, 0.0, 0.0), lbl);
    let gy = seg_indicator(p + vec3<f32>(0.0, d.y, 0.0), lbl) - seg_indicator(p - vec3<f32>(0.0, d.y, 0.0), lbl);
    let gz = seg_indicator(p + vec3<f32>(0.0, 0.0, d.z), lbl) - seg_indicator(p - vec3<f32>(0.0, 0.0, d.z), lbl);
    let g = vec3<f32>(gx / d.x, gy / d.y, gz / d.z);
    let len = length(g);
    if (len < 1e-6) {
        return -dir;
    }
    return -g / len;
}

// Shaded segment colour (rgb) and opacity (a) to composite at p, or zero.
// `prev` tracks the label of the previous sample along the ray.
fn seg_hit(p: vec3<f32>, dir: vec3<f32>, prev: ptr<function, u32>) -> vec4<f32> {
    if (U.segments.x < 0.5) {
        return vec4<f32>(0.0);
    }
    let lbl = visible_label(p);
    if (lbl == *prev) {
        return vec4<f32>(0.0);
    }
    *prev = lbl;
    if (lbl == 0u) {
        return vec4<f32>(0.0);
    }
    let c = textureLoad(seg_lut, vec2<i32>(i32(lbl), 0), 0);
    if (c.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let n = seg_normal(p, lbl, dir);
    let diff = max(dot(n, -U.light_iso.xyz), 0.0);
    let amb = U.segments.y;
    return vec4<f32>(c.rgb * (amb + (1.0 - amb) * diff), c.a);
}

fn tf_lookup(v: f32) -> vec4<f32> {
    let u = (v * 255.0 + 0.5) / 256.0;
    return textureSampleLevel(tf_tex, lin, vec2<f32>(u, 0.5), 0.0);
}

struct Segment {
    t0: f32,
    t1: f32,
    cut: bool,
    hit: bool,
};

fn clip_ray(eye: vec3<f32>, dir: vec3<f32>) -> Segment {
    var s: Segment;
    s.hit = false;
    s.cut = false;
    let half = 0.5 * U.extent_opacity.xyz;
    let safe = select(dir, vec3<f32>(1e-7), abs(dir) < vec3<f32>(1e-7));
    let a = (-half - eye) / safe;
    let b = (half - eye) / safe;
    let tmin = min(a, b);
    let tmax = max(a, b);
    s.t0 = max(max(tmin.x, max(tmin.y, tmin.z)), 0.0);
    s.t1 = min(tmax.x, min(tmax.y, tmax.z));
    if (s.t0 >= s.t1) {
        return s;
    }
    let count = u32(U.surf_lit.w);
    for (var i = 0u; i < count; i = i + 1u) {
        let h = U.planes[i];
        let denom = dot(h.xyz, dir);
        let dist = h.w - dot(h.xyz, eye);
        if (abs(denom) < 1e-8) {
            if (dist < 0.0) {
                return s;
            }
            continue;
        }
        let t = dist / denom;
        if (denom > 0.0) {
            s.t1 = min(s.t1, t);
        } else if (t > s.t0) {
            s.t0 = t;
            s.cut = true;
        }
    }
    s.hit = s.t0 < s.t1;
    return s;
}

fn march_tissue(eye: vec3<f32>, dir: vec3<f32>, seg: Segment, base: f32) -> vec3<f32> {
    let step = U.eye_step.w;
    let low = U.tissue.x;
    let high = U.tissue.y;
    let surf = U.tissue.z;
    let l = -U.light_iso.xyz;
    var acc = vec3<f32>(0.0);
    var alpha = 0.0;
    var surface = vec3<f32>(0.0);
    var sacc = vec3<f32>(0.0);
    var prev = 0u;
    var t = base;
    for (var i = 0u; i < MAX_STEPS; i = i + 1u) {
        if (t > seg.t1) {
            break;
        }
        let nt = skip_empty(eye, dir, t, base, step);
        if (nt != t) {
            t = nt;
            continue;
        }
        let p = eye + dir * t;
        let sh = seg_hit(p, dir, &prev);
        if (sh.a > 0.0) {
            sacc = sacc + (1.0 - alpha) * sh.a * sh.rgb;
            alpha = alpha + (1.0 - alpha) * sh.a;
            if (alpha > EARLY_EXIT) {
                break;
            }
        }
        let v = density(p);
        if (v > surf) {
            let th = refine(eye, dir, max(t - step, seg.t0), t, surf);
            surface = shade_surface(eye + dir * th, dir);
            break;
        }
        let w = high - low;
        var band = 0.0;
        if (w > 0.0) {
            band = max(min(v - low, high - v) / w, 0.0) * 2.0;
        }
        if (band > 0.0) {
            let a = 1.0 - exp(-TISSUE_DENSITY * U.extent_opacity.w * band * step);
            let n = normal_at(p, dir);
            let lit = 0.5 * max(dot(n, l), 0.0) + 0.5;
            let c = mix(U.color_low.rgb, U.color_high.rgb, clamp((v - low) / max(w, 1e-6), 0.0, 1.0)) * lit;
            acc = acc + (1.0 - alpha) * a * c;
            alpha = alpha + (1.0 - alpha) * a;
            if (alpha > EARLY_EXIT) {
                break;
            }
        }
        t = t + step;
    }
    return acc * (2.0 * U.dims_brightness.w) + sacc + (1.0 - alpha) * surface;
}

fn march_iso(eye: vec3<f32>, dir: vec3<f32>, seg: Segment, base: f32) -> vec3<f32> {
    let step = U.eye_step.w;
    let thr = U.light_iso.w;
    var alpha = 0.0;
    var sacc = vec3<f32>(0.0);
    var prev = 0u;
    var t = base;
    for (var i = 0u; i < MAX_STEPS; i = i + 1u) {
        if (t > seg.t1) {
            break;
        }
        let nt = skip_empty(eye, dir, t, base, step);
        if (nt != t) {
            t = nt;
            continue;
        }
        let p = eye + dir * t;
        let sh = seg_hit(p, dir, &prev);
        if (sh.a > 0.0) {
            sacc = sacc + (1.0 - alpha) * sh.a * sh.rgb;
            alpha = alpha + (1.0 - alpha) * sh.a;
            if (alpha > EARLY_EXIT) {
                break;
            }
        }
        if (density(p) > thr) {
            let th = refine(eye, dir, max(t - step, seg.t0), t, thr);
            return sacc + (1.0 - alpha) * shade_surface(eye + dir * th, dir);
        }
        t = t + step;
    }
    return sacc;
}

fn march_mip(eye: vec3<f32>, dir: vec3<f32>, seg: Segment, base: f32) -> vec3<f32> {
    let step = U.eye_step.w;
    var mx = 0.0;
    var alpha = 0.0;
    var sacc = vec3<f32>(0.0);
    var prev = 0u;
    var t = base;
    for (var i = 0u; i < MAX_STEPS; i = i + 1u) {
        if (t > seg.t1) {
            break;
        }
        let nt = skip_empty(eye, dir, t, base, step);
        if (nt != t) {
            t = nt;
            continue;
        }
        let p = eye + dir * t;
        let sh = seg_hit(p, dir, &prev);
        if (sh.a > 0.0) {
            sacc = sacc + (1.0 - alpha) * sh.a * sh.rgb;
            alpha = alpha + (1.0 - alpha) * sh.a;
            if (alpha > EARLY_EXIT) {
                break;
            }
        }
        mx = max(mx, density(p));
        t = t + step;
    }
    return sacc + (1.0 - alpha) * mx * U.surf_lit.rgb * (2.0 * U.dims_brightness.w);
}

fn march_tf(eye: vec3<f32>, dir: vec3<f32>, seg: Segment, base: f32) -> vec3<f32> {
    let step = U.eye_step.w;
    let l = -U.light_iso.xyz;
    let exponent = step / U.ao_scale.w;
    var acc = vec3<f32>(0.0);
    var alpha = 0.0;
    var sacc = vec3<f32>(0.0);
    var prev = 0u;
    var t = base;
    for (var i = 0u; i < MAX_STEPS; i = i + 1u) {
        if (t > seg.t1) {
            break;
        }
        let nt = skip_empty(eye, dir, t, base, step);
        if (nt != t) {
            t = nt;
            continue;
        }
        let p = eye + dir * t;
        let sh = seg_hit(p, dir, &prev);
        if (sh.a > 0.0) {
            sacc = sacc + (1.0 - alpha) * sh.a * sh.rgb;
            alpha = alpha + (1.0 - alpha) * sh.a;
            if (alpha > EARLY_EXIT) {
                break;
            }
        }
        let c = tf_lookup(density(p));
        let a0 = min(c.a * U.extent_opacity.w * 2.0, 0.999);
        if (a0 > 0.0) {
            let a = 1.0 - pow(1.0 - a0, exponent);
            let n = normal_at(p, dir);
            let lit = 0.5 * max(dot(n, l), 0.0) + 0.5;
            acc = acc + (1.0 - alpha) * a * c.rgb * lit;
            alpha = alpha + (1.0 - alpha) * a;
            if (alpha > EARLY_EXIT) {
                break;
            }
        }
        t = t + step;
    }
    return acc * (2.0 * U.dims_brightness.w) + sacc;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let vp = U.viewport;
    let ndc = vec2<f32>((frag.x - vp.x) / vp.z * 2.0 - 1.0, 1.0 - (frag.y - vp.y) / vp.w * 2.0);
    let far_h = U.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let far = far_h.xyz / far_h.w;
    let eye = U.eye_step.xyz;
    let dir = normalize(far - eye);

    let seg = clip_ray(eye, dir);
    if (!seg.hit) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    var jitter = 0.0;
    if (U.bricks_jitter.w > 0.5) {
        let px = vec2<u32>(u32(frag.x - vp.x), u32(frag.y - vp.y));
        jitter = f32(pcg_hash(px.x + px.y * 7919u)) / 4294967295.0;
    }
    let base = seg.t0 + jitter * U.eye_step.w;

    var rgb: vec3<f32>;
    switch MODE {
        case 1u: { rgb = march_iso(eye, dir, seg, base); }
        case 2u: { rgb = march_mip(eye, dir, seg, base); }
        case 3u: { rgb = march_tf(eye, dir, seg, base); }
        default: { rgb = march_tissue(eye, dir, seg, base); }
    }

    let cut_opacity = U.tissue.w;
    if (seg.cut && cut_opacity > 0.0) {
        let v = density(eye + dir * seg.t0);
        rgb = mix(rgb, vec3<f32>(v), cut_opacity);
    }
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
