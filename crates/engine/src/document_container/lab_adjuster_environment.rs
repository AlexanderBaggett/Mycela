//! Rename and merge the links stored inside Environment snapshots.
use super::*;
use crate::document_container::graph::numeric_adjusters::map_environment_number;
use crate::{EnvironmentRecord, SharedGradientRecord};

pub(super) struct FieldRename<'a> {
    pub(super) group: [u8; 16],
    pub(super) tag: i32,
    pub(super) before: &'a [u16],
    pub(super) after: &'a [u16],
    pub(super) destination: Option<&'a MergeValue>,
}

impl FieldRename<'_> {
    pub(super) fn stage_maps<E: 'static>(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        catalog: &GraphCatalog,
    ) -> Result<Box<EnvironmentAdjusterBatch<E>>, GtxGraphError<E>> {
        let reference = match self.destination {
            Some(MergeValue::Reference(EnvironmentLabReference::Gradient(gradient))) => {
                let record = gradient
                    .live()
                    .wire_record()
                    .map_err(|error| node_error(PointwiseError::Gradient(error)))?;
                Some(EnvironmentLabReference::Gradient(
                    SharedGradientRecord::new(record),
                ))
            }
            Some(MergeValue::Reference(value)) => Some(value.clone()),
            _ => None,
        };
        let mut batch = EnvironmentAdjusterBatch::empty();
        batch.stage_mapped_receivers(branches, catalog, true, |map| {
            if matches!(self.tag, 0 | 4) {
                self.map_numeric(map)
            } else {
                self.map_reference(map, reference.as_ref())
            }
        })?;
        Ok(Box::new(batch))
    }

    fn map_numeric<E: 'static>(
        &self,
        before: &EnvironmentRecord,
    ) -> Result<Option<Arc<EnvironmentRecord>>, GtxGraphError<E>> {
        let update = match self.destination {
            Some(MergeValue::Numeric(value)) => Some(value),
            _ => None,
        };
        let mut after = None;
        for field in before.numeric_receivers() {
            if before.numeric_adjuster_key(field) != Some((self.group, self.before)) {
                continue;
            }
            let mut adjustment = field
                .scope_adjustment(before, self.group)
                .expect("matched numeric adjustment");
            adjustment.components_mut()[0].name = self.after.to_vec();
            let map = after.get_or_insert_with(|| before.clone());
            if let Some(update) = update
                && let Some((_, value)) =
                    map_environment_number::<E>(std::slice::from_ref(update), &adjustment)
                        .map_err(node_error)?
            {
                field.apply(map, value).map_err(node_error)?;
            }
            field.set_adjustment(map, Some(adjustment));
        }
        Ok(after.map(Arc::new))
    }

    fn map_reference<E>(
        &self,
        before: &EnvironmentRecord,
        value: Option<&EnvironmentLabReference>,
    ) -> Result<Option<Arc<EnvironmentRecord>>, GtxGraphError<E>> {
        let mut after = None;
        for field in before.color_receivers() {
            if field.type_tag() != self.tag
                || before.color_adjuster_key(field) != Some((self.group, self.before))
            {
                continue;
            }
            let map = after.get_or_insert_with(|| before.clone());
            if let Some(value) = value {
                map.set_color_adjuster(field, Some((self.group, self.after, value.clone())))
                    .map_err(|error| node_error(PointwiseError::Gradient(error)))?;
            } else {
                map.rename_color_adjuster(field, self.after);
            }
        }
        Ok(after.map(Arc::new))
    }
}

fn node_error<E>(error: PointwiseError<E>) -> GtxGraphError<E> {
    GtxGraphError::Node(error.into())
}
