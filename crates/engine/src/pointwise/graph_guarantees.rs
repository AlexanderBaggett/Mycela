//! Shared channel rules for final rendering and incrementally prepared trees.
use super::{Payload, PointwiseNode, guard};
use crate::pointwise::{PointwiseError, PointwiseKind};
use std::collections::BTreeSet;

/// Four source channels: an exact guaranteed scalar and the source's declared
/// 0..=1 range flag. This is an input/output value, not a retained zl owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointwiseChannelGuarantees {
    constants: [Option<f32>; 4],
    in_range: [bool; 4],
    has_scene: bool,
    passthrough_slot: Option<usize>,
}
impl PointwiseChannelGuarantees {
    /// Construct a channel result supplied by an already-prepared child.
    pub const fn new(
        constants: [Option<f32>; 4],
        in_range: [bool; 4],
        has_scene: bool,
        passthrough_slot: Option<usize>,
    ) -> Self {
        Self {
            constants,
            in_range,
            has_scene,
            passthrough_slot,
        }
    }
    pub const fn constants(&self) -> [Option<f32>; 4] {
        self.constants
    }
    pub const fn in_range(&self) -> [bool; 4] {
        self.in_range
    }
    pub const fn has_scene(&self) -> bool {
        self.has_scene
    }
    pub const fn passthrough_slot(&self) -> Option<usize> {
        self.passthrough_slot
    }
    /// Source `l/m/h` for an absent slot: scalar zero on every channel.
    pub fn missing() -> Self {
        Self {
            constants: [Some(0.0); 4],
            in_range: [true; 4],
            has_scene: false,
            passthrough_slot: None,
        }
    }
    pub(in crate::pointwise) fn effective_range(&self, ix: usize) -> bool {
        match self.constants[ix] {
            Some(v) => (0.0..=1.0).contains(&v),
            None => self.in_range[ix],
        }
    }
}

fn animated<E: 'static>(
    p: &crate::pointwise::PointwiseParameter<E>,
) -> Result<bool, PointwiseError<E>> {
    if p.binding_index() >= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "guarantee adjustment ancestor context",
        ));
    }
    Ok(p.retained_animation()?
        .as_ref()
        .map(|a| a.is_active())
        .transpose()?
        .unwrap_or(false))
}

impl<E: 'static> PointwiseNode<E> {
    pub(super) fn is_guaranteed_scalar_source(&self) -> bool {
        self.image_generator_handle().is_some_and(|generator| {
            matches!(
                generator.kind(),
                crate::ImageGeneratorKind::Gradient
                    | crate::ImageGeneratorKind::Cells
                    | crate::ImageGeneratorKind::Noise
                    | crate::ImageGeneratorKind::WoodRings
                    | crate::ImageGeneratorKind::BricksBoards
            )
        })
    }

    /// Calculate supported multi-input guarantees in source slot order.
    /// Missing slots have the source zero-channel guarantee.
    pub fn calculate_image_guarantees_for_inputs(
        &self,
        inputs: &[Option<PointwiseChannelGuarantees>],
    ) -> Result<PointwiseChannelGuarantees, PointwiseError<E>> {
        self.calculate_image_guarantees_for_inputs_with_uniformity(inputs)
            .map(|(channels, _)| channels)
    }

    pub(super) fn calculate_image_guarantees_for_inputs_with_uniformity(
        &self,
        inputs: &[Option<PointwiseChannelGuarantees>],
    ) -> Result<(PointwiseChannelGuarantees, bool), PointwiseError<E>> {
        if matches!(
            self.kind(),
            Some(
                PointwiseKind::Canvas
                    | PointwiseKind::Style
                    | PointwiseKind::Synthesis
                    | PointwiseKind::Shader
            )
        ) {
            return Ok((
                PointwiseChannelGuarantees::new(
                    [None; 4],
                    [false; 4],
                    inputs.iter().enumerate().any(|(index, input)| {
                        self.uses_image_slot(index) && input.is_some_and(|map| map.has_scene())
                    }),
                    None,
                ),
                false,
            ));
        }
        if let Some(
            kind @ (PointwiseKind::Add | PointwiseKind::Multiply | PointwiseKind::FromHsb),
        ) = self.kind()
        {
            if inputs.len() != kind.input_count() {
                return Err(PointwiseError::Argument);
            }
            return Ok(arithmetic_guarantees(kind, inputs));
        }
        if self.kind() == Some(PointwiseKind::OklabColor) {
            if inputs.len() != PointwiseKind::OklabColor.input_count() {
                return Err(PointwiseError::Argument);
            }
            // Values can vary with the inputs or the sample time. Keep the
            // kernel in the plan and request any scene-backed input images.
            return Ok((
                PointwiseChannelGuarantees::new(
                    [None; 4],
                    [true; 4],
                    inputs.iter().flatten().any(|input| input.has_scene),
                    None,
                ),
                false,
            ));
        }
        if self.kind() == Some(PointwiseKind::Hair) {
            let [_, density, _] = inputs else {
                return Err(PointwiseError::Argument);
            };
            let density = density.unwrap_or_else(PointwiseChannelGuarantees::missing);
            let empty = density.constants[0] == Some(0.0);
            return Ok((
                PointwiseChannelGuarantees::new(
                    if empty {
                        [Some(0.), Some(0.), Some(0.), Some(1.)]
                    } else {
                        [None; 4]
                    },
                    [false; 4],
                    !empty && inputs.iter().flatten().any(|input| input.has_scene),
                    None,
                ),
                empty,
            ));
        }
        if self.kind() == Some(PointwiseKind::Regions) {
            let [pattern, image] = inputs else {
                return Err(PointwiseError::Argument);
            };
            let slots = [*pattern, *image]
                .map(|input| input.unwrap_or_else(PointwiseChannelGuarantees::missing));
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            let mut result = super::super::regions::guarantee_basis(&filter, slots)?;
            let uniform = !result.has_scene && result.constants.iter().all(Option::is_some);
            if uniform
                && let Some(pass) = slots
                    .iter()
                    .position(|input| input.constants == result.constants)
            {
                result.passthrough_slot = Some(pass);
                result.in_range =
                    std::array::from_fn(|channel| slots[pass].effective_range(channel));
            }
            return Ok((result, uniform));
        }
        if self.kind() == Some(PointwiseKind::FullLight) {
            if inputs.len() != 4 {
                return Err(PointwiseError::Argument);
            }
            return Ok((
                PointwiseChannelGuarantees::new(
                    [None; 4],
                    [false; 4],
                    inputs.iter().flatten().any(|v| v.has_scene),
                    None,
                ),
                false,
            ));
        }
        if self.kind() == Some(PointwiseKind::Remap) {
            let [original, _, _] = inputs else {
                return Err(PointwiseError::Argument);
            };
            return Ok(remap_guarantees(
                original.unwrap_or_else(PointwiseChannelGuarantees::missing),
                inputs.iter().flatten().any(|input| input.has_scene),
            ));
        }
        if let Some(
            kind @ (PointwiseKind::MotionBlur
            | PointwiseKind::ShapeBlur
            | PointwiseKind::VariableBlur),
        ) = self.kind()
        {
            if inputs.len() != kind.input_count() {
                return Err(PointwiseError::Argument);
            }
            return self.calculate_image_guarantees_with_uniformity(Some(
                PointwiseChannelGuarantees {
                    has_scene: inputs.iter().flatten().any(|input| input.has_scene),
                    ..inputs[0].unwrap_or_else(PointwiseChannelGuarantees::missing)
                },
            ));
        }
        if matches!(
            self.kind(),
            Some(PointwiseKind::Distort | PointwiseKind::Smear)
        ) {
            let [original, _, distance] = inputs else {
                return Err(PointwiseError::Argument);
            };
            let original = original.unwrap_or_else(PointwiseChannelGuarantees::missing);
            let distance = distance.unwrap_or_else(PointwiseChannelGuarantees::missing);
            let amount = self
                .filter()
                .ok_or(PointwiseError::InvalidCast)?
                .parameter(0)?;
            let pass = distance.constants[0] == Some(0.)
                || (!property_is_animated(&amount)?
                    && amount
                        .normalized_pair()?
                        .iter()
                        .all(|&value| value >= 10000.))
                || (!original.has_scene && original.constants.iter().all(Option::is_some));
            return Ok((
                PointwiseChannelGuarantees::new(
                    original.constants,
                    std::array::from_fn(|channel| original.effective_range(channel)),
                    original.has_scene,
                    pass.then_some(0),
                ),
                false,
            ));
        }
        if !matches!(
            self.kind(),
            Some(
                PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
                    | PointwiseKind::Reflect
                    | PointwiseKind::Refract
                    | PointwiseKind::Stereogram
                    | PointwiseKind::Transmit
            )
        ) {
            if inputs.len() > 1 {
                return Err(PointwiseError::UnsupportedCapability(
                    "multi-input node guarantee calculation",
                ));
            }
            return self
                .calculate_image_guarantees_with_uniformity(inputs.first().copied().flatten());
        }
        if inputs.len() != 2 {
            return Err(PointwiseError::Argument);
        }
        let slots = [
            inputs[0].unwrap_or_else(PointwiseChannelGuarantees::missing),
            inputs[1].unwrap_or_else(PointwiseChannelGuarantees::missing),
        ];
        let planar = matches!(
            self.kind(),
            Some(PointwiseKind::Reflect | PointwiseKind::Refract)
        );
        let transmit = self.kind() == Some(PointwiseKind::Transmit);
        let source = if planar || transmit || self.kind() == Some(PointwiseKind::Stereogram) {
            slots[0]
        } else {
            slots[1]
        };
        let mut out = PointwiseChannelGuarantees::new(
            if transmit {
                source
                    .constants
                    .map(|v| v.filter(|n| *n == 0.0).map(|_| 0.0))
            } else {
                source.constants
            },
            if planar || transmit {
                [false; 4]
            } else {
                std::array::from_fn(|i| source.effective_range(i))
            },
            slots.iter().any(|s| s.has_scene),
            None,
        );
        if planar && self.guarantee_aa_for_inputs(&slots)? == 0 {
            out.passthrough_slot = Some(0);
        }
        let uniform = out.passthrough_slot.is_none()
            && !out.has_scene
            && out.constants.iter().all(Option::is_some);
        if uniform {
            out.passthrough_slot = slots
                .iter()
                .position(|v| (0..4).all(|i| v.constants[i] == out.constants[i]));
        }
        if let Some(i) = out.passthrough_slot {
            out.constants = slots[i].constants;
            out.in_range = std::array::from_fn(|ch| slots[i].effective_range(ch));
        }
        Ok((out, uniform))
    }
    /// Compute `a7/a6/ab/aa` and the `zl.k` constant-input pass-through
    /// decision for one actual node. Caller supplies the already-prepared input
    /// and must not treat this as full graph preparation: `zl.k` state/events,
    /// parent-dependent adjustment bindings and cross-node scheduling are not
    /// implemented here. Unsupported node kinds fail explicitly.
    pub fn calculate_image_guarantees(
        &self,
        input: Option<PointwiseChannelGuarantees>,
    ) -> Result<PointwiseChannelGuarantees, PointwiseError<E>> {
        self.calculate_image_guarantees_with_uniformity(input)
            .map(|(channels, _)| channels)
    }

    pub(super) fn calculate_image_guarantees_with_uniformity(
        &self,
        input: Option<PointwiseChannelGuarantees>,
    ) -> Result<(PointwiseChannelGuarantees, bool), PointwiseError<E>> {
        let input = input.unwrap_or_else(PointwiseChannelGuarantees::missing);
        let mut result = PointwiseChannelGuarantees {
            constants: [None; 4],
            in_range: [false; 4],
            has_scene: input.has_scene,
            passthrough_slot: None,
        };
        match &self.core().payload {
            Payload::Input { .. } => {
                result.has_scene = false;
            }
            Payload::ImageGenerator { .. } if self.is_guaranteed_scalar_source() => {
                // These generators expose varying normalized red and zero on the other channels.
                for channel in 0..4 {
                    if self.guarantee_a7(channel, &input)? {
                        result.constants[channel] = Some(self.guarantee_a6(channel, &input)?);
                    }
                    result.in_range[channel] = self.guarantee_ab(channel, &input)?;
                }
                result.has_scene = false;
            }
            Payload::ImageGenerator { .. } if self.imported_image_handle().is_some() => {
                // y2.a7 and the inherited zl.ab both return false. An
                // Imported Image becomes a scene only at its consumer.
                result.has_scene = false;
            }
            Payload::Filter {
                kind: PointwiseKind::ChangeBci,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                change_bci_guarantees(&filter, input, &mut result)?;
            }
            Payload::Filter {
                kind: PointwiseKind::ChangeHsb,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                result.constants[3] = input.constants[3];
                let properties = filter.parameters();
                let mut animated = false;
                for property in &properties {
                    if super::super::parameter_ops::is_animated(|| property.retained_animation())?
                        || property.has_animated_binding()?
                    {
                        animated = true;
                        break;
                    }
                }
                if !animated && properties.iter().all(|property| property.number() == 0.0) {
                    result.passthrough_slot = Some(0);
                }
            }
            Payload::Filter {
                kind: PointwiseKind::Colorize,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                colorize_guarantees(&filter, input, &mut result)?;
            }
            Payload::Filter {
                kind: PointwiseKind::ColorizeG2,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
                result = super::super::colorize_g2::guarantees(&filter, input)?;
            }
            Payload::Filter {
                kind: PointwiseKind::Levels | PointwiseKind::LegacyBricksBoards,
                ..
            } => {
                // These nodes expose neither constants nor a known channel range.
            }
            Payload::Filter {
                kind: PointwiseKind::GaussianBlur,
                ..
            } => {
                let radius = self
                    .filter()
                    .ok_or(PointwiseError::InvalidCast)?
                    .parameter(1)?;
                result.constants = input.constants;
                result.in_range = std::array::from_fn(|channel| input.effective_range(channel));
                if !property_is_animated(&radius)? && radius.number() == 0. {
                    result.passthrough_slot = Some(0);
                }
            }
            Payload::Filter {
                kind: PointwiseKind::ToneCurve,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                result = super::super::tone_curve::guarantees(&filter, input)?;
            }
            Payload::Filter {
                kind: PointwiseKind::ToneCurveG2,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                result = super::super::tone_curve::legacy_guarantees(&filter, input)?;
            }
            Payload::Filter {
                kind:
                    kind @ (PointwiseKind::ClearMaps
                    | PointwiseKind::StoreMap
                    | PointwiseKind::MergeMaps
                    | PointwiseKind::FixMapContrast
                    | PointwiseKind::LayerMaps),
                ..
            } => {
                result.constants = input.constants;
                result.in_range = std::array::from_fn(|channel| input.effective_range(channel));
                // The source de() flag also keeps named-map workers active
                // when the ordinary channels are all uniform.
                result.has_scene = match kind {
                    PointwiseKind::ClearMaps => {
                        let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                        use super::super::effect_maps::choice_is_at;
                        choice_is_at(&filter, 0, None, "Keep Selection")?
                            && choice_is_at(&filter, 1, None, "None")?
                    }
                    PointwiseKind::FixMapContrast => input.has_scene(),
                    _ => true,
                };
            }
            Payload::Filter {
                kind: PointwiseKind::GetChannel,
                ..
            } => {
                let selection = self
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(0)?;
                result.constants[1..].fill(Some(0.0));
                if !super::super::parameter_ops::is_animated(|| selection.retained_animation())?
                    && !selection.has_animated_binding()?
                {
                    let index =
                        usize::try_from(selection.index()).map_err(|_| PointwiseError::Index)?;
                    result.constants[0] = input.constants.get(index).copied().flatten();
                    if selection
                        .text()?
                        .is_some_and(|text| text.into_iter().eq("Red".encode_utf16()))
                        && input.constants[1..].iter().all(|value| *value == Some(0.0))
                    {
                        result.passthrough_slot = Some(0);
                    }
                }
            }
            Payload::Filter {
                kind:
                    PointwiseKind::WizardColor
                    | PointwiseKind::WizardFiveColor
                    | PointwiseKind::WizardSlideColor
                    | PointwiseKind::WizardToneCurve,
                ..
            } => {
                if self.guarantee_aa()? == 0 {
                    result.passthrough_slot = Some(0);
                } else {
                    for i in 0..4 {
                        if self.guarantee_a7(i, &input)? {
                            result.constants[i] = Some(self.guarantee_a6(i, &input)?);
                        }
                        result.in_range[i] = self.guarantee_ab(i, &input)?;
                    }
                }
            }
            Payload::Filter {
                kind:
                    PointwiseKind::DiffuseEnvironment
                    | PointwiseKind::ScatterEnvironment
                    | PointwiseKind::RotateEnvironment
                    | PointwiseKind::Stereogram,
                ..
            } => {
                if self.kind() == Some(PointwiseKind::RotateEnvironment)
                    && self.guarantee_aa()? == 0
                {
                    result.passthrough_slot = Some(0);
                }
                result.constants = input.constants;
                result.in_range = std::array::from_fn(|i| input.effective_range(i));
            }
            Payload::Filter {
                kind:
                    PointwiseKind::Matrix
                    | PointwiseKind::WizardMatrix
                    | PointwiseKind::Kaleidoscope
                    | PointwiseKind::WizardKaleidoscope
                    | PointwiseKind::MotionBlur
                    | PointwiseKind::ShapeBlur
                    | PointwiseKind::VariableBlur,
                ..
            } => {
                // These workers carry every constant Original channel through.
                result.constants = input.constants;
            }
            Payload::Filter {
                kind: PointwiseKind::FreeTransform,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                if filter.transform_identity_input()? == 0 {
                    result.passthrough_slot = Some(0);
                } else {
                    // m.a7 guarantees only zero RGB outside the transformed image.
                    for channel in 0..3 {
                        if input.constants[channel] == Some(0.) {
                            result.constants[channel] = Some(0.);
                        }
                    }
                    result.in_range = std::array::from_fn(|channel| input.effective_range(channel));
                }
            }
            Payload::Filter {
                kind: PointwiseKind::Remap,
                ..
            } => return Ok(remap_guarantees(input, input.has_scene)),
            Payload::Filter {
                kind: PointwiseKind::Exposure,
                ..
            } => { /* pe.a7 and pe.ab always return false. */ }
            Payload::Filter {
                kind: PointwiseKind::Selector,
                ..
            } => {
                for i in 0..4 {
                    if self.guarantee_a7(i, &input)? {
                        result.constants[i] = Some(0.0);
                    }
                }
            }
            Payload::Filter {
                kind:
                    PointwiseKind::Fractalize
                    | PointwiseKind::Frequency
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::Rotate
                    | PointwiseKind::RotateTile
                    | PointwiseKind::Sharpen,
                ..
            } => {
                if self.guarantee_aa_for_inputs(&[input])? == 0 {
                    result.passthrough_slot = Some(0);
                } else {
                    for i in 0..4 {
                        if self.guarantee_a7(i, &input)? {
                            result.constants[i] = Some(self.guarantee_a6(i, &input)?);
                        }
                        result.in_range[i] = self.guarantee_ab(i, &input)?;
                    }
                }
            }
            Payload::Filter {
                kind: PointwiseKind::Atmosphere,
                ..
            } => {
                // ath.a7 examines only the input density's red guarantee.
                if input.constants[0] == Some(0.0) {
                    result.constants = [Some(0.0), Some(0.0), Some(0.0), Some(1.0)];
                }
            }
            Payload::Filter {
                kind:
                    PointwiseKind::ConvertEnvironment
                    | PointwiseKind::Canvas
                    | PointwiseKind::Shader
                    | PointwiseKind::Style
                    | PointwiseKind::Synthesis,
                ..
            } => {
                // ady.a7/ab always return false, even for equal formats.
            }
            Payload::Filter {
                kind: PointwiseKind::MakeEnvironment,
                ..
            } => {
                // bd.a7 and the inherited zl.ab make no channel guarantees.
                result.has_scene = false;
            }
            Payload::Filter {
                kind: PointwiseKind::Caustics,
                ..
            } => {
                result.constants[1..].fill(Some(0.0));
            }
            Payload::Filter {
                kind: PointwiseKind::Polygon,
                ..
            } => {
                // ah8.a7: channel zero varies; other channels are zero.
                result.constants[1..].fill(Some(0.0));
                result.in_range.fill(true);
                result.has_scene = false;
            }
            Payload::Filter {
                kind:
                    PointwiseKind::PointLight
                    | PointwiseKind::IlluminateG2
                    | PointwiseKind::Shadow
                    | PointwiseKind::ShadowG2,
                ..
            } => {
                for channel in 0..4 {
                    if self.guarantee_a7(channel, &input)? {
                        result.constants[channel] = Some(self.guarantee_a6(channel, &input)?);
                    }
                    result.in_range[channel] = self.guarantee_ab(channel, &input)?;
                }
            }
            Payload::Solid { model, .. } => {
                result.has_scene = false;
                let color = model.color_property();
                let transparency = model.transparency_property();
                if !self.solid_property_is_animated(&color)? {
                    for i in 0..3 {
                        result.constants[i] = Some(color.snapshot().color[i] as f32);
                    }
                }
                if !self.solid_property_is_animated(&transparency)? {
                    result.constants[3] = Some(transparency.snapshot().current as f32);
                }
            }
            Payload::Invert { .. } => {
                self.ensure_invert_evaluation_supported(true)?;
                let mut choices = [None; 4];
                for (i, choice) in choices.iter_mut().enumerate() {
                    *choice = self.invert_guarantee_selection(i)?;
                    if let (Some(choice), Some(value)) = (*choice, input.constants[i]) {
                        result.constants[i] = Some(if choice == 0 { 1.0 - value } else { value });
                    }
                    result.in_range[i] = input.effective_range(i);
                }
                if choices == [Some(1); 4] {
                    result.passthrough_slot = Some(0);
                }
            }
            _ => {
                return Err(PointwiseError::UnsupportedCapability(
                    "node guarantee calculation",
                ));
            }
        }
        // zl.k first honors aa(); otherwise cm() can select the first input
        // whose four constants equal its own. Solid Color has no input slot.
        let uniform = result.passthrough_slot.is_none()
            && !result.has_scene
            && result.constants.iter().all(Option::is_some);
        if !matches!(&self.core().payload, Payload::Solid { .. })
            && uniform
            && (0..4).all(|i| result.constants[i] == input.constants[i])
        {
            result.passthrough_slot = Some(0);
        }
        if result.passthrough_slot == Some(0) {
            result.constants = input.constants;
            result.in_range = std::array::from_fn(|i| input.effective_range(i));
        }
        Ok((result, uniform))
    }
}

fn remap_guarantees(
    original: PointwiseChannelGuarantees,
    has_scene: bool,
) -> (PointwiseChannelGuarantees, bool) {
    // an7 forwards only Original's channel guarantees. A scene in any
    // physical slot suppresses the uniform-input shortcut in zl.k.
    let uniform = !has_scene && original.constants.iter().all(Option::is_some);
    (
        PointwiseChannelGuarantees::new(
            original.constants,
            std::array::from_fn(|channel| original.effective_range(channel)),
            has_scene,
            uniform.then_some(0),
        ),
        uniform,
    )
}

fn arithmetic_guarantees(
    kind: PointwiseKind,
    inputs: &[Option<PointwiseChannelGuarantees>],
) -> (PointwiseChannelGuarantees, bool) {
    use super::super::numeric::{add, approximately_one, mul};

    let input = |index: usize| inputs[index].unwrap_or_else(PointwiseChannelGuarantees::missing);
    let has_scene = inputs
        .iter()
        .flatten()
        .any(PointwiseChannelGuarantees::has_scene);
    let passthrough = match kind {
        // ajc.aa: either zero input can be skipped if it carries no scene.
        PointwiseKind::Add => (0..2).find(|&index| {
            let other = input(1 - index);
            !other.has_scene && other.constants == [Some(0.0); 4]
        }),
        // ub.aa: a zero channel in the kept slot requires no unit factor.
        PointwiseKind::Multiply => (0..2).find(|&index| {
            let kept = input(index);
            let other = input(1 - index);
            !other.has_scene
                && kept
                    .constants
                    .iter()
                    .zip(other.constants)
                    .all(|(&kept, factor)| {
                        kept == Some(0.0) || factor.is_some_and(approximately_one)
                    })
        }),
        PointwiseKind::FromHsb => None,
        _ => unreachable!("arithmetic guarantee node"),
    };
    if let Some(index) = passthrough {
        let kept = input(index);
        return (
            PointwiseChannelGuarantees::new(
                kept.constants,
                std::array::from_fn(|channel| kept.effective_range(channel)),
                has_scene,
                Some(index),
            ),
            false,
        );
    }

    let a = input(0).constants;
    let b = input(1).constants;
    let constants = match kind {
        PointwiseKind::Add => {
            std::array::from_fn(|channel| a[channel].zip(b[channel]).map(|(a, b)| add(a, b)))
        }
        PointwiseKind::Multiply => std::array::from_fn(|channel| match (a[channel], b[channel]) {
            (Some(a), Some(b)) => Some(mul(a, b)),
            (Some(0.0), _) | (_, Some(0.0)) => Some(0.0),
            _ => None,
        }),
        PointwiseKind::FromHsb => {
            let rgb = (input(2).constants[0] == Some(0.0)).then_some(0.0);
            [rgb, rgb, rgb, Some(0.0)]
        }
        _ => unreachable!("arithmetic guarantee node"),
    };
    // zl.k can link a uniform result back to an input with the same values.
    let uniform = !has_scene && constants.iter().all(Option::is_some);
    let uniform_source = uniform
        .then(|| (0..inputs.len()).find(|&index| input(index).constants == constants))
        .flatten();
    let ranges = uniform_source.map_or([false; 4], |index| {
        std::array::from_fn(|channel| input(index).effective_range(channel))
    });
    (
        PointwiseChannelGuarantees::new(constants, ranges, has_scene, uniform_source),
        uniform,
    )
}

fn colorize_guarantees<E: 'static>(
    filter: &crate::PointwiseFilter<E>,
    input: PointwiseChannelGuarantees,
    result: &mut PointwiseChannelGuarantees,
) -> Result<(), PointwiseError<E>> {
    let gradient = filter.parameter(0)?.gradient()?;
    let tables = gradient.render_channels()?;
    let is_in_range = |value: f64| value.is_nan() || (0.0..=1.0).contains(&value);
    result.in_range.fill(true);
    for index in 0..gradient.color_count()? {
        let color = gradient
            .color_point(index)?
            .ok_or(PointwiseError::NullReference)?
            .color()
            .argb();
        for (in_range, value) in result.in_range[..3].iter_mut().zip(&color[1..]) {
            *in_range &= is_in_range(*value);
        }
    }
    let mut keep_transparency = filter
        .parameter(1)?
        .text()?
        .is_some_and(|text| text.into_iter().eq("Yes".encode_utf16()));
    for index in 0..gradient.alpha_count()? {
        let opacity = gradient
            .alpha_point(index)?
            .ok_or(PointwiseError::NullReference)?
            .value();
        keep_transparency &= opacity == 1.0;
        result.in_range[3] &= is_in_range(opacity);
    }
    for (channel, table) in tables.iter().enumerate() {
        result.constants[channel] = if channel == 3 && keep_transparency {
            result.in_range[3] = input.effective_range(3);
            input.constants[3]
        } else if table.len() == 1 {
            Some(table[0] as f32)
        } else if let Some(value) = input.constants[0] {
            Some(
                super::super::render::colorize_table_sample(table, value)?
                    .ok_or(PointwiseError::Index)?,
            )
        } else {
            None
        };
    }
    Ok(())
}

fn change_bci_guarantees<E: 'static>(
    filter: &crate::PointwiseFilter<E>,
    input: PointwiseChannelGuarantees,
    result: &mut PointwiseChannelGuarantees,
) -> Result<(), PointwiseError<E>> {
    let auto = filter.parameter(0)?;
    let bright = filter.parameter(1)?;
    let contrast = filter.parameter(2)?;
    let intensity = filter.parameter(3)?;
    let mut animated = [false; 4];
    for (animated, property) in animated
        .iter_mut()
        .zip([&auto, &bright, &contrast, &intensity])
    {
        *animated = super::super::parameter_ops::is_animated(|| property.retained_animation())?
            || property.has_animated_binding()?;
    }
    let brightness = bright.number();
    let contrast = contrast.number();
    let intensity = intensity.number();
    let active = animated.into_iter().any(|active| active);
    if !active && auto.index() == 1 && brightness == 0. && contrast == 0. && intensity == 0. {
        result.passthrough_slot = Some(0);
    }
    result.in_range[..3]
        .fill(active || auto.index() == 0 || brightness != 0. || contrast != 0. || intensity != 0.);

    if !animated[1..].iter().any(|active| *active) {
        result.constants[3] = input.constants[3];
        let offset = (brightness / 100.) as f32;
        let contrast_factor = super::super::render::factor(contrast);
        let mid_gray = contrast == -100. && intensity == 0.;
        for channel in 0..3 {
            let white = input.effective_range(channel)
                && brightness >= 51.
                && contrast >= 2000.
                && intensity >= 0.;
            let black = input.constants[channel] == Some(0.)
                && ((brightness <= 0. && contrast >= 0.)
                    || offset + (offset - 0.5) * contrast_factor < 0.);
            result.constants[channel] = if mid_gray {
                Some(0.5)
            } else if white {
                Some(1.)
            } else {
                black.then_some(0.)
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointwiseFilter, PointwiseKind};

    #[test]
    fn colorize_guarantees_sample_constants_and_preserve_input_transparency() {
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Colorize).unwrap();
        let ramp = filter.parameter(0).unwrap().gradient().unwrap();
        ramp.clear_points().unwrap();
        ramp.add_normalized_color(0., 0., 0.25, 1.25).unwrap();
        ramp.add_normalized_color(1., 1., 0.25, 1.25).unwrap();
        let dynamic = PointwiseChannelGuarantees::new([None; 4], [true; 4], false, None);
        let guarantees = filter
            .node()
            .calculate_image_guarantees(Some(dynamic))
            .unwrap();
        assert_eq!(guarantees.constants(), [None, Some(0.25), Some(1.25), None]);
        assert_eq!(guarantees.in_range(), [true, true, false, true]);

        let input = crate::SharedImageMap::new(crate::ImageGeometry::new(3, 2));
        input.set_constant(0, 0.375).unwrap();
        input.set_constant(3, 0.625).unwrap();
        let uniform = PointwiseChannelGuarantees::new(
            [Some(0.375), Some(0.), Some(0.), Some(0.625)],
            [true; 4],
            false,
            None,
        );
        let stop = ramp.add_alpha(0, 1.).unwrap();
        for (opacity, transparency) in [(1., 0.625), (0.5, 0.5)] {
            stop.set_value(opacity);
            let guarantees = filter
                .node()
                .calculate_image_guarantees(Some(uniform))
                .unwrap();
            let rendered = filter
                .render_supplied(&[Some(input.clone())], 3, 2)
                .unwrap()
                .unwrap();
            for (channel, constant) in guarantees.constants().into_iter().enumerate() {
                assert_eq!(
                    constant.unwrap().to_bits(),
                    rendered
                        .channel(channel as i32, false)
                        .unwrap()
                        .read(0)
                        .unwrap()
                        .to_bits()
                );
            }
            assert_eq!(guarantees.constants()[3], Some(transparency));
        }
    }

    #[test]
    fn point_light_uses_child_uniformity_and_scalar_source_formula() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        let mut input = PointwiseChannelGuarantees::missing();
        input.constants[0] = None;
        let dynamic = node.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(dynamic.constants, [None, Some(0.0), Some(0.0), Some(0.0)]);
        node.filter()
            .unwrap()
            .parameter(1)
            .unwrap()
            .set_number(0.0)
            .unwrap();
        let flat = node.calculate_image_guarantees(Some(input)).unwrap();
        let light = (1.0_f64 / 3.0_f64.sqrt()) as f32;
        assert_eq!(
            flat.constants[0],
            Some(0.5 * (light * 0.5 + 0.5) + 0.5 * light)
        );
        assert_eq!(flat.in_range, [true; 4]);
    }

    #[test]
    fn shadow_guarantees_follow_height_and_source_inherited_range() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Shadow)
            .unwrap()
            .node();
        let mut input = PointwiseChannelGuarantees::missing();
        input.constants[0] = None;
        let varying = node.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(varying.constants, [None, Some(0.0), Some(0.0), Some(0.0)]);
        assert_eq!(varying.in_range, [false; 4]);
        let height = node.filter().unwrap().parameter(0).unwrap();
        height.set_number(0.0).unwrap();
        let flat = node.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(flat.constants, [Some(1.0), Some(0.0), Some(0.0), Some(0.0)]);
        height.set_axis_animated(0, true).unwrap();
        assert_eq!(
            node.calculate_image_guarantees(Some(input))
                .unwrap()
                .constants[0],
            None
        );
    }

    #[test]
    fn shadow_constant_identity_selects_child_and_effective_ranges() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Shadow)
            .unwrap()
            .node();
        let input = PointwiseChannelGuarantees::new(
            [Some(1.0), Some(0.0), Some(0.0), Some(0.0)],
            [false; 4],
            false,
            None,
        );
        let result = node.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(result.passthrough_slot, Some(0));
        assert_eq!(
            result.constants,
            [Some(1.0), Some(0.0), Some(0.0), Some(0.0)]
        );
        assert_eq!(result.in_range, [true; 4]);
    }

    #[test]
    fn solid_and_invert_feed_the_same_channel_preparation_type() {
        let solid = PointwiseNode::<()>::solid_color().unwrap();
        solid.set_solid_color([0.25, 0.5, 0.75]).unwrap();
        let source = solid.calculate_image_guarantees(None).unwrap();
        assert_eq!(
            source.constants,
            [Some(0.25), Some(0.5), Some(0.75), Some(0.0)]
        );
        let invert = PointwiseNode::<()>::invert();
        let inverted = invert.calculate_image_guarantees(Some(source)).unwrap();
        assert_eq!(
            inverted.constants,
            [Some(0.75), Some(0.5), Some(0.25), Some(0.0)]
        );
        for i in 0..4 {
            invert.invert_property(i).unwrap().set_index(1).unwrap();
        }
        assert_eq!(
            invert
                .calculate_image_guarantees(Some(source))
                .unwrap()
                .passthrough_slot,
            Some(0)
        );
    }

    #[test]
    fn gradient_prepares_its_channel_guarantees_for_downstream_invert() {
        let gradient = PointwiseNode::<()>::gradient_generator().unwrap();
        let invert = PointwiseNode::<()>::invert();
        invert.slot(0).unwrap().set_node(Some(&gradient)).unwrap();

        invert.prepare_image_guarantees(&mut |_| Ok(())).unwrap();

        let source = gradient.prepared_image_guarantees().unwrap();
        assert_eq!(source, gradient.calculate_image_guarantees(None).unwrap());
        assert_eq!(source.constants(), [None, Some(0.0), Some(0.0), Some(0.0)]);
        assert_eq!(source.in_range(), [true; 4]);
        assert_eq!(source.passthrough_slot(), None);
        assert!(!source.has_scene());
        let downstream = invert.prepared_image_guarantees().unwrap();
        assert_eq!(
            downstream.constants(),
            [None, Some(1.0), Some(1.0), Some(0.0)]
        );
    }

    #[test]
    fn point_light_without_a_binding_owner_uses_the_stored_value() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        node.filter()
            .unwrap()
            .parameter(3)
            .unwrap()
            .set_binding_index(0);
        assert!(node.calculate_image_guarantees(None).unwrap().constants()[0].is_some());
    }

    #[test]
    fn unimplemented_guarantees_remain_explicit() {
        let other = PointwiseFilter::<()>::new(PointwiseKind::SmartBlur)
            .unwrap()
            .node();
        assert!(matches!(
            other.calculate_image_guarantees(None),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
    }
}

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use crate::{PointwiseFilter, PointwiseKind};
    #[test]
    fn child_before_parent_preparation_lives_on_real_owners() {
        let child = PointwiseNode::<()>::solid_color().unwrap();
        child.set_solid_color([0.2, 0.3, 0.4]).unwrap();
        let light = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        light.slot(0).unwrap().set_node(Some(&child)).unwrap();
        light.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert_eq!(
            child.prepared_image_guarantees().unwrap().constants[0],
            Some(0.2)
        );
        assert!(light.prepared_image_guarantees().unwrap().constants[0].is_some());
        assert!(light.clone().prepared_image_guarantees().is_some());
    }
    #[test]
    fn shadow_preparation_uses_real_child_and_owner() {
        let child = PointwiseNode::<()>::solid_color().unwrap();
        let shadow = PointwiseFilter::<()>::new(PointwiseKind::Shadow)
            .unwrap()
            .node();
        shadow.slot(0).unwrap().set_node(Some(&child)).unwrap();
        shadow.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        let result = shadow.prepared_image_guarantees().unwrap();
        assert_eq!(
            result.constants,
            [Some(1.0), Some(0.0), Some(0.0), Some(0.0)]
        );
        assert_eq!(result.in_range, [false; 4]);
    }

    #[test]
    fn unsupported_child_does_not_publish_parent_result() {
        let root = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        let child = PointwiseFilter::<()>::new(PointwiseKind::GaussianBlur)
            .unwrap()
            .node();
        root.slot(0).unwrap().set_node(Some(&child)).unwrap();
        assert!(matches!(
            root.prepare_image_guarantees(&mut |_| Ok(())),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
        assert!(root.prepared_image_guarantees().is_none());
    }
}

use crate::ordinary_events::{DelegateCore, HandlerCore};
use std::sync::Arc;
type PreparationDelegate<E> = DelegateCore<PointwiseNode<E>, (), E>;

/// A retained event delegate for source `zl.h` or `zl.b5` changes.
pub struct PointwisePreparationHandler<E = ()>(Arc<HandlerCore<PointwiseNode<E>, (), E>>);
impl<E> Clone for PointwisePreparationHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> PointwisePreparationHandler<E> {
    pub fn new(f: impl Fn(&PointwiseNode<E>) -> Result<(), E> + Send + Sync + 'static) -> Self {
        Self(Arc::new(HandlerCore {
            callback: Box::new(move |node, ()| f(node)),
        }))
    }
}
pub(super) struct PreparationCore<E> {
    active: bool,
    notified_active: bool,
    optimized: bool,
    notified_optimized: bool,
    uniform: bool,
    notified_uniform: bool,
    scene: bool,
    pass: i32,
    guaranteed: Option<[bool; 4]>,
    values: Option<[f32; 4]>,
    ranges: Option<[bool; 4]>,
    uniform_changed: PreparationDelegate<E>,
    activity_changed: PreparationDelegate<E>,
}
impl<E> PreparationCore<E> {
    pub(super) fn new() -> Self {
        Self {
            active: true,
            notified_active: true,
            optimized: false,
            notified_optimized: false,
            uniform: false,
            notified_uniform: false,
            scene: false,
            pass: -1,
            guaranteed: None,
            values: None,
            ranges: None,
            uniform_changed: DelegateCore::Absent,
            activity_changed: DelegateCore::Absent,
        }
    }
    fn snapshot(&self) -> Option<PointwiseChannelGuarantees> {
        let b = self.guaranteed?;
        let values = self.values?;
        let ranges = self.ranges?;
        Some(PointwiseChannelGuarantees {
            constants: std::array::from_fn(|i| b[i].then_some(values[i])),
            in_range: ranges,
            has_scene: self.scene,
            passthrough_slot: usize::try_from(self.pass).ok(),
        })
    }
}
fn wizard_kind(kind: PointwiseKind) -> bool {
    matches!(
        kind,
        PointwiseKind::WizardColor
            | PointwiseKind::WizardFiveColor
            | PointwiseKind::WizardSlideColor
            | PointwiseKind::WizardToneCurve
    )
}
fn wizard_affects<E: 'static>(
    filter: &crate::pointwise::PointwiseFilter<E>,
    ix: usize,
) -> Result<bool, PointwiseError<E>> {
    let choice = filter.parameter(ix as i32 + 3)?;
    if property_is_animated(&choice)? {
        Ok(true)
    } else {
        Ok(choice.index() == 0)
    }
}

fn property_is_animated<E: 'static>(
    property: &crate::pointwise::PointwiseParameter<E>,
) -> Result<bool, PointwiseError<E>> {
    Ok(
        super::super::parameter_ops::is_animated(|| property.retained_animation())?
            || property.has_animated_binding()?,
    )
}

fn wizard_uniform<E: 'static>(
    filter: &crate::pointwise::PointwiseFilter<E>,
    ix: usize,
    input: &PointwiseChannelGuarantees,
) -> Result<bool, PointwiseError<E>> {
    let kind = filter.kind();
    if kind == PointwiseKind::WizardToneCurve {
        if !wizard_affects(filter, ix)? {
            return Ok(input.constants[ix].is_some());
        }
        let start = filter.parameter(1)?;
        if property_is_animated(&start)? {
            return Ok(false);
        }
        let end = filter.parameter(2)?;
        if property_is_animated(&end)? {
            return Ok(false);
        }
        return Ok(if start.number() == end.number() {
            true
        } else {
            start.number() == 0. && input.constants[ix] == Some(0.)
        });
    }
    if ix == 3 {
        return Ok(if kind == PointwiseKind::WizardFiveColor {
            true
        } else {
            input.constants[3].is_some()
        });
    }
    let zero_input = input.constants[0] == Some(0.);
    if kind == PointwiseKind::WizardFiveColor && zero_input {
        return Ok(true);
    }
    let indices: &[i32] = match kind {
        PointwiseKind::WizardColor => &[0, 1],
        PointwiseKind::WizardFiveColor => &[0, 1, 2, 3, 4],
        PointwiseKind::WizardSlideColor => &[0, 2, 1],
        _ => return Err(PointwiseError::InvalidCast),
    };
    for &index in indices {
        if property_is_animated(&filter.parameter(index)?)? {
            return Ok(false);
        }
    }
    if kind == PointwiseKind::WizardColor && zero_input {
        return Ok(true);
    }
    let first = filter.parameter(0)?.color()?.components()[ix];
    let count = if kind == PointwiseKind::WizardFiveColor {
        5
    } else if kind == PointwiseKind::WizardColor {
        2
    } else {
        3
    };
    for index in 1..count {
        if filter.parameter(index)?.color()?.components()[ix] != first {
            return Ok(false);
        }
    }
    Ok(true)
}
fn wizard_value<E: 'static>(
    filter: &crate::pointwise::PointwiseFilter<E>,
    ix: usize,
    input: &PointwiseChannelGuarantees,
) -> Result<f32, PointwiseError<E>> {
    if filter.kind() == PointwiseKind::WizardToneCurve {
        if !wizard_affects(filter, ix)? {
            return input.constants[ix].ok_or(PointwiseError::Argument);
        }
        let start = filter.parameter(1)?.number();
        return Ok(if start == filter.parameter(2)?.number() {
            start as f32
        } else {
            0.
        });
    }
    if ix == 3 {
        return if filter.kind() == PointwiseKind::WizardFiveColor {
            Ok(0.)
        } else {
            input.constants[3].ok_or(PointwiseError::Argument)
        };
    }
    Ok(filter.parameter(0)?.color()?.components()[ix] as f32)
}
pub(super) fn wizard_passthrough<E: 'static>(
    filter: &crate::pointwise::PointwiseFilter<E>,
) -> Result<i32, PointwiseError<E>> {
    if filter.kind() != PointwiseKind::WizardToneCurve {
        return Ok(-1);
    }
    let mut all_static = true;
    for ix in 3..7 {
        if property_is_animated(&filter.parameter(ix)?)? {
            all_static = false;
            break;
        }
    }
    if all_static {
        let mut all_no = true;
        for ix in 3..7 {
            if filter.parameter(ix)?.index() != 1 {
                all_no = false;
                break;
            }
        }
        if all_no {
            return Ok(0);
        }
    }
    for ix in 0..3 {
        if property_is_animated(&filter.parameter(ix)?)? {
            return Ok(-1);
        }
    }
    Ok(
        if filter.parameter(0)?.number() == 0.
            && filter.parameter(1)?.number() == 0.
            && filter.parameter(2)?.number() == 1.
        {
            0
        } else {
            -1
        },
    )
}

impl<E: 'static> PointwiseNode<E> {
    pub fn prepared_image_guarantees(&self) -> Option<PointwiseChannelGuarantees> {
        guard(&self.core().mode_events).preparation.snapshot()
    }
    pub fn add_preparation_uniform_handler(&self, h: &PointwisePreparationHandler<E>) {
        let core = self.core();
        let mut state = guard(&core.mode_events);
        state.preparation.uniform_changed = state
            .preparation
            .uniform_changed
            .combine(&DelegateCore::Single(h.0.clone()));
    }
    pub fn remove_preparation_uniform_handler(&self, h: &PointwisePreparationHandler<E>) {
        let core = self.core();
        let old = {
            let mut state = guard(&core.mode_events);
            let next = state
                .preparation
                .uniform_changed
                .remove(&DelegateCore::Single(h.0.clone()));
            std::mem::replace(&mut state.preparation.uniform_changed, next)
        };
        drop(old);
    }
    pub fn add_preparation_activity_handler(&self, h: &PointwisePreparationHandler<E>) {
        let core = self.core();
        let mut state = guard(&core.mode_events);
        state.preparation.activity_changed = state
            .preparation
            .activity_changed
            .combine(&DelegateCore::Single(h.0.clone()));
    }
    pub fn remove_preparation_activity_handler(&self, h: &PointwisePreparationHandler<E>) {
        let core = self.core();
        let old = {
            let mut state = guard(&core.mode_events);
            let next = state
                .preparation
                .activity_changed
                .remove(&DelegateCore::Single(h.0.clone()));
            std::mem::replace(&mut state.preparation.activity_changed, next)
        };
        drop(old);
    }
    fn guarantee_a7(
        &self,
        ix: usize,
        input: &PointwiseChannelGuarantees,
    ) -> Result<bool, PointwiseError<E>> {
        match &self.core().payload {
            // zj.a7: red is the only channel with a potentially varying value.
            Payload::ImageGenerator { .. } if self.is_guaranteed_scalar_source() => Ok(ix > 0),
            Payload::Input { .. }
            | Payload::Filter {
                kind: PointwiseKind::FullLight,
                ..
            } => Ok(false),
            Payload::Filter {
                kind: PointwiseKind::Exposure,
                ..
            } => Ok(false),
            Payload::Filter {
                kind: PointwiseKind::Selector,
                ..
            } => {
                if ix > 0 {
                    return Ok(true);
                }
                let tolerance = self
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(2)?;
                Ok(!animated(&tolerance)? && tolerance.number() == 0.0)
            }
            Payload::Filter {
                kind:
                    PointwiseKind::Fractalize
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::Rotate
                    | PointwiseKind::RotateTile
                    | PointwiseKind::Sharpen,
                ..
            } => Ok(input.constants[ix].is_some()),
            Payload::Filter {
                kind: PointwiseKind::Frequency,
                ..
            } => {
                let f = self.filter().ok_or(PointwiseError::NullReference)?;
                let frequency = f.parameter(0)?;
                let phase = f.parameter(1)?;
                if !property_is_animated(&frequency)?
                    && !property_is_animated(&phase)?
                    && frequency.number() == 0.0
                    && phase.number() == 0.0
                {
                    return Ok(true);
                }
                Ok(input.constants[ix] == Some(0.0))
            }
            Payload::Filter {
                kind: PointwiseKind::Atmosphere,
                ..
            } => Ok(input.constants[0] == Some(0.0)),
            Payload::Filter {
                kind: PointwiseKind::ConvertEnvironment,
                ..
            } => Ok(false),
            Payload::Filter {
                kind: PointwiseKind::Transmit,
                ..
            } => Ok(input.constants[ix] == Some(0.0)),
            Payload::Filter {
                kind: PointwiseKind::Caustics,
                ..
            } => Ok(ix > 0),
            Payload::Filter {
                kind:
                    PointwiseKind::WizardMatrix
                    | PointwiseKind::DiffuseEnvironment
                    | PointwiseKind::ScatterEnvironment
                    | PointwiseKind::RotateEnvironment
                    | PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
                    | PointwiseKind::Reflect
                    | PointwiseKind::Refract
                    | PointwiseKind::Stereogram,
                ..
            } => Ok(input.constants[ix].is_some()),
            Payload::Filter { kind, .. } if wizard_kind(*kind) => wizard_uniform(
                &self.filter().ok_or(PointwiseError::NullReference)?,
                ix,
                input,
            ),
            Payload::Solid { model, .. } => {
                let p = if ix < 3 {
                    model.color_property()
                } else {
                    model.transparency_property()
                };
                self.solid_property_is_animated(&p)
                    .map(|animated| !animated)
            }
            Payload::Filter {
                kind: kind @ (PointwiseKind::PointLight | PointwiseKind::IlluminateG2),
                ..
            } => {
                if ix > 0 {
                    return Ok(true);
                }
                let f = self.filter().ok_or(PointwiseError::NullReference)?;
                let (range, height) = if *kind == PointwiseKind::IlluminateG2 {
                    (2, 0)
                } else {
                    (3, 1)
                };
                if property_is_animated(&f.parameter(range)?)? {
                    return Ok(false);
                }
                let height = f.parameter(height)?;
                Ok(
                    if property_is_animated(&height)? || height.number() != 0.0 {
                        input.constants[0].is_some()
                    } else {
                        true
                    },
                )
            }
            Payload::Filter {
                kind: PointwiseKind::Shadow | PointwiseKind::ShadowG2,
                ..
            } => {
                if ix > 0 {
                    return Ok(true);
                }
                let height = self
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(0)?;
                Ok(
                    if property_is_animated(&height)? || height.number() != 0.0 {
                        input.constants[0].is_some()
                    } else {
                        true
                    },
                )
            }
            Payload::Invert { .. } => {
                if input.constants[ix].is_none() {
                    return Ok(false);
                }
                Ok(self.invert_guarantee_selection(ix)?.is_some())
            }
            _ => Err(PointwiseError::UnsupportedCapability("node guarantee a7")),
        }
    }
    fn guarantee_a6(
        &self,
        ix: usize,
        input: &PointwiseChannelGuarantees,
    ) -> Result<f32, PointwiseError<E>> {
        match &self.core().payload {
            Payload::ImageGenerator { .. } if self.is_guaranteed_scalar_source() => Ok(0.0),
            Payload::Filter {
                kind:
                    PointwiseKind::Exposure
                    | PointwiseKind::Fractalize
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::Rotate
                    | PointwiseKind::RotateTile
                    | PointwiseKind::Sharpen,
                ..
            } => input.constants[ix].ok_or(PointwiseError::Argument),
            Payload::Filter {
                kind: PointwiseKind::Selector,
                ..
            } => Ok(0.0),
            Payload::Filter {
                kind: PointwiseKind::Frequency,
                ..
            } => {
                let f = self.filter().ok_or(PointwiseError::NullReference)?;
                let frequency = f.parameter(0)?;
                let phase = f.parameter(1)?;
                if !property_is_animated(&frequency)?
                    && !property_is_animated(&phase)?
                    && frequency.number() == 0.0
                    && phase.number() == 0.0
                {
                    Ok(0.0)
                } else {
                    input.constants[ix].ok_or(PointwiseError::Argument)
                }
            }
            Payload::Filter {
                kind: PointwiseKind::Atmosphere,
                ..
            } => Ok(if ix == 3 { 1.0 } else { 0.0 }),
            Payload::Filter {
                kind: PointwiseKind::ConvertEnvironment,
                ..
            } => input.constants[ix].ok_or(PointwiseError::Argument),
            Payload::Input { .. }
            | Payload::Filter {
                kind: PointwiseKind::Caustics | PointwiseKind::Transmit | PointwiseKind::FullLight,
                ..
            } => Ok(0.0),
            Payload::Filter {
                kind:
                    PointwiseKind::WizardMatrix
                    | PointwiseKind::DiffuseEnvironment
                    | PointwiseKind::ScatterEnvironment
                    | PointwiseKind::RotateEnvironment
                    | PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
                    | PointwiseKind::Reflect
                    | PointwiseKind::Refract
                    | PointwiseKind::Stereogram,
                ..
            } => input.constants[ix].ok_or(PointwiseError::Argument),
            Payload::Filter { kind, .. } if wizard_kind(*kind) => wizard_value(
                &self.filter().ok_or(PointwiseError::NullReference)?,
                ix,
                input,
            ),
            Payload::Solid { model, .. } => Ok(if ix < 3 {
                model.color_property().snapshot().color[ix] as f32
            } else {
                model.transparency_property().snapshot().current as f32
            }),
            Payload::Filter {
                kind: kind @ (PointwiseKind::PointLight | PointwiseKind::IlluminateG2),
                ..
            } => {
                if ix > 0 {
                    return Ok(0.0);
                }
                let f = self.filter().ok_or(PointwiseError::NullReference)?;
                let legacy = *kind == PointwiseKind::IlluminateG2;
                let o = f
                    .parameter(i32::from(legacy))?
                    .custom_object()?
                    .ok_or(PointwiseError::NullReference)?;
                if !o.is_light_position() {
                    return Err(PointwiseError::InvalidCast);
                }
                let x = -o.light_position_x()?;
                let y = o.light_position_y()?;
                let z = -1.0 / (x * x + y * y + 1.0).sqrt();
                let light = -z as f32;
                if legacy {
                    let direct = super::super::point_light::max_clr(0.0, -z) as f32;
                    return Ok(if f.parameter(2)?.index() == 0 {
                        direct
                    } else {
                        direct * 0.5 + 0.5
                    });
                }
                let gi = (f.parameter(3)?.number() / 100.0) as f32;
                let bright = if light > 0.0 { light } else { 0.0 };
                Ok(gi * (light * 0.5 + 0.5) + (1.0 - gi) * bright)
            }
            Payload::Filter {
                kind: PointwiseKind::Shadow | PointwiseKind::ShadowG2,
                ..
            } => Ok(if ix == 0 { 1.0 } else { 0.0 }),
            Payload::Invert { .. } => {
                let choice = self
                    .invert_guarantee_selection(ix)?
                    .ok_or(PointwiseError::Argument)?;
                let v = input.constants[ix].ok_or(PointwiseError::Argument)?;
                Ok(if choice == 0 { 1.0 - v } else { v })
            }
            _ => Err(PointwiseError::UnsupportedCapability("node guarantee a6")),
        }
    }
    fn guarantee_ab(
        &self,
        ix: usize,
        input: &PointwiseChannelGuarantees,
    ) -> Result<bool, PointwiseError<E>> {
        match &self.core().payload {
            // zj.ab declares the range even when channel zero varies.
            Payload::ImageGenerator { .. } if self.is_guaranteed_scalar_source() => Ok(true),
            Payload::Filter {
                kind: PointwiseKind::IlluminateG2,
                ..
            } => Ok(true),
            Payload::Filter { kind, .. } if wizard_kind(*kind) => {
                if *kind == PointwiseKind::WizardToneCurve {
                    wizard_affects(&self.filter().ok_or(PointwiseError::NullReference)?, ix)
                } else {
                    Ok(false)
                }
            }
            Payload::Input { .. }
            | Payload::Solid { .. }
            | Payload::Filter {
                kind:
                    PointwiseKind::Shadow
                    | PointwiseKind::ShadowG2
                    | PointwiseKind::Exposure
                    | PointwiseKind::Selector
                    | PointwiseKind::Atmosphere
                    | PointwiseKind::ConvertEnvironment
                    | PointwiseKind::WizardMatrix
                    | PointwiseKind::Caustics
                    | PointwiseKind::Transmit
                    | PointwiseKind::FullLight
                    | PointwiseKind::Reflect
                    | PointwiseKind::Refract,
                ..
            } => Ok(false),
            // These transforms inherit zl.ab until they forward a slot.
            Payload::Filter {
                kind: PointwiseKind::Rotate | PointwiseKind::RotateTile | PointwiseKind::Sharpen,
                ..
            } => Ok(false),
            Payload::Filter {
                kind: PointwiseKind::PointLight | PointwiseKind::Frequency,
                ..
            } => Ok(true),
            Payload::Filter {
                kind:
                    PointwiseKind::DiffuseEnvironment
                    | PointwiseKind::ScatterEnvironment
                    | PointwiseKind::RotateEnvironment
                    | PointwiseKind::Fractalize
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
                    | PointwiseKind::Stereogram,
                ..
            }
            | Payload::Invert { .. } => Ok(input.effective_range(ix)),
            _ => Err(PointwiseError::UnsupportedCapability("node guarantee ab")),
        }
    }
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // Source rejects NaN elevation.
    fn guarantee_aa_for_inputs(
        &self,
        inputs: &[PointwiseChannelGuarantees],
    ) -> Result<i32, PointwiseError<E>> {
        let kind = self.kind();
        if kind == Some(PointwiseKind::Frequency) {
            let f = self.filter().ok_or(PointwiseError::NullReference)?;
            let modifier = f.parameter(2)?;
            let phase = f.parameter(1)?;
            let frequency = f.parameter(0)?;
            if property_is_animated(&modifier)?
                || property_is_animated(&phase)?
                || property_is_animated(&frequency)?
            {
                return Ok(-1);
            }
            let input = inputs
                .first()
                .copied()
                .unwrap_or_else(PointwiseChannelGuarantees::missing);
            if !(0..4).all(|i| input.effective_range(i)) || phase.number() % 1.0 != 0.0 {
                return Ok(-1);
            }
            let text = modifier.text()?;
            let none: Vec<u16> = "None".encode_utf16().collect();
            let saw: Vec<u16> = "Saw Tooth".encode_utf16().collect();
            return Ok(
                if (frequency.number() == 1.0 && text.as_ref() == Some(&none))
                    || (frequency.number() == 0.5 && text.as_ref() == Some(&saw))
                {
                    0
                } else {
                    -1
                },
            );
        }
        if !matches!(kind, Some(PointwiseKind::Reflect | PointwiseKind::Refract)) {
            return self.guarantee_aa();
        }
        let f = self.filter().ok_or(PointwiseError::NullReference)?;
        if kind == Some(PointwiseKind::Reflect) {
            let elevation = f.parameter(0)?;
            if property_is_animated(&elevation)? || !(elevation.number() >= 0.0) {
                return Ok(-1);
            }
        }
        let height = f.parameter(if kind == Some(PointwiseKind::Reflect) {
            1
        } else {
            2
        })?;
        let child = inputs
            .get(1)
            .copied()
            .unwrap_or_else(PointwiseChannelGuarantees::missing);
        if (property_is_animated(&height)? || height.number() != 0.0)
            && child.constants[0].is_none()
        {
            return Ok(-1);
        }
        if child.has_scene {
            return Ok(-1);
        }
        Ok(0)
    }
    fn guarantee_aa(&self) -> Result<i32, PointwiseError<E>> {
        match &self.core().payload {
            Payload::ImageGenerator { .. } if self.is_guaranteed_scalar_source() => Ok(-1),
            Payload::Filter {
                kind: PointwiseKind::MoveRepeat,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                let size = filter.parameter(0)?;
                let location = filter.parameter(1)?;
                if property_is_animated(&size)?
                    || property_is_animated(&location)?
                    || size.normalized_pair()? != [1., 1.]
                {
                    return Ok(-1);
                }
                let periodic_identity = location
                    .pair()?
                    .into_iter()
                    .all(|value| f64::from(crate::framework_cast::integer(value)) == value);
                Ok(if periodic_identity { 0 } else { -1 })
            }
            Payload::Filter {
                kind: PointwiseKind::Rotate,
                ..
            } => {
                let rotation = self
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(0)?;
                let identity = !property_is_animated(&rotation)?
                    && rotation
                        .text()?
                        .is_some_and(|text| text.into_iter().eq("None".encode_utf16()));
                Ok(if identity { 0 } else { -1 })
            }
            Payload::Filter {
                kind: PointwiseKind::RotateTile,
                ..
            } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                let size = filter.parameter(0)?;
                let amount = filter.parameter(1)?;
                let identity = !property_is_animated(&size)?
                    && size.normalized_pair()? == [1., 1.]
                    && !property_is_animated(&amount)?
                    && amount.number() == 0.;
                Ok(if identity { 0 } else { -1 })
            }
            Payload::Filter {
                kind: PointwiseKind::Sharpen,
                ..
            } => {
                let radius = self
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(1)?;
                Ok(
                    if !property_is_animated(&radius)? && radius.number() == 0. {
                        0
                    } else {
                        -1
                    },
                )
            }
            Payload::Filter {
                kind: PointwiseKind::Fractalize,
                ..
            } => {
                let iterations = self
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(0)?;
                Ok(
                    if !property_is_animated(&iterations)? && iterations.number() == 0.0 {
                        0
                    } else {
                        -1
                    },
                )
            }
            // Frequency's passthrough also consults its already-prepared input.
            Payload::Filter {
                kind: PointwiseKind::Frequency,
                ..
            } => Ok(-1),
            Payload::Filter {
                kind: PointwiseKind::RotateEnvironment,
                ..
            } => {
                let f = self.filter().ok_or(PointwiseError::NullReference)?;
                for i in 0..3 {
                    let p = f.parameter(i)?;
                    if property_is_animated(&p)? || p.number() % 360.0 != 0.0 {
                        return Ok(-1);
                    }
                }
                Ok(0)
            }
            Payload::Filter { kind, .. } if wizard_kind(*kind) => {
                wizard_passthrough(&self.filter().ok_or(PointwiseError::NullReference)?)
            }
            Payload::Input { .. }
            | Payload::Solid { .. }
            | Payload::Filter {
                kind:
                    PointwiseKind::PointLight
                    | PointwiseKind::IlluminateG2
                    | PointwiseKind::Shadow
                    | PointwiseKind::ShadowG2
                    | PointwiseKind::Exposure
                    | PointwiseKind::Selector
                    | PointwiseKind::Atmosphere
                    | PointwiseKind::ConvertEnvironment
                    | PointwiseKind::WizardMatrix
                    | PointwiseKind::DiffuseEnvironment
                    | PointwiseKind::ScatterEnvironment
                    | PointwiseKind::ReflectEnvironment
                    | PointwiseKind::RefractEnvironment
                    | PointwiseKind::Stereogram
                    | PointwiseKind::Caustics
                    | PointwiseKind::Transmit
                    | PointwiseKind::FullLight,
                ..
            } => Ok(-1),
            Payload::Invert { .. } => {
                self.ensure_invert_evaluation_supported(true)?;
                for i in 0..4 {
                    if self.invert_guarantee_selection(i)? != Some(1) {
                        return Ok(-1);
                    }
                }
                Ok(0)
            }
            _ => Err(PointwiseError::UnsupportedCapability("node guarantee aa")),
        }
    }
}

impl<E: 'static> PointwiseNode<E> {
    /// Source `zl.b2` preparation for supported detached image-only trees.
    /// `logic` must address the actual node's document/TextureNodeLogic (or be
    /// deliberately supplied by a caller that knows the node is detached).
    /// Clone and group contexts, all other concrete nodes and cancellation are
    /// explicit errors. No graph renderer has yet been joined to this method.
    pub fn prepare_image_guarantees(
        &self,
        logic: &mut impl FnMut(&PointwiseNode<E>) -> Result<(), PointwiseError<E>>,
    ) -> Result<(), PointwiseError<E>> {
        if self.parent().is_some() {
            return Err(PointwiseError::UnsupportedCapability(
                "attached root guarantee preparation context",
            ));
        }
        enum Step<E> {
            Enter(PointwiseNode<E>),
            Visit(
                PointwiseNode<E>,
                usize,
                super::PointwiseSlot<E>,
                Option<PointwiseNode<E>>,
            ),
            Leave(
                PointwiseNode<E>,
                Vec<(super::PointwiseSlot<E>, Option<PointwiseNode<E>>)>,
            ),
        }
        let mut active = BTreeSet::new();
        let mut stack = vec![Step::Enter(self.clone())];
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(node) => {
                    if !active.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic source guarantee preparation",
                        ));
                    }
                    {
                        let core = node.core();
                        let mut state = guard(&core.mode_events);
                        state.preparation.active = true;
                        state.preparation.optimized = false;
                        state.preparation.uniform = false;
                    }
                    if node.is_clone_mode() || !node.contents().is_empty() {
                        return Err(PointwiseError::UnsupportedCapability(
                            "clone or Contents guarantee preparation",
                        ));
                    }
                    let slots = node.slots();
                    let eligible = match &node.core().payload {
                        Payload::Input { .. } | Payload::Solid { .. } => slots.is_empty(),
                        Payload::ImageGenerator { .. } => {
                            slots.is_empty() && node.is_guaranteed_scalar_source()
                        }
                        Payload::Invert { .. }
                        | Payload::Filter {
                            kind:
                                PointwiseKind::PointLight
                                | PointwiseKind::IlluminateG2
                                | PointwiseKind::Shadow
                                | PointwiseKind::ShadowG2
                                | PointwiseKind::Exposure
                                | PointwiseKind::Selector
                                | PointwiseKind::Fractalize
                                | PointwiseKind::Frequency
                                | PointwiseKind::MoveRepeat
                                | PointwiseKind::Rotate
                                | PointwiseKind::RotateTile
                                | PointwiseKind::Sharpen
                                | PointwiseKind::Atmosphere
                                | PointwiseKind::ConvertEnvironment
                                | PointwiseKind::WizardColor
                                | PointwiseKind::WizardFiveColor
                                | PointwiseKind::WizardSlideColor
                                | PointwiseKind::WizardToneCurve
                                | PointwiseKind::WizardMatrix
                                | PointwiseKind::DiffuseEnvironment
                                | PointwiseKind::ScatterEnvironment
                                | PointwiseKind::RotateEnvironment
                                | PointwiseKind::Caustics,
                            ..
                        } => slots.len() == 1,
                        Payload::Filter {
                            kind: PointwiseKind::FullLight,
                            ..
                        } => slots.len() == 4,
                        Payload::Filter {
                            kind:
                                PointwiseKind::Remap
                                | PointwiseKind::MotionBlur
                                | PointwiseKind::ShapeBlur
                                | PointwiseKind::Smear,
                            ..
                        } => slots.len() == 3,
                        Payload::Filter {
                            kind:
                                PointwiseKind::ReflectEnvironment
                                | PointwiseKind::RefractEnvironment
                                | PointwiseKind::Reflect
                                | PointwiseKind::Refract
                                | PointwiseKind::Stereogram
                                | PointwiseKind::Transmit
                                | PointwiseKind::VariableBlur
                                | PointwiseKind::Regions,
                            ..
                        } => slots.len() == 2,
                        _ => false,
                    };
                    if !eligible {
                        return Err(PointwiseError::UnsupportedCapability(
                            "node guarantee preparation",
                        ));
                    }
                    let captured: Vec<_> = slots
                        .into_iter()
                        .map(|slot| {
                            let child = slot.node();
                            (slot, child)
                        })
                        .collect();
                    for (_, child) in &captured {
                        if let Some(child_parent) = child.as_ref().and_then(PointwiseNode::parent)
                            && !child_parent.same_reference(&node)
                        {
                            return Err(PointwiseError::UnsupportedCapability(
                                "external child parent guarantee context",
                            ));
                        }
                    }
                    stack.push(Step::Leave(node.clone(), captured.clone()));
                    for (index, (slot, child)) in captured.into_iter().enumerate().rev() {
                        stack.push(Step::Visit(node.clone(), index, slot, child));
                    }
                }
                Step::Visit(parent, index, slot, child) => {
                    let current = parent.slots();
                    let stable_slot = current.get(index).is_some_and(|s| s.same_reference(&slot));
                    let same_child = match (slot.node(), child.as_ref()) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a.same_reference(b),
                        _ => false,
                    };
                    if !stable_slot || !same_child {
                        return Err(PointwiseError::UnsupportedCapability(
                            "guarantee slot changed during child preparation",
                        ));
                    }
                    if let Some(child) = child {
                        stack.push(Step::Enter(child));
                    }
                }
                Step::Leave(node, captured) => {
                    let slots = node.slots();
                    let unchanged = slots.len() == captured.len()
                        && slots.iter().zip(&captured).all(|(slot, (old, child))| {
                            slot.same_reference(old)
                                && match (slot.node(), child.as_ref()) {
                                    (None, None) => true,
                                    (Some(a), Some(b)) => a.same_reference(b),
                                    _ => false,
                                }
                        });
                    if !unchanged {
                        return Err(PointwiseError::UnsupportedCapability(
                            "guarantee slot changed during child preparation",
                        ));
                    }
                    let inputs = captured
                        .iter()
                        .map(|(_, child)| match child {
                            Some(child) => child.prepared_image_guarantees().ok_or(
                                PointwiseError::UnsupportedCapability("unprepared guarantee child"),
                            ),
                            None => Ok(PointwiseChannelGuarantees::missing()),
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let optics = matches!(
                        node.kind(),
                        Some(PointwiseKind::ReflectEnvironment | PointwiseKind::RefractEnvironment)
                    );
                    let input = inputs
                        .get(usize::from(optics))
                        .copied()
                        .unwrap_or_else(PointwiseChannelGuarantees::missing);
                    let regions = if node.kind() == Some(PointwiseKind::Regions) {
                        let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
                        Some(super::super::regions::guarantee_basis(
                            &filter,
                            [inputs[0], inputs[1]],
                        )?)
                    } else {
                        None
                    };
                    {
                        let core = node.core();
                        let mut s = guard(&core.mode_events);
                        s.preparation.scene = regions.map_or_else(
                            || inputs.iter().any(|input| input.has_scene),
                            |basis| basis.has_scene,
                        );
                    }
                    let pass = if regions.is_some() {
                        -1
                    } else {
                        node.guarantee_aa_for_inputs(&inputs)?
                    };
                    {
                        let core = node.core();
                        let mut s = guard(&core.mode_events);
                        let p = &mut s.preparation;
                        p.pass = pass;
                        if p.guaranteed.is_none() {
                            p.guaranteed = Some([false; 4]);
                            p.values = Some([0.0; 4]);
                            p.ranges = Some([false; 4]);
                        }
                    }
                    if pass < 0 {
                        for i in 0..4 {
                            let uniform = if let Some(basis) = regions {
                                basis.constants[i].is_some()
                            } else {
                                node.guarantee_a7(i, &input)?
                            };
                            {
                                let core = node.core();
                                let mut s = guard(&core.mode_events);
                                s.preparation.guaranteed.as_mut().unwrap()[i] = uniform;
                            }
                            if uniform {
                                let value = if let Some(basis) = regions {
                                    basis.constants[i].ok_or(PointwiseError::Argument)?
                                } else {
                                    node.guarantee_a6(i, &input)?
                                };
                                let core = node.core();
                                let mut s = guard(&core.mode_events);
                                s.preparation.values.as_mut().unwrap()[i] = value;
                            }
                            let range = if let Some(basis) = regions {
                                basis.in_range[i]
                            } else {
                                node.guarantee_ab(i, &input)?
                            };
                            let core = node.core();
                            let mut s = guard(&core.mode_events);
                            s.preparation.ranges.as_mut().unwrap()[i] = range;
                        }
                        let cm = {
                            let core = node.core();
                            let s = guard(&core.mode_events);
                            let p = &s.preparation;
                            !p.scene && p.guaranteed.unwrap().iter().all(|v| *v)
                        };
                        if cm {
                            let core = node.core();
                            let mut s = guard(&core.mode_events);
                            let p = &mut s.preparation;
                            p.optimized = true;
                            p.uniform = true;
                            for (slot_index, input) in inputs.iter().enumerate() {
                                if (0..4).all(|i| input.constants[i] == Some(p.values.unwrap()[i]))
                                {
                                    p.pass = slot_index as i32;
                                    break;
                                }
                            }
                        }
                    }
                    let pass = {
                        let core = node.core();
                        let s = guard(&core.mode_events);
                        s.preparation.pass
                    };
                    if pass >= 0 {
                        let input = inputs
                            .get(pass as usize)
                            .copied()
                            .unwrap_or_else(PointwiseChannelGuarantees::missing);
                        for i in 0..4 {
                            {
                                let core = node.core();
                                let mut s = guard(&core.mode_events);
                                s.preparation.guaranteed.as_mut().unwrap()[i] =
                                    input.constants[i].is_some();
                            }
                            if let Some(value) = input.constants[i] {
                                let core = node.core();
                                let mut s = guard(&core.mode_events);
                                s.preparation.values.as_mut().unwrap()[i] = value;
                            }
                            let core = node.core();
                            let mut s = guard(&core.mode_events);
                            s.preparation.ranges.as_mut().unwrap()[i] = input.effective_range(i);
                        }
                    }
                    let handlers = {
                        let core = node.core();
                        let s = guard(&core.mode_events);
                        let p = &s.preparation;
                        (p.notified_uniform != p.uniform).then(|| p.uniform_changed.clone())
                    };
                    if let Some(handlers) = handlers {
                        handlers
                            .invoke(&node, ())
                            .map_err(PointwiseError::Observer)?;
                        logic(&node)?;
                        let core = node.core();
                        let mut s = guard(&core.mode_events);
                        s.preparation.notified_uniform = s.preparation.uniform;
                    }
                    active.remove(&node.id);
                }
            }
        }
        // Source b5 separately visits children then commits bt/bx before m_o.
        let mut stack = vec![Step::Enter(self.clone())];
        let mut active = BTreeSet::new();
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(node) => {
                    if !active.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic source b5 notifications",
                        ));
                    }
                    let slots = node.slots();
                    let supported_two = matches!(
                        node.kind(),
                        Some(
                            PointwiseKind::ReflectEnvironment
                                | PointwiseKind::RefractEnvironment
                                | PointwiseKind::Reflect
                                | PointwiseKind::Refract
                                | PointwiseKind::Stereogram
                                | PointwiseKind::Transmit
                                | PointwiseKind::Regions
                        )
                    );
                    let maximum_slots = if node.kind() == Some(PointwiseKind::FullLight) {
                        4
                    } else if supported_two {
                        2
                    } else {
                        1
                    };
                    if !node.contents().is_empty() || slots.len() > maximum_slots {
                        return Err(PointwiseError::UnsupportedCapability(
                            "b5 topology changed outside supported context",
                        ));
                    }
                    let captured: Vec<_> = slots
                        .into_iter()
                        .map(|slot| {
                            let child = slot.node();
                            (slot, child)
                        })
                        .collect();
                    stack.push(Step::Leave(node.clone(), captured.clone()));
                    for (index, (slot, child)) in captured.into_iter().enumerate().rev() {
                        stack.push(Step::Visit(node.clone(), index, slot, child));
                    }
                }
                Step::Visit(parent, index, slot, child) => {
                    let current = parent.slots();
                    let stable_slot = current.get(index).is_some_and(|s| s.same_reference(&slot));
                    let same_child = match (slot.node(), child.as_ref()) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a.same_reference(b),
                        _ => false,
                    };
                    if !stable_slot || !same_child {
                        return Err(PointwiseError::UnsupportedCapability(
                            "b5 slot changed during child notification",
                        ));
                    }
                    if let Some(child) = child {
                        stack.push(Step::Enter(child));
                    }
                }
                Step::Leave(node, _) => {
                    let handler = {
                        let core = node.core();
                        let mut s = guard(&core.mode_events);
                        let p = &mut s.preparation;
                        if p.active != p.notified_active || p.optimized != p.notified_optimized {
                            p.notified_active = p.active;
                            p.notified_optimized = p.optimized;
                            Some(p.activity_changed.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(handler) = handler {
                        handler
                            .invoke(&node, ())
                            .map_err(PointwiseError::Observer)?;
                    }
                    active.remove(&node.id);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    use crate::{PointwiseFilter, PointwiseKind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn source_uniform_callback_failure_keeps_committed_channels_and_retries_before_b5() {
        let n = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        let calls = Arc::new(AtomicUsize::new(0));
        let activity = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        n.add_preparation_uniform_handler(&PointwisePreparationHandler::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let c = activity.clone();
        n.add_preparation_activity_handler(&PointwisePreparationHandler::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        assert!(matches!(
            n.prepare_image_guarantees(&mut |_| Err(PointwiseError::UnsupportedCapability(
                "logic unavailable"
            ))),
            Err(PointwiseError::UnsupportedCapability("logic unavailable"))
        ));
        assert!(n.prepared_image_guarantees().unwrap().constants[0].is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(activity.load(Ordering::SeqCst), 0);
        n.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(activity.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod reentry_tests {
    use super::*;
    use crate::{PointwiseFilter, PointwiseKind};
    #[test]
    fn child_callback_replacing_slot_retains_child_prefix_and_rejects_stale_parent() {
        let root = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        let child = PointwiseNode::<()>::solid_color().unwrap();
        let replacement = PointwiseNode::<()>::solid_color().unwrap();
        root.slot(0).unwrap().set_node(Some(&child)).unwrap();
        let callback_root = root.clone();
        let callback_new = replacement.clone();
        let handler = PointwisePreparationHandler::new(move |_| {
            callback_root
                .slot(0)
                .unwrap()
                .set_node(Some(&callback_new))
                .unwrap();
            Ok(())
        });
        child.add_preparation_uniform_handler(&handler);
        assert!(matches!(
            root.prepare_image_guarantees(&mut |_| Ok(())),
            Err(PointwiseError::UnsupportedCapability(
                "guarantee slot changed during child preparation"
            ))
        ));
        assert!(child.prepared_image_guarantees().unwrap().constants[0].is_some());
        assert!(root.prepared_image_guarantees().is_none());
        child.remove_preparation_uniform_handler(&handler);
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use crate::{PointwiseFilter, PointwiseKind};
    #[test]
    fn attached_subtree_is_rejected_before_preparation() {
        let parent = PointwiseNode::<()>::invert();
        let light = PointwiseFilter::<()>::new(PointwiseKind::PointLight)
            .unwrap()
            .node();
        parent.slot(0).unwrap().set_node(Some(&light)).unwrap();
        assert!(matches!(
            light.prepare_image_guarantees(&mut |_| Ok(())),
            Err(PointwiseError::UnsupportedCapability(
                "attached root guarantee preparation context"
            ))
        ));
        assert!(light.prepared_image_guarantees().is_none());
    }
}

#[cfg(test)]
mod wizard_guarantee_tests {
    use super::*;
    use crate::{ContextColorValue, PointwiseFilter};
    fn node(kind: PointwiseKind) -> PointwiseNode<()> {
        PointwiseFilter::new(kind).unwrap().node()
    }
    fn varying() -> PointwiseChannelGuarantees {
        PointwiseChannelGuarantees::new([None, None, None, Some(0.4)], [false; 4], false, None)
    }
    #[test]
    fn wizard_color_and_slide_uniformity_follow_actual_colors_and_alpha() {
        for kind in [PointwiseKind::WizardColor, PointwiseKind::WizardSlideColor] {
            let node = node(kind);
            let f = node.filter().unwrap();
            for p in f
                .parameters()
                .iter()
                .take(if kind == PointwiseKind::WizardColor {
                    2
                } else {
                    3
                })
            {
                p.set_color(ContextColorValue::unlinked([0.2, 0.4, 0.6]))
                    .unwrap();
            }
            let state = node.calculate_image_guarantees(Some(varying())).unwrap();
            assert_eq!(
                state.constants(),
                [Some(0.2), Some(0.4), Some(0.6), Some(0.4)]
            );
            f.parameter(1)
                .unwrap()
                .set_color(ContextColorValue::unlinked([0.2, 0.8, 0.6]))
                .unwrap();
            assert_eq!(
                node.calculate_image_guarantees(Some(varying()))
                    .unwrap()
                    .constants(),
                [Some(0.2), None, Some(0.6), Some(0.4)]
            );
        }
    }
    #[test]
    fn palette_guarantees_use_the_stored_color_without_a_binding_owner() {
        let five = node(PointwiseKind::WizardFiveColor);
        let baseline = five.calculate_image_guarantees(Some(varying())).unwrap();
        five.filter()
            .unwrap()
            .parameter(0)
            .unwrap()
            .set_binding_index(0);
        let zero =
            PointwiseChannelGuarantees::new([Some(0.), None, None, None], [false; 4], false, None);
        let state = five.calculate_image_guarantees(Some(zero)).unwrap();
        assert_eq!(state.constants(), [Some(0.), Some(0.), Some(0.), Some(0.)]);
        let two = node(PointwiseKind::WizardColor);
        let two_baseline = two.calculate_image_guarantees(Some(zero)).unwrap();
        two.filter()
            .unwrap()
            .parameter(0)
            .unwrap()
            .set_binding_index(0);
        assert_eq!(
            two.calculate_image_guarantees(Some(zero)).unwrap(),
            two_baseline
        );
        assert_eq!(
            five.calculate_image_guarantees(Some(varying())).unwrap(),
            baseline
        );
    }
    #[test]
    fn wizard_tone_guarantees_and_early_passthrough_match_source() {
        let tone = node(PointwiseKind::WizardToneCurve);
        let f = tone.filter().unwrap();
        let input = PointwiseChannelGuarantees::new(
            [Some(0.), None, Some(0.4), Some(0.3)],
            [true, false, true, true],
            false,
            None,
        );
        let state = tone.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(state.constants(), [Some(0.), None, None, Some(0.3)]);
        assert_eq!(state.in_range(), [true, true, true, false]);
        f.parameter(1).unwrap().set_number(0.4).unwrap();
        f.parameter(2).unwrap().set_number(0.4).unwrap();
        assert_eq!(
            tone.calculate_image_guarantees(Some(input))
                .unwrap()
                .constants(),
            [Some(0.4), Some(0.4), Some(0.4), Some(0.3)]
        );
        for ix in 3..7 {
            f.parameter(ix).unwrap().set_index(1).unwrap();
        }
        f.parameter(0).unwrap().set_binding_index(0);
        let state = tone.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(state.passthrough_slot(), Some(0));
        assert_eq!(state.constants(), input.constants());
        f.parameter(3).unwrap().set_index(0).unwrap();
        f.parameter(6).unwrap().set_binding_index(1);
        let state = tone.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(state.passthrough_slot(), None);
        assert_eq!(state.constants(), [Some(0.4), None, Some(0.4), Some(0.3)]);
    }
    #[test]
    fn matrix_guarantees_copy_constants_and_only_pass_uniform_inputs() {
        let matrix = node(PointwiseKind::WizardMatrix);
        let mixed = PointwiseChannelGuarantees::new(
            [Some(0.25), None, Some(2.0), None],
            [true; 4],
            false,
            None,
        );
        let state = matrix.calculate_image_guarantees(Some(mixed)).unwrap();
        assert_eq!(state.constants(), mixed.constants());
        assert_eq!(state.in_range(), [false; 4]);
        assert_eq!(state.passthrough_slot(), None);
        let uniform = PointwiseChannelGuarantees::new(
            [Some(0.25), Some(0.5), Some(2.0), Some(1.0)],
            [false; 4],
            false,
            None,
        );
        let state = matrix.calculate_image_guarantees(Some(uniform)).unwrap();
        assert_eq!(state.passthrough_slot(), Some(0));
        assert_eq!(state.in_range(), [true, true, false, true]);
        let scene = PointwiseChannelGuarantees::new(uniform.constants(), [false; 4], true, None);
        assert_eq!(
            matrix
                .calculate_image_guarantees(Some(scene))
                .unwrap()
                .passthrough_slot(),
            None
        );
        matrix.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert_eq!(
            matrix.prepared_image_guarantees().unwrap(),
            matrix.calculate_image_guarantees(None).unwrap()
        );
    }
    #[test]
    fn detached_preparation_publishes_wizard_results_on_actual_nodes() {
        for kind in [
            PointwiseKind::WizardColor,
            PointwiseKind::WizardFiveColor,
            PointwiseKind::WizardSlideColor,
            PointwiseKind::WizardToneCurve,
            PointwiseKind::WizardMatrix,
        ] {
            let child = PointwiseNode::<()>::solid_color().unwrap();
            child.set_solid_color([0.25, 0.5, 0.75]).unwrap();
            let parent = node(kind);
            parent.slot(0).unwrap().set_node(Some(&child)).unwrap();
            let expected = parent
                .calculate_image_guarantees(Some(child.calculate_image_guarantees(None).unwrap()))
                .unwrap();
            parent.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
            assert_eq!(parent.prepared_image_guarantees().unwrap(), expected);
            let rendered = parent.render_original(2, 2).unwrap();
            assert!(rendered.caught_errors().is_empty());
        }
    }
}

#[cfg(test)]
mod environment_scheduler_tests {
    use super::*;
    use crate::PointwiseFilter;
    use std::sync::{Arc, Mutex};
    fn solid(color: [f64; 3]) -> PointwiseNode<()> {
        let node = PointwiseNode::solid_color().unwrap();
        node.set_solid_color(color).unwrap();
        node
    }
    #[test]
    fn two_slot_preparation_follows_source_order_and_selects_matching_slot() {
        for kind in [
            PointwiseKind::ReflectEnvironment,
            PointwiseKind::RefractEnvironment,
        ] {
            let root = PointwiseFilter::<()>::new(kind).unwrap().node();
            let height = solid([0.1, 0.2, 0.3]);
            let env = solid([0.6, 0.7, 0.8]);
            root.slot(0).unwrap().set_node(Some(&height)).unwrap();
            root.slot(1).unwrap().set_node(Some(&env)).unwrap();
            let expected = root
                .calculate_image_guarantees_for_inputs(&[
                    Some(height.calculate_image_guarantees(None).unwrap()),
                    Some(env.calculate_image_guarantees(None).unwrap()),
                ])
                .unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            for (ix, node) in [(0, &height), (1, &env), (2, &root)] {
                let seen = seen.clone();
                node.add_preparation_activity_handler(&PointwisePreparationHandler::new(
                    move |_| {
                        seen.lock().unwrap().push(ix);
                        Ok(())
                    },
                ));
            }
            root.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
            assert_eq!(root.prepared_image_guarantees().unwrap(), expected);
            assert_eq!(expected.passthrough_slot(), Some(1));
            assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2]);
            let both = root
                .calculate_image_guarantees_for_inputs(&[Some(expected), Some(expected)])
                .unwrap();
            assert_eq!(both.passthrough_slot(), Some(0));
            let scene = PointwiseChannelGuarantees::new([Some(0.0); 4], [true; 4], true, None);
            assert_eq!(
                root.calculate_image_guarantees_for_inputs(&[Some(scene), Some(expected)])
                    .unwrap()
                    .passthrough_slot(),
                None
            );
        }
    }
    #[test]
    fn changed_later_slot_is_rejected_before_stale_sibling_is_prepared() {
        let root = PointwiseFilter::<()>::new(PointwiseKind::ReflectEnvironment)
            .unwrap()
            .node();
        let first = solid([0.1, 0.2, 0.3]);
        let old = solid([0.4, 0.5, 0.6]);
        let new = solid([0.7, 0.8, 0.9]);
        root.slot(0).unwrap().set_node(Some(&first)).unwrap();
        root.slot(1).unwrap().set_node(Some(&old)).unwrap();
        let slot = root.slot(1).unwrap();
        first.add_preparation_uniform_handler(&PointwisePreparationHandler::new(move |_| {
            slot.set_node(Some(&new)).unwrap();
            Ok(())
        }));
        assert!(matches!(
            root.prepare_image_guarantees(&mut |_| Ok(())),
            Err(PointwiseError::UnsupportedCapability(
                "guarantee slot changed during child preparation"
            ))
        ));
        assert!(old.prepared_image_guarantees().is_none());
    }
    #[test]
    fn rotation_passthrough_uses_stored_angles_without_a_binding_owner() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::RotateEnvironment).unwrap();
        let input = PointwiseChannelGuarantees::new([None; 4], [true; 4], false, None);
        assert_eq!(
            f.node()
                .calculate_image_guarantees(Some(input))
                .unwrap()
                .passthrough_slot(),
            Some(0)
        );
        f.parameter(0).unwrap().set_number(90.0).unwrap();
        f.parameter(1).unwrap().set_binding_index(0);
        assert_eq!(
            f.node()
                .calculate_image_guarantees(Some(input))
                .unwrap()
                .passthrough_slot(),
            None
        );
        f.parameter(0).unwrap().set_number(0.0).unwrap();
        assert_eq!(
            f.node()
                .calculate_image_guarantees(Some(input))
                .unwrap()
                .passthrough_slot(),
            Some(0)
        );
    }
}

#[cfg(test)]
mod atmosphere_conversion_guarantee_tests {
    use super::*;
    use crate::PointwiseFilter;

    fn node(kind: PointwiseKind) -> PointwiseNode<()> {
        PointwiseFilter::<()>::new(kind).unwrap().node()
    }
    #[test]
    fn atmosphere_only_promises_transparent_black_for_uniform_zero_density() {
        let node = node(PointwiseKind::Atmosphere);
        let zero =
            PointwiseChannelGuarantees::new([Some(0.0), None, None, None], [true; 4], false, None);
        let result = node.calculate_image_guarantees(Some(zero)).unwrap();
        assert_eq!(
            result.constants(),
            [Some(0.0), Some(0.0), Some(0.0), Some(1.0)]
        );
        assert_eq!(result.in_range(), [false; 4]);
        assert_eq!(result.passthrough_slot(), None);
        let same = PointwiseChannelGuarantees::new(
            [Some(-0.0), Some(0.0), Some(0.0), Some(1.0)],
            [false; 4],
            false,
            None,
        );
        let result = node.calculate_image_guarantees(Some(same)).unwrap();
        assert_eq!(result.passthrough_slot(), Some(0));
        assert_eq!(result.in_range(), [true; 4]);
        for density in [None, Some(0.25), Some(f32::NAN)] {
            let input = PointwiseChannelGuarantees::new(
                [density, Some(0.0), Some(0.0), Some(1.0)],
                [true; 4],
                false,
                None,
            );
            let result = node.calculate_image_guarantees(Some(input)).unwrap();
            assert_eq!(result.constants(), [None; 4]);
            assert_eq!(result.in_range(), [false; 4]);
            assert_eq!(result.passthrough_slot(), None);
        }
        let scene = PointwiseChannelGuarantees::new(same.constants(), [false; 4], true, None);
        let result = node.calculate_image_guarantees(Some(scene)).unwrap();
        assert!(result.has_scene());
        assert_eq!(result.passthrough_slot(), None);
    }
    #[test]
    fn conversion_never_claims_uniformity_or_equal_format_passthrough() {
        let node = node(PointwiseKind::ConvertEnvironment);
        let filter = node.filter().unwrap();
        filter.parameter(1).unwrap().set_index(0).unwrap();
        let uniform = PointwiseChannelGuarantees::new(
            [Some(0.2), Some(0.4), Some(0.6), Some(0.0)],
            [true; 4],
            false,
            None,
        );
        let result = node.calculate_image_guarantees(Some(uniform)).unwrap();
        assert_eq!(result.constants(), [None; 4]);
        assert_eq!(result.in_range(), [false; 4]);
        assert_eq!(result.passthrough_slot(), None);
    }
    #[test]
    fn detached_preparation_uses_actual_child_and_keeps_graph_renderable() {
        for kind in [PointwiseKind::Atmosphere, PointwiseKind::ConvertEnvironment] {
            let child = PointwiseNode::<()>::solid_color().unwrap();
            child.set_solid_color([0.0, 0.5, 0.75]).unwrap();
            let parent = node(kind);
            parent.slot(0).unwrap().set_node(Some(&child)).unwrap();
            let expected = parent
                .calculate_image_guarantees(Some(child.calculate_image_guarantees(None).unwrap()))
                .unwrap();
            parent.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
            assert_eq!(parent.prepared_image_guarantees().unwrap(), expected);
            let rendered = parent.render_original(4, 4).unwrap();
            assert!(rendered.caught_errors().is_empty());
        }
    }
}

#[cfg(test)]
mod nonlinear_guarantee_tests {
    use super::*;
    use crate::PointwiseFilter;
    fn node(kind: PointwiseKind) -> PointwiseNode<()> {
        PointwiseFilter::<()>::new(kind).unwrap().node()
    }
    fn varying(range: bool) -> PointwiseChannelGuarantees {
        PointwiseChannelGuarantees::new([None; 4], [range; 4], false, None)
    }
    #[test]
    fn frequency_short_circuits_in_source_order_and_respects_input_range() {
        let n = node(PointwiseKind::Frequency);
        let f = n.filter().unwrap();
        f.parameter(0).unwrap().set_number(1.0).unwrap();
        assert_eq!(
            n.calculate_image_guarantees(Some(varying(true)))
                .unwrap()
                .passthrough_slot(),
            Some(0)
        );
        let out = n.calculate_image_guarantees(Some(varying(false))).unwrap();
        assert_eq!(out.passthrough_slot(), None);
        assert_eq!(out.constants(), [None; 4]);
        assert_eq!(out.in_range(), [true; 4]);
        f.parameter(0).unwrap().set_number(0.5).unwrap();
        f.parameter(2).unwrap().set_index(1).unwrap();
        assert_eq!(
            n.calculate_image_guarantees(Some(varying(true)))
                .unwrap()
                .passthrough_slot(),
            Some(0)
        );
        f.parameter(0).unwrap().set_number(0.0).unwrap();
        f.parameter(2).unwrap().set_index(0).unwrap();
        let out = n.calculate_image_guarantees(Some(varying(false))).unwrap();
        assert_eq!(out.constants(), [Some(0.0); 4]);
        assert_eq!(out.in_range(), [true; 4]);
        f.parameter(2).unwrap().set_binding_index(0);
        assert_eq!(
            n.calculate_image_guarantees(Some(varying(true)))
                .unwrap()
                .constants(),
            [Some(0.0); 4]
        );
    }
    #[test]
    fn fractalize_selector_and_exposure_keep_their_distinct_source_rules() {
        let fractal = node(PointwiseKind::Fractalize);
        let f = fractal.filter().unwrap();
        f.parameter(0).unwrap().set_number(0.0).unwrap();
        assert_eq!(
            fractal
                .calculate_image_guarantees(Some(varying(false)))
                .unwrap()
                .passthrough_slot(),
            Some(0)
        );
        f.parameter(0).unwrap().set_number(2.0).unwrap();
        let input = PointwiseChannelGuarantees::new(
            [Some(0.5), None, Some(2.0), None],
            [false, true, false, true],
            false,
            None,
        );
        let out = fractal.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(out.constants(), input.constants());
        assert_eq!(out.in_range(), [true, true, false, true]);
        let selector = node(PointwiseKind::Selector);
        let f = selector.filter().unwrap();
        assert_eq!(
            selector
                .calculate_image_guarantees(Some(input))
                .unwrap()
                .constants(),
            [None, Some(0.0), Some(0.0), Some(0.0)]
        );
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        let out = selector.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(out.constants(), [Some(0.0); 4]);
        assert_eq!(out.in_range(), [false; 4]);
        let exposure = node(PointwiseKind::Exposure);
        let out = exposure.calculate_image_guarantees(Some(input)).unwrap();
        assert_eq!(out.constants(), [None; 4]);
        assert_eq!(out.in_range(), [false; 4]);
    }
    #[test]
    fn detached_nonlinear_preparation_matches_independent_guarantees() {
        for kind in [
            PointwiseKind::Frequency,
            PointwiseKind::Fractalize,
            PointwiseKind::Selector,
            PointwiseKind::Exposure,
        ] {
            let child = PointwiseNode::<()>::solid_color().unwrap();
            child.set_solid_color([0.2, 0.4, 0.6]).unwrap();
            let parent = node(kind);
            parent.slot(0).unwrap().set_node(Some(&child)).unwrap();
            let expected = parent
                .calculate_image_guarantees(Some(child.calculate_image_guarantees(None).unwrap()))
                .unwrap();
            parent.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
            assert_eq!(
                parent.prepared_image_guarantees().unwrap(),
                expected,
                "{}",
                kind.name()
            );
        }
    }
}
