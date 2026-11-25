//! Finite built-in source registry profile. Missing keys differ from known
//! registered constructors whose retained implementation is not available.
use super::{NodeWireError, PointwiseNode};
use crate::{
    BinaryCursor, ImageGeneratorKind, ImageGeneratorParameterKind, PointwiseError, PointwiseFilter,
    PointwiseKind,
};
fn equals(value: &[u16], literal: &str) -> bool {
    value.iter().copied().eq(literal.encode_utf16())
}

/// Actual built-in apx.b constructor lookup. Dynamic registry mutation and
/// other registered families remain explicit capabilities, not unknown blobs.
pub fn create_node_by_identity<E: 'static>(
    identity: Option<&[u16]>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    create_with_context(identity, &mut super::NodeWireReadContext::for_raw_node())
}

fn create_with_context<E: 'static>(
    identity: Option<&[u16]>,
    context: &mut super::NodeWireReadContext<'_, E>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    let identity = identity.ok_or(PointwiseError::ArgumentNull)?;
    if let Some(lab) = crate::TemplateLabKind::from_identity(identity) {
        let node = PointwiseFilter::new(PointwiseKind::TemplateLab(lab))?.node();
        super::lab::attach_template(&node, lab, context)?;
        return Ok(node);
    }
    if equals(identity, "Wizard Group") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardGroup)?.node());
    }
    if equals(identity, "Wizard Reference") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardReference)?.node());
    }
    if equals(identity, "Group") {
        return Ok(PointwiseFilter::new(PointwiseKind::Group)?.node());
    }
    if equals(identity, "Canvas") {
        return Ok(PointwiseFilter::new(PointwiseKind::Canvas)?.node());
    }
    if equals(identity, "Synthesis") {
        return Ok(PointwiseFilter::new(PointwiseKind::Synthesis)?.node());
    }
    if equals(identity, "Style") {
        return Ok(PointwiseFilter::new(PointwiseKind::Style)?.node());
    }
    if equals(identity, "Map Variable") {
        return Ok(PointwiseFilter::new(PointwiseKind::MapVariable)?.node());
    }
    if equals(identity, "Reference") {
        return Ok(PointwiseFilter::new(PointwiseKind::Reference)?.node());
    }
    if equals(identity, "UV Project") {
        return Ok(PointwiseNode::uv_project()?);
    }
    if equals(identity, "UV Multi Project") {
        return Ok(PointwiseNode::uv_multi_project()?);
    }
    if equals(identity, "UV Apply") {
        return Ok(PointwiseNode::uv_apply()?);
    }
    if equals(identity, "UV Ambient Occlusion") {
        return Ok(PointwiseNode::uv_ambient_occlusion()?);
    }
    if equals(identity, "UV Environment Occlusion") {
        return Ok(PointwiseNode::uv_environment_occlusion()?);
    }
    if equals(identity, "UV Environment Light") {
        return Ok(PointwiseNode::uv_environment_light()?);
    }
    if equals(identity, "UV Curvature") {
        return Ok(PointwiseNode::uv_curvature()?);
    }
    if equals(identity, "UV Light") {
        return Ok(PointwiseNode::uv_light()?);
    }
    if equals(identity, "UV Thickness") {
        return Ok(PointwiseNode::uv_thickness()?);
    }
    if equals(identity, "Combine 3D") {
        return Ok(crate::SceneCombineNode::new().node());
    }
    if equals(identity, "Clear Lights") {
        return Ok(crate::SceneClearLightsNode::new().node());
    }
    if equals(identity, "Transform 3D") {
        return Ok(crate::SceneTransformNode::new()?.node());
    }
    if equals(identity, "Import 3D") {
        return Ok(PointwiseNode::import_3d()?);
    }
    for kind in crate::ScenePrimitiveKind::all() {
        if equals(identity, kind.name()) {
            return Ok(crate::ScenePrimitiveNode::new(kind)?.node());
        }
    }
    if equals(identity, "Input") {
        return Ok(PointwiseNode::input());
    }
    if equals(identity, "Solid Color") {
        return Ok(PointwiseNode::solid_color()?);
    }
    if equals(identity, PointwiseKind::Shader.name()) {
        return Ok(PointwiseFilter::new(PointwiseKind::Shader)?.node());
    }
    if equals(identity, PointwiseKind::OklabColor.name()) {
        // Mycela extension. This identity is outside the original registry.
        return Ok(PointwiseFilter::new(PointwiseKind::OklabColor)?.node());
    }
    if equals(identity, PointwiseKind::Checkerboard.name()) {
        return Ok(PointwiseFilter::new(PointwiseKind::Checkerboard)?.node());
    }
    if context.is_mycela_project() && equals(identity, PointwiseKind::MedianBlur.name()) {
        return Ok(PointwiseFilter::new(PointwiseKind::MedianBlur)?.node());
    }
    if equals(identity, "Invert") {
        return Ok(PointwiseNode::invert());
    }
    // mh constructs up with the actual descriptor name; ordinary Get Channel
    // uses the separately qualified null/default constructor bridge (F223).
    if equals(identity, "Get Channel") {
        return Ok(PointwiseFilter::legacy_channel(None)?.node());
    }
    if ["Hue", "Saturation", "Brightness"]
        .iter()
        .any(|name| equals(identity, name))
    {
        return Ok(PointwiseFilter::legacy_channel(Some(identity))?.node());
    }
    if equals(identity, "Gradient") {
        return Ok(PointwiseNode::image_generator(
            ImageGeneratorKind::Gradient,
        )?);
    }
    if equals(identity, "Noise") {
        return Ok(PointwiseNode::image_generator(ImageGeneratorKind::Noise)?);
    }
    if equals(identity, "Cells") {
        return Ok(PointwiseNode::image_generator(ImageGeneratorKind::Cells)?);
    }
    if equals(identity, "Wood Rings") {
        return Ok(PointwiseNode::image_generator(
            ImageGeneratorKind::WoodRings,
        )?);
    }
    if equals(identity, "Bricks 'n Boards") {
        return Ok(PointwiseFilter::new(PointwiseKind::LegacyBricksBoards)?.node());
    }
    if equals(identity, "Bricks & Boards") {
        return Ok(PointwiseNode::image_generator(
            ImageGeneratorKind::BricksBoards,
        )?);
    }
    if equals(identity, "Imported Image") {
        return Ok(PointwiseNode::image_generator(
            ImageGeneratorKind::ImportedImage,
        )?);
    }
    if equals(identity, "Change BCI") {
        return Ok(PointwiseFilter::new(PointwiseKind::ChangeBci)?.node());
    }
    if equals(identity, "Change HSB") {
        return Ok(PointwiseFilter::new(PointwiseKind::ChangeHsb)?.node());
    }
    if equals(identity, "Colorize") {
        return Ok(PointwiseFilter::new(PointwiseKind::Colorize)?.node());
    }
    if equals(identity, "Colorize G2") {
        return Ok(PointwiseFilter::new(PointwiseKind::ColorizeG2)?.node());
    }
    if equals(identity, "Match Color") {
        return Ok(PointwiseFilter::new(PointwiseKind::MatchColor)?.node());
    }
    if equals(identity, "Distort") {
        return Ok(PointwiseFilter::new(PointwiseKind::Distort)?.node());
    }
    if equals(identity, "Free Transform") {
        return Ok(PointwiseFilter::new(PointwiseKind::FreeTransform)?.node());
    }
    if equals(identity, "Kaleidoscope") {
        return Ok(PointwiseFilter::new(PointwiseKind::Kaleidoscope)?.node());
    }
    if equals(identity, "Move & Repeat") {
        return Ok(PointwiseFilter::new(PointwiseKind::MoveRepeat)?.node());
    }
    if equals(identity, "Remap") {
        return Ok(PointwiseFilter::new(PointwiseKind::Remap)?.node());
    }
    if equals(identity, "Rotate & Tile") {
        return Ok(PointwiseFilter::new(PointwiseKind::RotateTile)?.node());
    }
    if equals(identity, "Art Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::ArtBlur)?.node());
    }
    if equals(identity, "Exposure") {
        return Ok(PointwiseFilter::new(PointwiseKind::Exposure)?.node());
    }
    if equals(identity, "Selector") {
        return Ok(PointwiseFilter::new(PointwiseKind::Selector)?.node());
    }
    if equals(identity, "Frequency") {
        return Ok(PointwiseFilter::new(PointwiseKind::Frequency)?.node());
    }
    if equals(identity, "Fractalize") {
        return Ok(PointwiseFilter::new(PointwiseKind::Fractalize)?.node());
    }
    if equals(identity, "Full Light") {
        return Ok(PointwiseFilter::new(PointwiseKind::FullLight)?.node());
    }
    if equals(identity, "Transmit") {
        return Ok(PointwiseFilter::new(PointwiseKind::Transmit)?.node());
    }
    if equals(identity, "Caustics") {
        return Ok(PointwiseFilter::new(PointwiseKind::Caustics)?.node());
    }
    if equals(identity, "Stereogram") {
        return Ok(PointwiseFilter::new(PointwiseKind::Stereogram)?.node());
    }
    if equals(identity, "Regions") {
        return Ok(PointwiseFilter::new(PointwiseKind::Regions)?.node());
    }
    if equals(identity, "Reflect") {
        return Ok(PointwiseFilter::new(PointwiseKind::Reflect)?.node());
    }
    if equals(identity, "Refract") {
        return Ok(PointwiseFilter::new(PointwiseKind::Refract)?.node());
    }
    if equals(identity, "Reflect Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::ReflectEnvironment)?.node());
    }
    if equals(identity, "Refract Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::RefractEnvironment)?.node());
    }
    if equals(identity, "Diffuse Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::DiffuseEnvironment)?.node());
    }
    if equals(identity, "Scatter Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::ScatterEnvironment)?.node());
    }
    if equals(identity, "Atmosphere") {
        return Ok(PointwiseFilter::new(PointwiseKind::Atmosphere)?.node());
    }
    if equals(identity, "Hair") {
        return Ok(PointwiseFilter::new(PointwiseKind::Hair)?.node());
    }
    if equals(identity, "Convert Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::ConvertEnvironment)?.node());
    }
    if equals(identity, "Make Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::MakeEnvironment)?.node());
    }
    if equals(identity, "Rotate Environment") {
        return Ok(PointwiseFilter::new(PointwiseKind::RotateEnvironment)?.node());
    }
    if equals(identity, "Polygon") {
        return Ok(PointwiseFilter::new(PointwiseKind::Polygon)?.node());
    }
    if equals(identity, "Shadow") {
        return Ok(PointwiseFilter::new(PointwiseKind::Shadow)?.node());
    }
    if equals(identity, "Shadow G2") {
        return Ok(PointwiseFilter::new(PointwiseKind::ShadowG2)?.node());
    }
    if equals(identity, "Illuminate G2") {
        return Ok(PointwiseFilter::new(PointwiseKind::IlluminateG2)?.node());
    }
    if equals(identity, "Point Light") {
        return Ok(PointwiseFilter::new(PointwiseKind::PointLight)?.node());
    }
    if equals(identity, "Equalize") {
        return Ok(PointwiseFilter::new(PointwiseKind::Equalize)?.node());
    }
    if equals(identity, "Levels") {
        return Ok(PointwiseFilter::new(PointwiseKind::Levels)?.node());
    }
    if equals(identity, "A + B") {
        return Ok(PointwiseFilter::new(PointwiseKind::Add)?.node());
    }
    if equals(identity, "A * B") {
        return Ok(PointwiseFilter::new(PointwiseKind::Multiply)?.node());
    }
    if equals(identity, "From HSB") {
        return Ok(PointwiseFilter::new(PointwiseKind::FromHsb)?.node());
    }
    if equals(identity, "Clear Maps") {
        return Ok(PointwiseFilter::new(PointwiseKind::ClearMaps)?.node());
    }
    if equals(identity, "Store Map") {
        return Ok(PointwiseFilter::new(PointwiseKind::StoreMap)?.node());
    }
    if equals(identity, "Merge Maps") {
        return Ok(PointwiseFilter::new(PointwiseKind::MergeMaps)?.node());
    }
    if equals(identity, "Fix Map Contrast") {
        return Ok(PointwiseFilter::new(PointwiseKind::FixMapContrast)?.node());
    }
    if equals(identity, "Layer Maps") {
        return Ok(PointwiseFilter::new(PointwiseKind::LayerMaps)?.node());
    }
    if equals(identity, "Tone Curve") {
        return Ok(PointwiseFilter::new(PointwiseKind::ToneCurve)?.node());
    }
    if equals(identity, "Tone Curve G2") {
        return Ok(PointwiseFilter::new(PointwiseKind::ToneCurveG2)?.node());
    }
    if equals(identity, "Combine") {
        return Ok(PointwiseFilter::new(PointwiseKind::Combine)?.node());
    }
    if equals(identity, "Pixelate") {
        return Ok(PointwiseFilter::new(PointwiseKind::Pixelate)?.node());
    }
    if equals(identity, "Wizard 5 Color") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardFiveColor)?.node());
    }
    if equals(identity, "Wizard Slide Color") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardSlideColor)?.node());
    }
    if equals(identity, "Wizard Kaleidoscope") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardKaleidoscope)?.node());
    }
    if equals(identity, "Wizard Tone Curve") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardToneCurve)?.node());
    }
    if equals(identity, "Wizard Color") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardColor)?.node());
    }
    if equals(identity, "Wizard Matrix") {
        return Ok(PointwiseFilter::new(PointwiseKind::WizardMatrix)?.node());
    }
    if equals(identity, "Matrix") {
        return Ok(PointwiseFilter::new(PointwiseKind::Matrix)?.node());
    }
    if equals(identity, "Rotate") {
        return Ok(PointwiseFilter::new(PointwiseKind::Rotate)?.node());
    }
    if equals(identity, "Gaussian Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::GaussianBlur)?.node());
    }
    if equals(identity, "Sharpen") {
        return Ok(PointwiseFilter::new(PointwiseKind::Sharpen)?.node());
    }
    if equals(identity, "Motion Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::MotionBlur)?.node());
    }
    if equals(identity, "Shape Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::ShapeBlur)?.node());
    }
    if equals(identity, "Smart Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::SmartBlur)?.node());
    }
    if equals(identity, "Smear Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::Smear)?.node());
    }
    if equals(identity, "Variable Blur") {
        return Ok(PointwiseFilter::new(PointwiseKind::VariableBlur)?.node());
    }
    if equals(identity, "Store Channel") {
        return Ok(PointwiseFilter::new(PointwiseKind::StoreMap)?.node());
    }
    if equals(identity, "Layer Channels") {
        return Ok(PointwiseFilter::new(PointwiseKind::LayerMaps)?.node());
    }
    if equals(identity, "Clear Channels") {
        return Ok(PointwiseFilter::new(PointwiseKind::ClearMaps)?.node());
    }
    if equals(identity, "Merge Channels") {
        return Ok(PointwiseFilter::new(PointwiseKind::MergeMaps)?.node());
    }
    if SOURCE_REGISTRY_NAMES
        .iter()
        .any(|name| equals(identity, name))
    {
        return Err(NodeWireError::UnimplementedNode {
            identity: identity.to_vec(),
        });
    }
    // Median Blur has a real direct constructor but NO default apx entry.
    // It reaches this exact missing-registry Argument/className boundary.
    Err(NodeWireError::FactoryArgument {
        class_name: identity.to_vec(),
    })
}
/// zl.g: identity was consumed by caller, strict historical rewrite precedes
/// construction, and only a successfully loaded child is returned to its slot.
pub fn load_node_binary<E: 'static>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    identity: Option<&[u16]>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    load_node_binary_with_context(reader, version, identity, None)
}
pub(crate) fn load_node_binary_with_context<E: 'static>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    identity: Option<&[u16]>,
    mut context: Option<&mut super::NodeWireReadContext<'_, E>>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    if let Some(context) = context.as_deref_mut() {
        context.enter()?;
    }
    let result = (|| {
        if identity.is_some_and(|name| equals(name, "PixelScript")) {
            return import_pixel_script(reader, version, context.as_deref_mut());
        }
        let node = create_versioned_node(identity, version, context.as_deref_mut())?;
        node.load_binary_with_context(reader, version, context.as_deref_mut())?;
        if node.kind() == Some(PointwiseKind::ColorizeG2)
            && context
                .as_ref()
                .is_some_and(|context| context.is_mycela_project())
        {
            return Ok(node);
        }
        migrate_loaded_node(node)
    })();
    if let Some(context) = context {
        context.leave();
    }
    result
}

pub(super) fn create_versioned_node<E: 'static>(
    identity: Option<&[u16]>,
    version: f64,
    context: Option<&mut super::NodeWireReadContext<'_, E>>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    // Null identity enters the source XML attribute path; literal "null" is
    // handled exclusively by root/slot callers, never by this factory.
    let identity = identity.ok_or(PointwiseError::NullReference)?;
    let historical = if version < 3.05 && equals(identity, "Colorize") {
        Some("Colorize G2")
    } else if version < 3.06 && equals(identity, "Tone Curve") {
        Some("Tone Curve G2")
    } else if version < 3.07 && equals(identity, "Shadow") {
        Some("Shadow G2")
    } else if version < 3.08 && equals(identity, "Illuminate") {
        Some("Illuminate G2")
    } else if version < 3.11 && equals(identity, "Illuminate") {
        Some("Point Light")
    } else {
        None
    };
    // All < comparisons are false for NaN, matching the source bge.un gates.
    let rewritten = historical.map(|name| name.encode_utf16().collect::<Vec<_>>());
    let identity = rewritten.as_deref().unwrap_or(identity);
    let node = match context {
        Some(context) => create_with_context(Some(identity), context)?,
        None => create_node_by_identity(Some(identity))?,
    };
    if version < 3.58
        && node
            .image_generator_handle()
            .is_some_and(|model| model.kind() == ImageGeneratorKind::ImportedImage)
    {
        let text: Vec<_> = "Cubic".encode_utf16().collect();
        node.image_generator_handle()
            .ok_or(PointwiseError::InvalidCast)?
            .parameter(ImageGeneratorParameterKind::UpsizeSmoothing)
            .set_text(Some(&text))?;
    }
    Ok(node)
}

pub(super) fn migrate_loaded_node<E: 'static>(
    node: PointwiseNode<E>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    let migrated = match node.kind() {
        Some(PointwiseKind::IlluminateG2) => PointwiseFilter::point_light_from_illuminate(
            &node.filter().ok_or(PointwiseError::InvalidCast)?,
        )?,
        Some(PointwiseKind::ToneCurveG2) => super::super::tone_curve::migrate_legacy(
            &node.filter().ok_or(PointwiseError::InvalidCast)?,
        )?,
        Some(PointwiseKind::ColorizeG2) => {
            let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
            let Some(replacement) = super::super::colorize_g2::migrate(&filter)? else {
                return Ok(node);
            };
            replacement
        }
        _ => return Ok(node),
    }
    .node();
    migrated.slot(0)?.set_node(node.slot(0)?.node().as_ref())?;
    Ok(migrated)
}

// Exact literal apx initializer names via ElementName.Name (last raw label).
// This table grants no constructor/runtime/resource/availability success.
const SOURCE_REGISTRY_NAMES: &[&str] = &[
    "Canvas",
    "Cut & Tile Lab",
    "Fiber Lab",
    "Layer Lab",
    "Noise Lab",
    "PixelScript",
    "Select Noise",
    "Select Pattern",
    "Style",
    "Substance Lab",
    "Synthesis",
    "Weather Lab",
    "Wood Lab",
    "Caustics",
    "Full Light",
    "Point Light",
    "Reflect",
    "Refract",
    "Shadow",
    "Stereogram",
    "Transmit",
    "Cone",
    "Cube",
    "Cylinder",
    "Disc",
    "Import 3D",
    "Plane",
    "Sphere",
    "Torus",
    "Clear Lights",
    "Combine 3D",
    "Transform 3D",
    "UV Ambient Occlusion",
    "UV Apply",
    "UV Curvature",
    "UV Environment Light",
    "UV Environment Occlusion",
    "UV Light",
    "UV Multi Project",
    "UV Project",
    "UV Thickness",
    "Art Blur",
    "Gaussian Blur",
    "Motion Blur",
    "Shape Blur",
    "Smart Blur",
    "Smear Blur",
    "Variable Blur",
    "Change BCI",
    "Change HSB",
    "Colorize",
    "Equalize",
    "Invert",
    "Levels",
    "Match Color",
    "Tone Curve",
    "A + B",
    "A * B",
    "Combine",
    "From HSB",
    "Clear Maps",
    "Fix Map Contrast",
    "Layer Maps",
    "Merge Maps",
    "Store Map",
    "Convert Environment",
    "Diffuse Environment",
    "Exposure",
    "Make Environment",
    "Reflect Environment",
    "Refract Environment",
    "Rotate Environment",
    "Scatter Environment",
    "Fractalize",
    "Frequency",
    "Get Channel",
    "Matrix",
    "Pixelate",
    "Selector",
    "Sharpen",
    "Atmosphere",
    "Bricks & Boards",
    "Cells",
    "Gradient",
    "Hair",
    "Imported Image",
    "Noise",
    "Polygon",
    "Regions",
    "Solid Color",
    "Wood Rings",
    "Group",
    "Reference",
    "Distort",
    "Free Transform",
    "Kaleidoscope",
    "Move & Repeat",
    "Remap",
    "Rotate",
    "Rotate & Tile",
    "Wizard Group",
    "Wizard Reference",
    "Wizard Color",
    "Wizard 5 Color",
    "Wizard Slide Color",
    "Map Variable",
    "Wizard Kaleidoscope",
    "Wizard Tone Curve",
    "Wizard Matrix",
    "Select Area",
    "Select Rings",
    "Rings",
    "Slot Reference",
    "Colorize G2",
    "Illuminate G2",
    "Move and Repeat",
    "Shadow G2",
    "Bricks 'n Boards",
    "Tone Curve G2",
    "Rotate and Tile",
    "Make Tiles",
    "Store Channel",
    "Layer Channels",
    "Clear Channels",
    "Merge Channels",
    "Lab Wrapper Node",
    "Material Lab",
    "Input",
    "Hue",
    "Saturation",
    "Brightness",
];

/// Consume the source binary layout without evaluating or compiling its C#.
#[allow(clippy::approx_constant)] // Genetica encoding revision numbers.
fn import_pixel_script<E: 'static>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    mut context: Option<&mut super::NodeWireReadContext<'_, E>>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    use crate::{
        LimitsRecord, ModernAdjustmentConfiguration, ModernPropertyConfiguration,
        ModernPropertyRecord, ModernPropertyValue,
    };
    let blocks = reader.read_u8()?;
    if blocks == 0 {
        return Err(NodeWireError::PropertyIndex);
    }
    if (version >= 3.72 || version == 3.71 || version == 3.70)
        && (reader.read_bool()? || reader.read_bool()?)
    {
        return Err(NodeWireError::UnsupportedCapability(
            "animated or adjusted PixelScript source",
        ));
    }
    let source = String::from_utf16(&reader.read_string()?)
        .map_err(|_| PointwiseError::Shader("PixelScript source is invalid Unicode.".into()))?;
    let modern_number = |reader: &mut BinaryCursor<'_>| {
        let mut record = ModernPropertyRecord::new(
            ModernPropertyValue::Double {
                current: 0.,
                previous: 0.,
            },
            ModernPropertyConfiguration {
                can_create_animation: true,
                adjustment: Some(ModernAdjustmentConfiguration {
                    bounds: vec![[0., 1.]],
                    property_type: 0,
                }),
            },
        )
        .map_err(PointwiseError::ModernProperty)?;
        record
            .load(reader, version)
            .map_err(PointwiseError::ModernProperty)?;
        Ok::<_, NodeWireError<E>>(())
    };
    if version >= 3.54 {
        let properties = reader.read_i32()?;
        if !(0..=6).contains(&properties) {
            return Err(NodeWireError::PropertyIndex);
        }
        for _ in 0..properties {
            if version >= 3.72 {
                reader.read_string()?;
                modern_number(reader)?;
                if version >= 3.73 {
                    LimitsRecord::default()
                        .load(reader)
                        .map_err(|error| PointwiseError::Shader(error.to_string()))?;
                }
            } else {
                reader.read_f64()?;
                reader.read_f64()?;
                reader.read_i32()?;
                reader.read_string()?;
                reader.read_string()?;
                if version >= 3.55 {
                    reader.read_f64()?;
                }
            }
        }
    }
    if blocks > 7 {
        return Err(NodeWireError::PropertyIndex);
    }
    for _ in 1..blocks {
        modern_number(reader)?;
    }
    let node = PointwiseFilter::new(PointwiseKind::Shader)?.node();
    node.load_shader_program(std::sync::Arc::new(
        super::super::ShaderProgram::from_pixel_script(source),
    ));
    let slots = reader.read_u8()?;
    if slots != 2 {
        return Err(NodeWireError::SlotCountMismatch {
            serialized: slots,
            constructed: 2,
        });
    }
    for index in 0..slots {
        let identity = reader.read_string()?;
        if !equals(&identity, "null") {
            let child = load_node_binary_with_context(
                reader,
                version,
                Some(&identity),
                context.as_deref_mut(),
            )?;
            node.slot(index.into())?.set_node(Some(&child))?;
        }
    }
    Ok(node)
}
