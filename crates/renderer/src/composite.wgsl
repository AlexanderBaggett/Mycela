// Exact source BlendVertexShader projection of its unit-UV quad, not a full-screen identity.
struct CompositeFrame { projection: mat4x4<f32> }
@group(0) @binding(0) var<uniform> frame: CompositeFrame;
@group(0) @binding(1) var layers: texture_2d_array<f32>;
@group(0) @binding(2) var linear_repeat: sampler;
struct Surface { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Surface {
    let coordinates=array<vec2<f32>,6>(vec2<f32>(0.0,0.0),vec2<f32>(0.0,1.0),vec2<f32>(1.0,1.0),vec2<f32>(0.0,0.0),vec2<f32>(1.0,1.0),vec2<f32>(1.0,0.0));
    var output: Surface;
    output.position=frame.projection*vec4<f32>(coordinates[index],0.0,1.0);
    output.uv=coordinates[index];
    return output;
}
@fragment fn fragment(input: Surface) -> @location(0) vec4<f32> {
    // Source textures have a lower-left origin; WebGPU layer storage has a top-left origin.
    // Combined with the source quad projection this preserves the final image's vertical chain.
    let coordinate=vec2<f32>(input.uv.x,1.0-input.uv.y);
    var background=textureSampleLevel(layers,linear_repeat,coordinate,0,0.0);
    for(var layer=1u;layer<min(textureNumLayers(layers),4u);layer+=1u) {
        let foreground=textureSampleLevel(layers,linear_repeat,coordinate,i32(layer),0.0);
        let alpha=foreground.a+background.a*(1.0-foreground.a);
        if alpha>0.0001 {
            background=vec4<f32>((foreground.xyz*foreground.a+background.a*background.xyz*(1.0-foreground.a))/alpha,alpha);
        } else { background=vec4<f32>(0.0); }
    }
    return background;
}
