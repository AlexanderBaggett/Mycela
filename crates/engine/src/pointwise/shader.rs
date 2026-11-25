//! The Mycela fragment-shader interface and reflected image inputs.
use std::{ops::Range, sync::Arc};

mod swizzle;
use swizzle::SwizzleLowering;

pub const DEFAULT_SHADER_SOURCE: &str =
    "fn shade(uv: vec2f) -> vec4f {\n    return vec4f(uv, 0.0, 1.0);\n}\n";

const WRAPPER: &str = r#"
struct MycelaUniforms {
    resolution: vec2f,
    time: f32,
}

@group(0) @binding(0) var<uniform> mycela_uniforms: MycelaUniforms;
@group(0) @binding(1) var mycela_sampler: sampler;
var<private> resolution: vec2f;
var<private> texel_size: vec2f;
var<private> time: f32;
var<private> pixel: vec2f;

fn sample_input(image: texture_2d<f32>, uv: vec2f) -> vec4f {
    return textureSampleLevel(image, mycela_sampler, uv, 0.0);
}

@vertex
fn mycela_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return vec4f(array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0))[index], 0.0, 1.0);
}

@fragment
fn mycela_fragment(@builtin(position) position: vec4f) -> @location(0) vec4f {
    resolution = mycela_uniforms.resolution;
    time = mycela_uniforms.time;
    texel_size = 1.0 / resolution;
    pixel = position.xy;
    return shade(pixel * texel_size);
}
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderInput {
    name: String,
    binding: u32,
}

impl ShaderInput {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn binding(&self) -> u32 {
        self.binding
    }
}

#[derive(Clone, Debug)]
pub struct ShaderProgram {
    source: Arc<str>,
    wgsl: Arc<str>,
    inputs: Vec<ShaderInput>,
    imported_pixel_script: bool,
    time_dependent: bool,
}

impl ShaderProgram {
    pub fn compile(source: &str) -> Result<Self, String> {
        let tokens = tokens(source);
        let mut wgsl = source.to_string();
        let mut swizzles = false;
        let mut directives = tokens.as_slice();
        while let [first, rest @ ..] = directives {
            if !matches!(&source[first.clone()], "requires" | "enable" | "diagnostic") {
                break;
            }
            let Some(end) = rest.iter().position(|token| &source[token.clone()] == ";") else {
                break;
            };
            if matches!(&source[first.clone()], "requires" | "enable")
                && let Some(index) = rest[..end]
                    .iter()
                    .position(|token| &source[token.clone()] == "swizzle_assignment")
            {
                swizzles = true;
                let single =
                    end == 1 || (end == 2 && index == 0 && &source[rest[1].clone()] == ",");
                let span = if single {
                    first.start..rest[end].end
                } else if index + 1 < end && &source[rest[index + 1].clone()] == "," {
                    rest[index].start..rest[index + 1].end
                } else if index > 0 && &source[rest[index - 1].clone()] == "," {
                    rest[index - 1].start..rest[index].end
                } else {
                    return Err("Separate WGSL extensions with commas.".into());
                };
                let whitespace: String = source.as_bytes()[span.clone()]
                    .iter()
                    .map(|&byte| if byte == b'\n' { '\n' } else { ' ' })
                    .collect();
                wgsl.replace_range(span, &whitespace);
            }
            directives = &rest[end + 1..];
        }
        wgsl.push_str(WRAPPER);
        let reserved = tokens
            .iter()
            .any(|span| source[span.clone()].starts_with("mycela_swizzle_"));
        let mut lowering = SwizzleLowering::default();
        let module = loop {
            match naga::front::wgsl::parse_str(&wgsl) {
                Ok(module) => break module,
                Err(error) => {
                    if lowering.rewrite(&mut wgsl, &error, swizzles, reserved)? {
                        continue;
                    }
                    return Err(error.emit_to_string(&wgsl));
                }
            }
        };
        if module.entry_points.len() != 2 {
            return Err("Define fn shade(uv: vec2f) -> vec4f as the shader entry point.".into());
        }
        let time_dependent = module.functions.iter().any(|(_, function)| {
            function.expressions.iter().any(|(_, expression)| {
                matches!(expression, naga::Expression::GlobalVariable(handle)
                    if module.global_variables[*handle].name.as_deref() == Some("time"))
            })
        });
        let mut inputs = Vec::new();
        for (_, global) in module.global_variables.iter() {
            let Some(binding) = &global.binding else {
                continue;
            };
            let name = global.name.as_deref().unwrap_or_default();
            if binding.group == 0
                && [("mycela_uniforms", 0), ("mycela_sampler", 1)]
                    .contains(&(name, binding.binding))
            {
                continue;
            }
            if binding.group != 1
                || !matches!(
                    module.types[global.ty].inner,
                    naga::TypeInner::Image {
                        dim: naga::ImageDimension::D2,
                        arrayed: false,
                        class: naga::ImageClass::Sampled {
                            kind: naga::ScalarKind::Float,
                            multi: false
                        },
                    }
                )
            {
                return Err(format!(
                    "Declare {name} as a group 1 texture_2d<f32> to create a graph input."
                ));
            }
            if binding.binding >= 1000 {
                return Err(format!(
                    "Input {name} needs a binding number from 0 to 999."
                ));
            }
            inputs.push(ShaderInput {
                name: name.to_owned(),
                binding: binding.binding,
            });
        }
        if inputs.len() > 16 {
            return Err("A Shader node supports up to 16 input textures.".into());
        }
        inputs.sort_by_key(|input| input.binding);
        if inputs
            .windows(2)
            .any(|pair| pair[0].binding == pair[1].binding)
        {
            return Err("Every Shader input needs its own binding number.".into());
        }
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .map_err(|error| error.emit_to_string(&wgsl))?;
        Ok(Self {
            source: source.into(),
            wgsl: wgsl.into(),
            inputs,
            imported_pixel_script: false,
            time_dependent,
        })
    }

    pub(super) fn from_pixel_script(source: String) -> Self {
        Self {
            source: source.into(),
            wgsl: "".into(),
            inputs: (0..2)
                .map(|binding| ShaderInput {
                    binding,
                    name: format!("input_{}", binding + 1),
                })
                .collect(),
            imported_pixel_script: true,
            time_dependent: false,
        }
    }

    pub fn is_time_dependent(&self) -> bool {
        self.time_dependent
    }

    pub fn is_imported_pixel_script(&self) -> bool {
        self.imported_pixel_script
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn wgsl(&self) -> &str {
        &self.wgsl
    }

    pub fn inputs(&self) -> &[ShaderInput] {
        &self.inputs
    }
}

fn tokens(source: &str) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((start, character)) = chars.next() {
        if character.is_whitespace() {
            continue;
        }
        if character == '/' && chars.next_if(|(_, next)| *next == '/').is_some() {
            while chars.next_if(|(_, next)| *next != '\n').is_some() {}
            continue;
        }
        if character == '/' && chars.next_if(|(_, next)| *next == '*').is_some() {
            let mut depth = 1;
            while let Some((_, next)) = chars.next() {
                if next == '*' && chars.next_if(|(_, next)| *next == '/').is_some() {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if next == '/' && chars.next_if(|(_, next)| *next == '*').is_some() {
                    depth += 1;
                }
            }
            continue;
        }
        if character.is_alphanumeric() || character == '_' {
            while chars
                .next_if(|(_, next)| next.is_alphanumeric() || *next == '_')
                .is_some()
            {}
        }
        result.push(start..chars.peek().map_or(source.len(), |(end, _)| *end));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_reflects_ports_and_requires_swizzle_opt_in() {
        ShaderProgram::compile(DEFAULT_SHADER_SOURCE).unwrap();
        let source = r#"
@group(1) @binding(7) var overlay: texture_2d<f32>;
@group(1) @binding(2) var base: texture_2d<f32>;
fn shade(uv: vec2f) -> vec4f {
    var color = sample_input(base, uv) + sample_input(overlay, uv);
    color.rg = color.gr;
    color.ba += vec2f(0.5);
    return color;
}
"#;
        let error = ShaderProgram::compile(source).unwrap_err();
        assert!(error.contains("requires swizzle_assignment"), "{error}");
        let program =
            ShaderProgram::compile(&format!("requires swizzle_assignment;{source}")).unwrap();
        let ports: Vec<_> = program
            .inputs()
            .iter()
            .map(|input| (input.name(), input.binding()))
            .collect();
        assert_eq!(ports, [("base", 2), ("overlay", 7)]);
        assert!(program.source().contains("color.rg = color.gr"));
        assert!(program.wgsl().contains("mycela_swizzle_base"));
    }

    #[test]
    fn shader_supports_swizzle_writes_in_for_headers() {
        let source = r#"
fn shade(uv: vec2f) -> vec4f {
    var index = vec2u();
    var color = vec4f(uv, 0.0, 1.0);
    for (index.yx = vec2u(3u, 0u); index.x < 2u; index.xy += vec2u(1u)) {
        if index.x == 0u { continue; }
        for (color.rg = uv; color.r < 0.5; color.rg += vec2f(0.25)) {
            var index = 4.0;
            color.b += index;
        }
    }
    return color;
}
"#;
        let error = ShaderProgram::compile(source).unwrap_err();
        assert!(error.contains("requires swizzle_assignment"), "{error}");
        let enabled = format!("requires swizzle_assignment,;{source}");
        let program = ShaderProgram::compile(&enabled).unwrap();
        assert_eq!(program.source(), enabled);
    }

    #[test]
    fn shader_composes_swizzles_without_confusing_similarly_named_struct_fields() {
        let source = r#"
struct Pixel { rg: vec4f }
fn shade(uv: vec2f) -> vec4f {
    var pixel_value = Pixel(vec4f(uv, 0.0, 1.0));
    pixel_value.rg.yx = vec2f(0.5);
    let pointer = &pixel_value.rg;
    pointer.agb.yx += vec2f(0.5);
    (pixel_value.rg.bgr).y = 0.25;
    pixel_value.rg.bgr[u32(uv.x)] *= 0.5;
    var counters = vec4u();
    for (; counters.x < 2u; counters.yx.y++) {
        counters.wz[1u]++;
    }
    counters.zw[0u]--;
    return pixel_value.rg;
}
"#;
        assert!(
            ShaderProgram::compile(source)
                .unwrap_err()
                .contains("requires swizzle_assignment")
        );
        let enabled = format!("requires swizzle_assignment;{source}");
        assert_eq!(ShaderProgram::compile(&enabled).unwrap().source(), enabled);
        let duplicates = enabled.replace("pointer.agb.yx", "pointer.aab.xy");
        assert!(
            ShaderProgram::compile(&duplicates)
                .unwrap_err()
                .contains("distinct destination components")
        );
        let immutable = r#"
requires swizzle_assignment;
fn shade(uv: vec2f) -> vec4f {
    let color = vec4f(uv, 0.0, 1.0);
    color.rgba.rg = uv;
    return color;
}
"#;
        assert!(
            ShaderProgram::compile(immutable)
                .unwrap_err()
                .contains("needs a writable vector")
        );
    }

    #[test]
    fn shader_layout_remaps_connections_by_variable_and_round_trips() {
        const FIRST: &str = "
@group(1) @binding(0) var image: texture_2d<f32>;
fn shade(uv: vec2f) -> vec4f { return sample_input(image, uv); }
";
        const SECOND: &str = "
@group(1) @binding(8) var image: texture_2d<f32>;
@group(1) @binding(1) var mask: texture_2d<f32>;
fn shade(uv: vec2f) -> vec4f { return sample_input(image, uv) * sample_input(mask, uv).r; }
";
        let shader = crate::PointwiseFilter::<()>::new(crate::PointwiseKind::Shader)
            .unwrap()
            .node();
        shader
            .edit_shader_program(Arc::new(ShaderProgram::compile(FIRST).unwrap()))
            .unwrap();
        let image = shader.slot(0).unwrap();
        let color = crate::PointwiseNode::solid_color().unwrap();
        image.set_node(Some(&color)).unwrap();
        shader
            .edit_shader_program(Arc::new(ShaderProgram::compile(SECOND).unwrap()))
            .unwrap();
        assert!(shader.slot(1).unwrap().same_reference(&image));
        let mut document = crate::Gtx375GraphDocument::new(vec![Some(shader.clone())]).unwrap();
        assert_eq!(document.gtx_incompatibilities().unwrap().len(), 1);
        let mut writer = crate::BinaryWriter::default();
        document.save_project(&mut writer).unwrap();
        let reopened = crate::Gtx375GraphDocument::<()>::load_project(
            writer.bytes(),
            crate::NodeWireReadPolicy::new(32, 128),
        )
        .unwrap();
        let shader = reopened.branch(0).flatten().unwrap();
        assert_eq!(shader.shader_program().unwrap().source(), SECOND);
        assert!(shader.slot(0).unwrap().node().is_none());
        assert!(
            shader
                .slot(1)
                .unwrap()
                .node()
                .unwrap()
                .solid_color_values()
                .is_ok()
        );
    }
}

#[cfg(test)]
#[test]
fn shader_imports_genetica_pixel_script_and_reopens_the_original_text() {
    use crate::{
        BinaryCursor, BinaryWriter, Gtx375GraphDocument, LimitsRecord, NodeWireReadPolicy,
    };
    let original = "real v = property(1);\r\nresult = new color(v, 0, 0, 1);";
    let mut wire = BinaryWriter::default();
    wire.write_u8(2).unwrap();
    wire.write_bool(false).unwrap();
    wire.write_bool(false).unwrap();
    wire.write_utf16(&original.encode_utf16().collect::<Vec<_>>())
        .unwrap();
    wire.write_i32(1).unwrap();
    wire.write_utf16(&[b'v' as u16]).unwrap();
    for index in 0..2 {
        wire.write_bool(false).unwrap();
        wire.write_bool(false).unwrap();
        wire.write_f64(0.5).unwrap();
        if index == 0 {
            LimitsRecord::default().save(&mut wire).unwrap();
        }
    }
    wire.write_u8(2).unwrap();
    crate::PointwiseNode::<()>::solid_color()
        .unwrap()
        .save_binary(&mut wire)
        .unwrap();
    crate::PointwiseFilter::<()>::new(crate::PointwiseKind::GaussianBlur)
        .unwrap()
        .node()
        .save_binary(&mut wire)
        .unwrap();
    let mut reader = BinaryCursor::new(wire.bytes()).unwrap();
    let identity: Vec<_> = "PixelScript".encode_utf16().collect();
    let node = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&identity)).unwrap();
    assert_eq!(reader.position().unwrap() as usize, wire.bytes().len());
    let mut document = Gtx375GraphDocument::new(vec![Some(node.clone())]).unwrap();
    let issues = document.gtx_incompatibilities().unwrap();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].to_string().contains("Shader"));
    let mut native = BinaryWriter::default();
    document.save_project(&mut native).unwrap();
    let reopened =
        Gtx375GraphDocument::<()>::load_project(native.bytes(), NodeWireReadPolicy::new(20, 100))
            .unwrap();
    for node in [node, reopened.branch(0).flatten().unwrap()] {
        let shader = node.shader_program().unwrap();
        assert_eq!(shader.source(), original);
        assert!(shader.is_imported_pixel_script());
        assert!(shader.wgsl().is_empty());
        assert_eq!(shader.inputs()[0].name(), "input_1");
        assert_eq!(shader.inputs()[1].name(), "input_2");
        assert!(
            node.slot(0)
                .unwrap()
                .node()
                .unwrap()
                .solid_color_values()
                .is_ok()
        );
        assert_eq!(
            node.slot(1).unwrap().node().unwrap().kind(),
            Some(crate::PointwiseKind::GaussianBlur),
        );
    }
    assert!(ShaderProgram::compile(original).is_err());
}
