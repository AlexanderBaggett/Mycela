//! The dialog retains both forms of a modulated control in the document's memory.
//! The current GTX layout writes only the selected form.
use super::*;
use crate::{
    BinaryCursor, BinaryWriter, CurveControlValue, CurveKind, CurveLayerRecord, CurveRecord,
    CurveRecordError,
};

const CURRENT_WIRE_VERSION: f64 = 3.75;

impl CurveRecord {
    /// Keep inactive child layers and scalar values omitted by the ordinary save.
    pub(crate) fn capture_dormant_modulation<E>(
        &mut self,
        curve: &SharedCurve<E>,
    ) -> Result<(), CurveError<E>> {
        for (record, layer) in self.layers.iter_mut().zip(curve.layers()) {
            capture_layer(record, &layer.ok_or(CurveError::NullReference)?)?;
        }
        Ok(())
    }

    /// Restore the alternate settings after loading the active curve into a draft.
    pub(crate) fn restore_dormant_modulation<E>(
        &self,
        curve: &SharedCurve<E>,
    ) -> Result<(), CurveError<E>> {
        for (record, layer) in self.layers.iter().zip(curve.layers()) {
            restore_layer(record, &layer.ok_or(CurveError::NullReference)?)?;
        }
        Ok(())
    }
}

fn controls<E>(
    layer: &CurveLayerHandle<E>,
) -> impl Iterator<Item = Result<CurveControlHandle<E>, CurveError<E>>> {
    (0..14)
        .map(|index| layer.common(index))
        .chain(layer.shape_controls().into_iter().map(Ok))
}

fn capture_layer<E>(
    record: &mut CurveLayerRecord,
    layer: &CurveLayerHandle<E>,
) -> Result<(), CurveError<E>> {
    for (record, control) in record.controls_mut().iter_mut().zip(controls(layer)) {
        let CurveControlValue::Modulated { scalar, child, .. } = &mut record.value else {
            continue;
        };
        let control = control?;
        *scalar = control.scalar()?;
        if let Some(layer) = control.retained_child() {
            let child = match child {
                Some(child) => child.as_mut(),
                None => {
                    let mut writer = BinaryWriter::default();
                    layer.save(&mut writer)?;
                    let mut reader = BinaryCursor::new(writer.bytes())?;
                    let kind = CurveKind::from_tag(layer.source_tag()).map_err(record_error)?;
                    let mut nested = CurveLayerRecord::new(kind);
                    nested
                        .load(&mut reader, CURRENT_WIRE_VERSION)
                        .map_err(record_error)?;
                    child.insert(Box::new(nested)).as_mut()
                }
            };
            capture_layer(child, &layer)?;
        }
    }
    Ok(())
}

fn restore_layer<E>(
    record: &CurveLayerRecord,
    layer: &CurveLayerHandle<E>,
) -> Result<(), CurveError<E>> {
    for (record, control) in record.controls().iter().zip(controls(layer)) {
        let CurveControlValue::Modulated { scalar, child, .. } = &record.value else {
            continue;
        };
        let control = control?;
        if record.enabled && control.scalar()?.to_bits() != scalar.to_bits() {
            control.set_scalar(*scalar)?;
        }
        if let Some(child) = child {
            let layer = match control.retained_child() {
                Some(layer) => layer,
                None => {
                    let mut writer = BinaryWriter::default();
                    child.save(&mut writer).map_err(record_error)?;
                    let mut reader = BinaryCursor::new(writer.bytes())?;
                    let layer = CurveLayerHandle::from_source_tag(child.kind().tag())?;
                    layer.load(&mut reader, CURRENT_WIRE_VERSION)?;
                    control.attach_loaded_child(&layer);
                    layer
                }
            };
            restore_layer(child, &layer)?;
        }
    }
    Ok(())
}

fn record_error<E>(error: CurveRecordError) -> CurveError<E> {
    match error {
        CurveRecordError::Binary(error) => CurveError::Binary(error),
        CurveRecordError::TypeIndex => CurveError::Index,
        CurveRecordError::MissingChild => CurveError::NullReference,
        CurveRecordError::Allocation => CurveError::Allocation,
        CurveRecordError::Validation | CurveRecordError::NoiseSeed => CurveError::InvalidData,
    }
}
