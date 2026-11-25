use super::*;
use crate::AnimationCurves;

/// A detached mapping draft tied to the property axis it was opened for.
pub struct LabNumericMapping<E = ()> {
    property: LabNumericProperty<E>,
    adjustment: SharedPropertyAdjustment<E>,
    links: Links,
    index: i32,
    initial: Option<Vec<u8>>,
    output: [f32; 2],
    bounds: [f32; 2],
    curve: SharedCurve<E>,
}

impl<E> LabNumericMapping<E> {
    pub fn curve(&self) -> &SharedCurve<E> {
        &self.curve
    }

    pub fn bounds(&self) -> [f32; 2] {
        self.bounds
    }
}

impl<E: 'static> Gtx375GraphDocument<E> {
    pub fn numeric_property_mapping(
        &self,
        property: &LabNumericProperty<E>,
        axis: LabNumericAxis,
    ) -> Result<LabNumericMapping<E>, GtxGraphError<E>> {
        let receiver = &property.0;
        let index = match (property.is_pair(), axis.index()) {
            (true, Some(index)) => index,
            (false, None) => 0,
            _ => {
                return Err(GtxGraphError::NumericAdjuster(
                    "Choose the property axis to map.",
                ));
            }
        };
        let adjustment = receiver
            .retained_adjustment()
            .ok_or(GtxGraphError::NumericAdjuster(
                "Link an adjuster to this property first.",
            ))?;
        let links = Links::read(Some(&adjustment));
        self.mapping_adjuster(receiver, &links, index)?;
        let source = retained_curve(&adjustment, index as i32)?;
        let initial = source.as_ref().map(curve_bytes).transpose()?;
        let curve = match source {
            Some(curve) => curve.source_clone().map_err(PointwiseError::from),
            None => adjustment.default_curve(index as i32),
        }
        .map_err(|error| GtxGraphError::Node(error.into()))?;
        let output = [curve.minimum(), curve.maximum()];
        let bounds = receiver
            .mapping_bounds()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
        Ok(LabNumericMapping {
            property: property.clone(),
            adjustment,
            links,
            index: index as i32,
            initial,
            output,
            bounds,
            curve,
        })
    }

    /// Replace the axis curve and recompute the receiver in one history action.
    pub fn edit_numeric_property_mapping(
        &self,
        mapping: &LabNumericMapping<E>,
    ) -> Result<bool, GtxGraphError<E>> {
        let receiver = &mapping.property.0;
        let before = receiver
            .retained_adjustment()
            .filter(|adjustment| adjustment.same_reference(&mapping.adjustment))
            .ok_or(GtxGraphError::NumericAdjuster(
                "The property link changed. Reopen its mapping.",
            ))?;
        let links = Links::read(Some(&before));
        let original = retained_curve(&before, mapping.index)?;
        if links != mapping.links
            || original.as_ref().map(curve_bytes).transpose()? != mapping.initial
        {
            return Err(GtxGraphError::NumericAdjuster(
                "The property mapping changed. Reopen it to edit the current curve.",
            ));
        }
        let update = self.mapping_adjuster(receiver, &links, mapping.index as usize)?;
        let output = [mapping.curve.minimum(), mapping.curve.maximum()];
        let [minimum, maximum] = receiver
            .mapping_bounds()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
        if output
            .into_iter()
            .zip(mapping.output)
            .any(|(value, before)| {
                !value.is_finite() || (before != value && !(minimum..=maximum).contains(&value))
            })
        {
            return Err(GtxGraphError::NumericAdjuster(
                "Enter output bounds within the property's range.",
            ));
        }
        let bytes = curve_bytes(&mapping.curve)?;
        let unchanged = match &mapping.initial {
            Some(initial) => *initial == bytes,
            None => {
                curve_bytes(
                    &before
                        .default_curve(mapping.index)
                        .map_err(|error| GtxGraphError::Node(error.into()))?,
                )? == bytes
            }
        };
        if unchanged {
            return Ok(false);
        }
        let curve = mapping
            .curve
            .source_clone()
            .map_err(|error| GtxGraphError::Node(PointwiseError::from(error).into()))?;
        let mut parts = before.capture_parts();
        let mut curves = (0..parts.count)
            .map(|index| {
                parts
                    .curves
                    .as_ref()
                    .map_or(Ok(None), |curves| curves.value(index))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| GtxGraphError::Node(PointwiseError::from(error).into()))?;
        let slot = curves
            .get_mut(mapping.index as usize)
            .ok_or(GtxGraphError::NumericAdjuster(
                "The property mapping is unavailable.",
            ))?;
        *slot = Some(curve);
        parts.curves = Some(AnimationCurves::from_values(curves));
        let after = SharedPropertyAdjustment::from_capture_parts(parts);
        let shape = receiver
            .shape()
            .map_err(|error| GtxGraphError::Node(error.into()))?
            .ok_or(GtxGraphError::NumericAdjuster(
                "This property cannot use a numeric adjuster.",
            ))?;
        let state = map_property(&[update], shape, &after)
            .and_then(|value| value.map(|value| receiver.prepare(value)).transpose())
            .map_err(|error| GtxGraphError::Node(error.into()))?;
        let edit = NumericLinkEdit {
            property: receiver.downgrade(),
            before: Some(before),
            after: Some(after),
            state,
            created: None,
            retired: Vec::new(),
        };
        let property = edit
            .publish(HistoryDirection::Redo)
            .map_err(GtxGraphError::History)?;
        self.record_lab_edit("Edit Adjuster Mapping", edit)
            .and(notify(&property))
            .map_err(GtxGraphError::History)?;
        Ok(true)
    }

    fn mapping_adjuster(
        &self,
        receiver: &NumericReceiver<E>,
        links: &Links,
        index: usize,
    ) -> Result<NumericCatalogUpdate, GtxGraphError<E>> {
        if !self.contains_numeric_receiver(receiver) {
            return Err(GtxGraphError::NumericAdjuster(
                "This property is no longer available.",
            ));
        }
        let name = links.names[index].as_ref().or_else(|| {
            if links.paired {
                links.names.iter().flatten().next()
            } else {
                None
            }
        });
        self.catalog
            .entries()
            .filter_map(|(record, _)| match record {
                GtxCatalogRecord::Numeric { metadata, value }
                    if name.is_some()
                        && metadata.name.as_ref() == name
                        && links.includes(metadata) =>
                {
                    NumericCatalogUpdate::reconnect(metadata, value)
                }
                _ => None,
            })
            .last()
            .ok_or(GtxGraphError::NumericAdjuster(
                "The linked adjuster is no longer available.",
            ))
    }
}

fn retained_curve<E: 'static>(
    adjustment: &SharedPropertyAdjustment<E>,
    index: i32,
) -> Result<Option<SharedCurve<E>>, GtxGraphError<E>> {
    adjustment
        .capture_parts()
        .curves
        .map_or(Ok(None), |curves| {
            curves
                .value(index)
                .map_err(|error| GtxGraphError::Node(PointwiseError::from(error).into()))
        })
}

fn curve_bytes<E>(curve: &SharedCurve<E>) -> Result<Vec<u8>, GtxGraphError<E>> {
    let mut writer = BinaryWriter::default();
    curve
        .save(&mut writer)
        .map_err(|error| GtxGraphError::Node(PointwiseError::from(error).into()))?;
    Ok(writer.bytes().to_vec())
}
