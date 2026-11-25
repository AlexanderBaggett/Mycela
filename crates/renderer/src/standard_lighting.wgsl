// Recovered StandardLightingVertexShader/FragmentShader ordinary primitive path.
// Single-part source ColorMap is sampled with linear repeat. Other shader
// features remain explicit unsupported capabilities.
struct Frame {
    view: mat4x4<f32>,
    view_projection: mat4x4<f32>,
    normal: mat3x3<f32>,
    ambient: vec4<f32>,
    diffuse: vec4<f32>,
    specular: vec4<f32>,
    material: vec4<f32>, // shininess, transparency, source RGB quantization
    counts: vec4<u32>, // processed lights, previous depth, material texture, UV Light shader
}
struct Light {
    position: vec4<f32>,
    ambient: vec4<f32>,
    diffuse: vec4<f32>,
    specular: vec4<f32>,
    spot_direction_cutoff: vec4<f32>,
    attenuation_exponent: vec4<f32>,
    spot_cosine: vec4<f32>,
}
@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var previous_depth: texture_depth_2d;
@group(0) @binding(2) var<storage, read> lights: array<Light>;
@group(0) @binding(3) var color_map: texture_2d<f32>;
@group(0) @binding(4) var color_sampler: sampler;
struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec3<f32>,
    @location(3) uv: vec2<f32>,
}
struct Surface {
    @builtin(position) position: vec4<f32>,
    @location(0) eye_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec3<f32>,
    @location(3) bitangent: vec3<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) world_position: vec3<f32>,
}
@vertex fn vertex(input: Vertex) -> Surface {
    var output: Surface;
    let point = vec4<f32>(input.position, 1.0);
    let eye = frame.view * point;
    let normal = normalize(frame.normal * input.normal);
    let tangent = normalize(frame.normal * input.tangent);
    output.eye_position = eye.xyz / eye.w;
    output.normal = normal;
    output.tangent = tangent;
    output.bitangent = normalize(cross(normal, tangent));
    output.uv = input.uv;
    output.world_position = input.position;
    var clip = frame.view_projection * point;
    // UV Light's original shaded image reads the OpenGL rows bottom first.
    if frame.counts.w != 0u { clip.y = -clip.y; }
    // OpenGL clip z is [-w,w]; WebGPU clip z is [0,w]. Window depth stays [0,1].
    clip.z = (clip.z + clip.w) * 0.5;
    output.position = clip;
    return output;
}
fn uv_shadow(coord: vec4<f32>, strength: f32) -> f32 {
    if coord.w <= 0.0 { return 1.0; }
    let uv = coord.xy / coord.w;
    let depth = (coord.z - 0.01) / coord.w;
    let size = vec2<f32>(textureDimensions(color_map));
    var shadow = 0.0;
    for (var y = -1; y <= 1; y += 2) {
        for (var x = -1; x <= 1; x += 2) {
            let sample_uv = uv + vec2<f32>(f32(x), f32(y)) * 0.0005;
            let pixel = vec2<i32>(clamp(floor(sample_uv * size), vec2<f32>(0.0), size - 1.0));
            shadow += select(0.0, strength, textureLoad(color_map, pixel, 0).r < depth);
        }
    }
    return 1.0 - shadow * 0.25;
}
@fragment fn fragment(input: Surface) -> @location(0) vec4<f32> {
    if frame.counts.y != 0u {
        let previous = textureLoad(previous_depth, vec2<i32>(input.position.xy), 0);
        if input.position.z <= previous + 0.0001 { discard; }
    }
    if frame.counts.w != 0u {
        let light = lights[0];
        let ray = light.position.xyz - input.eye_position;
        let angle = select(0.0, dot(normalize(ray), input.normal), any(ray != vec3<f32>(0.0)));
        let ambient = light.ambient.x;
        let strength = light.ambient.y;
        var shadow = 1.0;
        if strength > 0.0 {
            let matrix = mat4x4<f32>(light.diffuse, light.specular, light.spot_direction_cutoff, light.attenuation_exponent);
            shadow = uv_shadow(matrix * vec4<f32>(input.world_position, 1.0), strength);
        }
        let red = ambient * (angle + 1.0) * 0.5 + (1.0 - ambient) * angle * shadow;
        return vec4<f32>(clamp(red, 0.0, 1.0), 0.0, 0.0, 1.0);
    }
    var base = vec4<f32>(1.0, 1.0, 1.0, frame.material.y);
    if frame.counts.z != 0u {
        base = textureSampleLevel(color_map, color_sampler, input.uv, 0.0);
    }
    var total_ambient = vec4<f32>(0.0);
    var total_diffuse = vec4<f32>(0.0);
    var total_specular = vec4<f32>(0.0);
    for (var ix = 0u; ix < frame.counts.x; ix += 1u) {
        let light = lights[ix];
        var surface_to_light = light.position.xyz;
        if light.position.w != 0.0 { surface_to_light = light.position.xyz - input.eye_position; }
        var ambient = vec4<f32>(0.0);
        var diffuse = vec4<f32>(0.0);
        var specular = vec4<f32>(0.0);
        let light_dot_normal = dot(normalize(surface_to_light), normalize(input.normal));
        if light.position.w == 0.0 {
            ambient = light.ambient;
            if light_dot_normal > 0.0 { diffuse = light.diffuse; specular = light.specular; }
        } else {
            let distance = length(surface_to_light);
            var attenuation = 1.0 / (light.attenuation_exponent.x + light.attenuation_exponent.y * distance + light.attenuation_exponent.z * distance * distance);
            ambient = light.ambient * attenuation;
            if light.spot_direction_cutoff.w < 180.0 {
                let spot_dot = dot(-normalize(surface_to_light), normalize(light.spot_direction_cutoff.xyz));
                if spot_dot > light.spot_cosine.x { attenuation *= pow(spot_dot, light.attenuation_exponent.w); }
                else { attenuation = 0.0; }
            }
            if light_dot_normal > 0.0 { diffuse = light.diffuse * attenuation; specular = light.specular * attenuation; }
        }
        if light_dot_normal > 0.0 {
            diffuse *= light_dot_normal;
            specular *= pow(max(dot(normalize(input.normal), normalize(normalize(surface_to_light) + normalize(-input.eye_position))), 0.0), frame.material.x);
            specular *= light_dot_normal;
        }
        total_ambient += ambient;
        total_diffuse += diffuse;
        total_specular += specular;
    }
    var color = clamp(total_ambient * frame.ambient * base + total_diffuse * frame.diffuse * base + total_specular * frame.specular * base, vec4<f32>(0.0), vec4<f32>(1.0));
    color.a = base.a;
    if frame.material.z != 0.0 {
        // The qualified GL RGBA8 layer truncates positive RGB to the ten
        // binary16 fraction bits before conversion to UNORM8. Binary16
        // subnormals and smaller colors all land in the zero UNORM8 bin.
        let truncated = bitcast<vec3<f32>>(bitcast<vec3<u32>>(color.rgb) & vec3<u32>(0xffffe000u));
        color = vec4<f32>(truncated, color.a);
    }
    return color;
}
