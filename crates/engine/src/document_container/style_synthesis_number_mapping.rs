use super::*;
use crate::{
    BinaryCursor, BinaryWriter, CurveError, CurveRecord, PointwiseError, PropertyRecordError,
    SharedCurve,
};

/// A detached curve edit for a single Texture Synthesis field.
pub struct StyleSynthesisNumberMapping<E> {
    owner: PointwiseNode<E>,
    receiver: SynthesisNumberOwner<E>,
    synthesis: SynthesisEdit,
    target: SynthesisMappingField,
    control: NumericCatalogValue,
    initial: Vec<u8>,
    output: [f32; 2],
    curve: SharedCurve<E>,
}

#[derive(Clone, Copy)]
pub(super) enum SynthesisMappingField {
    Number(Option<usize>, SynthesisNumberField),
    Pair {
        layer: usize,
        field: SynthesisPairField,
        axis: usize,
    },
}

impl SynthesisMappingField {
    fn axis(self) -> usize {
        match self {
            Self::Number(..) => 0,
            Self::Pair { axis, .. } => axis,
        }
    }

    fn adjustment(self, edit: &SynthesisEdit) -> Option<&AdjustmentRecord> {
        match self {
            Self::Number(layer, field) => edit.number_adjustment(layer, field),
            Self::Pair { layer, field, .. } => edit.pair_adjustment(layer, field),
        }
    }
}

impl<E> StyleSynthesisNumberMapping<E> {
    pub fn curve(&self) -> &SharedCurve<E> {
        &self.curve
    }

    pub fn bounds(&self) -> [f32; 2] {
        let limits = match self.target {
            SynthesisMappingField::Number(_, field) => field.mapping_limits(),
            SynthesisMappingField::Pair { field, .. } => field.range(),
        };
        limits.map(|bound| bound as f32)
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn style_synthesis_number_mapping(
        &self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        expected: &SynthesisEdit,
        target: StyleSynthesisNumberTarget,
    ) -> Result<StyleSynthesisNumberMapping<E>, GtxGraphError<E>> {
        let StyleSynthesisNumberTarget { pass, layer, field } = target;
        self.texture_number_mapping(
            owner,
            SynthesisNumberOwner::paint(canvas, pass),
            expected,
            SynthesisMappingField::Number(layer, field),
        )
    }

    /// Opens one mapping in a standalone, locally edited Texture.
    pub fn synthesis_number_mapping(
        &self,
        owner: &PointwiseNode<E>,
        expected: &SynthesisEdit,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Result<StyleSynthesisNumberMapping<E>, GtxGraphError<E>> {
        self.texture_number_mapping(
            owner,
            SynthesisNumberOwner::Texture,
            expected,
            SynthesisMappingField::Number(layer, field),
        )
    }

    pub(super) fn texture_number_mapping(
        &self,
        owner: &PointwiseNode<E>,
        receiver: SynthesisNumberOwner<E>,
        expected: &SynthesisEdit,
        target: SynthesisMappingField,
    ) -> Result<StyleSynthesisNumberMapping<E>, GtxGraphError<E>> {
        let draft = SynthesisNumberDraft::capture(&self.branches(), owner, &receiver, expected)?;
        let axis = target.axis();
        let adjustment = target
            .adjustment(&draft.synthesis)
            .ok_or_else(unavailable_control)?;
        let (_, control) = self
            .synthesis_mapping_control(adjustment, axis)
            .ok_or_else(unavailable_control)?;
        let record = adjustment.components()[axis]
            .curve
            .as_ref()
            .ok_or_else(unavailable_control)?;
        let curve = super::super::numeric_adjusters::environment_mapping_curve(record)
            .map_err(curve_error)?;
        record
            .restore_dormant_modulation(&curve)
            .map_err(|error| curve_error(error.into()))?;
        Ok(StyleSynthesisNumberMapping {
            owner: owner.clone(),
            receiver,
            synthesis: draft.synthesis,
            target,
            control: control.clone(),
            initial: curve_bytes(&curve)?,
            output: [curve.minimum(), curve.maximum()],
            curve,
        })
    }

    pub fn edit_style_synthesis_number_mapping(
        &mut self,
        mapping: &StyleSynthesisNumberMapping<E>,
    ) -> Result<bool, GtxGraphError<E>> {
        self.edit_synthesis_number_mapping(mapping)
    }

    /// Applies a mapping to the Texture revision from which it was opened.
    pub fn edit_synthesis_number_mapping(
        &mut self,
        mapping: &StyleSynthesisNumberMapping<E>,
    ) -> Result<bool, GtxGraphError<E>> {
        self.edit_synthesis_number_mapping_with_input_geometry(mapping, None)
    }

    /// Applies a mapping using a current raw guide raster when it edits a graph-fed Patches shape.
    pub fn edit_synthesis_number_mapping_with_input_geometry(
        &mut self,
        mapping: &StyleSynthesisNumberMapping<E>,
        input: Option<crate::ImageGeometry>,
    ) -> Result<bool, GtxGraphError<E>> {
        let target = mapping.target;
        let axis = target.axis();
        let mut draft = SynthesisNumberDraft::capture(
            &self.branches(),
            &mapping.owner,
            &mapping.receiver,
            &mapping.synthesis,
        )?;
        let mut adjustment = target
            .adjustment(&draft.synthesis)
            .ok_or_else(unavailable_control)?
            .clone();
        let (metadata, control) = self
            .synthesis_mapping_control(&adjustment, axis)
            .filter(|(_, value)| value.lock_order() == mapping.control.lock_order())
            .ok_or_else(unavailable_control)?;
        let [min, max] = mapping.bounds();
        let [lower, upper] = [mapping.curve.minimum(), mapping.curve.maximum()];
        if [lower, upper]
            .into_iter()
            .zip(mapping.output)
            .any(|(value, original)| {
                !value.is_finite() || (value != original && !(min..=max).contains(&value))
            })
        {
            return Err(GtxGraphError::NumericAdjuster(
                "Enter output bounds within the mapping range.",
            ));
        }
        let bytes = curve_bytes(&mapping.curve)?;
        if bytes == mapping.initial {
            return Ok(false);
        }
        let mut reader = BinaryCursor::new(&bytes)
            .map_err(|error| curve_error(CurveError::Binary(error).into()))?;
        let mut curve = CurveRecord::new(lower, upper, mapping.curve.is_wrapped());
        curve.load(&mut reader, 3.75).map_err(|error| {
            curve_error(PointwiseError::ModernProperty(
                PropertyRecordError::Curve(error).into(),
            ))
        })?;
        curve
            .capture_dormant_modulation(&mapping.curve)
            .map_err(|error| curve_error(error.into()))?;
        adjustment.components_mut()[axis].curve = Some(curve);
        let mapped = sample_connection(&adjustment, axis, metadata, control)?;
        let changed = match target {
            SynthesisMappingField::Number(layer, field) => {
                draft.synthesis.set_number_adjustment_with_input_geometry(
                    layer,
                    field,
                    Some((adjustment, mapped)),
                    input,
                )
            }
            SynthesisMappingField::Pair { layer, field, .. } => {
                let mut axes = [None; 2];
                axes[axis] = Some(mapped);
                draft
                    .synthesis
                    .set_pair_adjustment(layer, field, Some((adjustment, axes)))
            }
        }
        .ok_or_else(stale_paint)?;
        if !changed {
            return Ok(false);
        }
        self.commit_synthesis_number_change(
            draft.into_edit()?,
            None,
            Vec::new(),
            "Edit Synthesis Adjuster Mapping",
        )
    }

    pub(super) fn synthesis_mapping_control(
        &self,
        adjustment: &AdjustmentRecord,
        axis: usize,
    ) -> Option<(&AdjusterMetadataRecord, &NumericCatalogValue)> {
        let key = link_key(adjustment, axis)?;
        self.catalog.entries().find_map(|(entry, _)| match entry {
            GtxCatalogRecord::Numeric { metadata, value } if matches_key(metadata, key) => {
                Some((metadata, value))
            }
            _ => None,
        })
    }
}

fn curve_bytes<E>(curve: &SharedCurve<E>) -> Result<Vec<u8>, GtxGraphError<E>> {
    let mut writer = BinaryWriter::default();
    curve
        .save(&mut writer)
        .map_err(|error| curve_error(error.into()))?;
    Ok(writer.bytes().to_vec())
}

fn curve_error<E>(error: PointwiseError<E>) -> GtxGraphError<E> {
    GtxGraphError::Node(error.into())
}
