// Pixel shader for the live wave. Track timing, palette and spark table are shared with the CPU path.
struct Layer {
    edge: vec4f,     // base radius, spark strength, reaction, scale
    motion: vec4f,   // noise offset, phase sine, glow strength, like
    color: vec4f,    // base color, horizontal shift
    gradient: vec4f, // color difference, vertical shift
}

struct Frame {
    crop: vec4u,     // x, y, width, height in canvas pixels
    info: vec4f,    // canvas side, time, opacity, number of layers
    layers: array<Layer, 3>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> sparks: array<f32>;
@group(0) @binding(2) var<storage, read_write> pixels: array<u32>;

fn permute(x: f32) -> f32 {
    let a = (x * 34.0 + 1.0) * x;
    return a - 289.0 * floor(a / 289.0);
}

// Ashima simplex noise, same as snoise3 in the CPU renderer.
fn noise(v: vec3f) -> f32 {
    let c = vec2f(0.1666667, 0.3333333);
    let cell = floor(v + dot(v, vec3f(c.y)));
    let x0 = v - cell + dot(cell, vec3f(c.x));
    let g = step(x0.yzx, x0.xyz);
    let l = vec3f(1.0) - g;
    let i1 = min(g, l.zxy);
    let i2 = max(g, l.zxy);
    let corners = array(x0, x0 - i1 + c.x, x0 - i2 + 2.0 * c.x, x0 - 1.0 + 3.0 * c.x);
    let offsets = array(vec3f(0.0), i1, i2, vec3f(1.0));
    let i = cell - 289.0 * floor(cell / 289.0);
    let n = 0.14285715;
    var sum = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let p = permute(permute(permute(i.z + offsets[k].z) + i.y + offsets[k].y) + i.x + offsets[k].x);
        let j = p - 49.0 * floor(p * n * n);
        let a = floor(j * n);
        let b = floor(j - 7.0 * a);
        let xy = vec2f(a, b) * (2.0 * n) + (0.5 * n - 1.0);
        let h = 1.0 - abs(xy.x) - abs(xy.y);
        let sh = select(-1.0, 0.0, h > 0.0);
        let grad = vec3f(xy + (floor(xy) * 2.0 + 1.0) * sh, h);
        let norm = 1.7928429 - 0.85373473 * dot(grad, grad);
        let q = corners[k];
        let m = max(0.6 - dot(q, q), 0.0);
        sum += m * m * m * m * dot(grad * norm, q);
    }
    return 42.0 * sum;
}

fn ramp(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn shade(uv: vec2f) -> u32 {
    let len = length(uv);
    var angle = 1024.0;
    if len > 0.0 {
        angle = clamp(atan2(uv.y, uv.x) / 6.28318530718 + 0.5, 0.0, 1.0) * 2048.0;
    }
    let k = min(u32(angle), 2047u);
    let spark = mix(sparks[k], sparks[k + 1u], angle - f32(k));
    let circles = u32(frame.info.w);
    var edge = array<f32, 3>(0.0, 0.0, 0.0);
    var reach = 0.0;
    for (var j = 0u; j < circles; j++) {
        let p = frame.layers[j].edge;
        let base = p.y * spark;
        edge[j] = p.x + base * (1.0 + p.z * base);
        reach = max(reach, edge[j]);
    }
    if len >= reach + 0.3 { return 0u; }
    let time = frame.info.y;
    let shared_noise = noise(vec3f(uv * 1.2, time * 0.5));
    var rgb = vec3f(0.0);
    var alpha = 0.0;
    for (var j = 0u; j < circles; j++) {
        let layer = frame.layers[j];
        let outer = edge[j] + 0.3 * shared_noise;
        if len >= outer { continue; }
        let local = uv * layer.edge.w + vec2f(layer.color.w, layer.gradient.w);
        let ll = length(local);
        let r = noise(vec3f(local * 1.2, time * 0.5) + layer.motion.x) * 0.5 + 0.5;
        let a = ramp(r + 0.1 + layer.motion.y + 1.0, r, ll) * ramp(outer, 0.5, len);
        let glow = layer.motion.z / (1.0 + 11.0 * abs(ll - r));
        let flash = 0.6 * layer.motion.w * (1.0 - ramp(0.2, outer * 0.8, len));
        let color = clamp(layer.color.xyz + layer.gradient.xyz * (local.y * 2.0) + glow, vec3f(0.0), vec3f(1.0)) + flash;
        rgb = mix(rgb, color, a);
        alpha = max(alpha, a);
    }
    let a = min(alpha, 1.0) * frame.info.z;
    if a * 255.0 + 0.5 < 3.0 { return 0u; }
    return pack4x8unorm(vec4f(clamp(rgb, vec3f(0.0), vec3f(1.0)) * a, a));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3u) {
    if id.x >= frame.crop.z || id.y >= frame.crop.w { return; }
    let p = vec2f(frame.crop.xy + id.xy) + 0.5;
    let uv = (p * (2.0 / frame.info.x) - 1.0) * vec2f(1.0, -1.0) / 0.35;
    pixels[id.y * frame.crop.z + id.x] = shade(uv);
}
