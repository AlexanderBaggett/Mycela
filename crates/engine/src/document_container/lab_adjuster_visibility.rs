//! The Lab view of retained controls with inactive Style paints or source patches.
use super::environment_adjusters::graph_nodes;
use super::lab_adjuster_metadata::section_id;
use super::style_synthesis_numbers::{LinkKey, link_keys};
use super::*;
use crate::pointwise::reference_adjustment::LabReferenceEdit;
use crate::{PointwiseFilter, SynthesisRecord};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct ParkedLabNames(HashMap<([u8; 16], i32), HashSet<Vec<u16>>>);

fn lab_family(tag: i32) -> Option<i32> {
    match tag {
        0 | 4 => Some(0),
        1 | 6 => Some(tag),
        _ => None,
    }
}

impl ParkedLabNames {
    fn add(&mut self, (group, tag, name): LinkKey<'_>) {
        if let Some(tag) = lab_family(tag) {
            self.0
                .entry((group, tag))
                .or_default()
                .insert(name.to_vec());
        }
    }

    fn remove(&mut self, (group, tag, name): LinkKey<'_>) {
        if let Some(tag) = lab_family(tag)
            && let Some(names) = self.0.get_mut(&(group, tag))
        {
            names.remove(name);
        }
    }

    pub(super) fn contains(&self, metadata: &AdjusterMetadataRecord) -> bool {
        lab_family(metadata.type_tag).is_some_and(|tag| {
            self.0.get(&(metadata.group_id, tag)).is_some_and(|names| {
                metadata
                    .name
                    .as_deref()
                    .is_some_and(|name| names.contains(name))
            })
        })
    }
}

fn visit_links(record: &SynthesisRecord, mut visit: impl FnMut(LinkKey<'_>, bool)) {
    record.visit_numeric_adjustments_with_visibility(|adjustment, live| {
        link_keys(adjustment).for_each(|key| visit(key, live));
    });
    record.visit_color_adjustments(|key| visit(key, true));
}

fn visit_textures<E: 'static>(
    filter: &PointwiseFilter<E>,
    mut visit: impl FnMut(&SynthesisRecord),
) {
    if let Some(state) = filter
        .synthesis_link_state()
        .filter(|state| state.catalog.is_none())
    {
        visit(&state.local);
    }
    filter.visit_synthesis_paints(&mut visit);
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Returns the value and metadata rows of the current Lab menu. Inactive-only
    /// receivers still use the retained catalog for history and shared edits.
    pub fn lab_menu_adjusters(
        &self,
        selected: Option<&PointwiseNode<E>>,
    ) -> (Vec<LabNumericAdjuster>, Vec<LabAdjusterMetadata>) {
        let group = section_id(selected);
        let parked = self.parked_only_lab_names(&self.branches(), Some(group));
        let visible = |metadata: &AdjusterMetadataRecord| !parked.contains(metadata);
        (
            self.numeric_adjusters_in_section(group, visible),
            self.lab_metadata_in_section(group, visible),
        )
    }

    /// Binding an explicitly typed dormant name revives its retained value.
    pub(super) fn resolve_parked_numeric_link(
        &self,
        link: LabNumericLink,
        group: [u8; 16],
        tag: i32,
    ) -> LabNumericLink {
        let LabNumericLink::Create(name) = &link else {
            return link;
        };
        let name: Vec<_> = name.trim().encode_utf16().collect();
        let Some((metadata, value)) = self.catalog.entries().find_map(|(entry, _)| match entry {
            GtxCatalogRecord::Numeric { metadata, value }
                if metadata.group_id == group
                    && lab_family(metadata.type_tag) == lab_family(tag)
                    && metadata.name.as_deref() == Some(name.as_slice()) =>
            {
                Some((metadata, value))
            }
            _ => None,
        }) else {
            return link;
        };
        if self
            .parked_only_lab_names(&self.branches(), Some(group))
            .contains(metadata)
            && let Some(control) = LabNumericAdjuster::read(metadata, value)
        {
            return LabNumericLink::Existing(control);
        }
        link
    }

    pub(super) fn parked_only_lab_names(
        &self,
        branches: &[Option<PointwiseNode<E>>],
        scope: Option<[u8; 16]>,
    ) -> ParkedLabNames {
        let mut parked = ParkedLabNames::default();
        if !self.catalog.entries().any(|(entry, _)| {
            let metadata = entry.metadata();
            lab_family(metadata.type_tag).is_some()
                && scope.is_none_or(|scope| metadata.group_id == scope)
        }) {
            return parked;
        }

        let filters: Vec<_> = graph_nodes(branches)
            .filter_map(|node| node.filter())
            .collect();
        let mut add_parked = |key: LinkKey<'_>| {
            if scope.is_none_or(|scope| key.0 == scope) {
                parked.add(key);
            }
        };
        for filter in &filters {
            filter.visit_parked_synthesis_paints(|record| {
                visit_links(record, |key, _| add_parked(key));
            });
            visit_textures(filter, |record| {
                visit_links(record, |key, live| {
                    if !live {
                        add_parked(key);
                    }
                });
            });
        }
        for (entry, _) in self.catalog.entries() {
            if let GtxCatalogRecord::Synthesis { value, .. } = entry {
                visit_links(&value.record(), |key, live| {
                    if !live {
                        add_parked(key);
                    }
                });
            }
        }
        if parked.0.is_empty() {
            return parked;
        }

        let mut mark_live = |record: &SynthesisRecord| {
            visit_links(record, |key, live| {
                if live {
                    parked.remove(key);
                }
            });
        };
        for filter in &filters {
            visit_textures(filter, &mut mark_live);
        }
        for (entry, _) in self.catalog.entries() {
            if let GtxCatalogRecord::Synthesis { value, .. } = entry {
                mark_live(&value.record());
            }
        }
        for (entry, _) in self.catalog.entries() {
            let metadata = entry.metadata();
            let Some(name) = metadata
                .name
                .as_deref()
                .filter(|_| parked.contains(metadata))
            else {
                continue;
            };
            let key = (metadata.group_id, metadata.type_tag, name);
            let live = if key.1 == 6 {
                graph_nodes(branches).any(|node| LabReferenceEdit::node_uses_control(&node, key))
                    || self.count_environment_links(branches, |map| map.count_color_adjuster(key))
                        > 0
            } else {
                Self::graph_uses_numeric_adjuster(branches, metadata)
                    || (key.1 != 1
                        && self.count_environment_links(branches, |map| {
                            map.count_numeric_adjuster(key.0, name)
                        }) > 0)
            };
            if live {
                parked.remove(key);
            }
        }
        parked
    }
}

#[cfg(test)]
mod tests {
    use super::super::environment_catalog_tests::LabTestHistory;
    use super::*;
    use crate::{
        LabNumericProperty, NodeWireReadPolicy, PointwiseFilter, PointwiseKind, StylePaintKind,
        SynthesisLayerKind,
        SynthesisNumberField::{PositionJitterX, Seed},
    };

    #[test]
    fn parked_lab_entries_are_hidden_by_type_and_restored_with_their_receivers() {
        let style = PointwiseFilter::<()>::new(PointwiseKind::Style).unwrap();
        let owner = style.node();
        let mut edit = style.style_edit().unwrap();
        edit.set_paint(0, StylePaintKind::Synthesis).unwrap();
        let mut draft = edit.synthesis_edit(0).unwrap();
        draft.add_layer(SynthesisLayerKind::Patches);
        edit.set_synthesis(0, draft).unwrap();
        style.edit_style(edit, "Set up texture").unwrap();
        let texture = || style.style_edit().unwrap().synthesis_edit(0).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        let property = LabNumericProperty::transparency(&solid).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(owner.clone()), Some(solid)]).unwrap();
        for (layer, field, name) in [
            (None, Seed, "Dormant"),
            (Some(0), PositionJitterX, "Shared"),
        ] {
            doc.set_style_synthesis_number_adjuster(
                &owner,
                None,
                &texture(),
                StyleSynthesisNumberTarget::new(0, layer, field),
                LabNumericLink::Create(name.into()),
            )
            .unwrap();
        }
        let seed = doc
            .style_synthesis_number_adjuster(&texture(), None, Seed)
            .unwrap();
        let shared = doc
            .style_synthesis_number_adjuster(&texture(), Some(0), PositionJitterX)
            .unwrap();
        doc.set_numeric_property_link(
            &property,
            LabNumericAxis::Whole,
            LabNumericLink::Existing(shared.clone()),
        )
        .unwrap();
        let entry = ValueAdjusterRecord::new(
            ValueAdjusterKind::Dual,
            Some("Dormant".encode_utf16().collect()),
            [0; 16],
        );
        use crate::document_container::catalog::NumericCatalogValue;
        doc.catalog
            .stage(GtxCatalogRecord::Numeric {
                metadata: entry.metadata,
                value: NumericCatalogValue::new(entry.payload),
            })
            .store(true, std::sync::atomic::Ordering::Release);

        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let check_parked = |doc: &Gtx375GraphDocument<()>| {
            let (controls, metadata) = doc.lab_menu_adjusters(None);
            assert_eq!(controls.len(), 2);
            assert_eq!(metadata.len(), 2);
            assert!(!controls.iter().any(|control| control.same_reference(&seed)));
            assert!(
                controls
                    .iter()
                    .any(|control| control.same_reference(&shared))
            );
            assert!(controls.iter().any(|control| control.name() == "Dormant"
                && matches!(control.value(), LabNumericValue::Dual(_))));
            let options = doc.numeric_property_adjusters(&property, LabNumericAxis::Whole);
            assert_eq!(options.len(), 1);
            assert!(options[0].same_reference(&shared));
            assert_eq!(doc.numeric_lab_adjusters(None).len(), 3);
        };

        let mut edit = style.style_edit().unwrap();
        edit.set_paint(0, StylePaintKind::Solid).unwrap();
        assert!(style.edit_style(edit, "Park texture").unwrap());
        check_parked(&doc);
        history.undo().unwrap();
        assert_eq!(doc.lab_menu_adjusters(None).0.len(), 3);
        history.redo().unwrap();
        check_parked(&doc);

        let mut project = BinaryWriter::default();
        doc.save_project(&mut project).unwrap();
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        for reopened in [
            Gtx375GraphDocument::<()>::load_project(
                project.bytes(),
                NodeWireReadPolicy::new(128, 4096),
            )
            .unwrap(),
            Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap(),
        ] {
            let controls = reopened.numeric_lab_adjusters(None);
            assert_eq!(controls.len(), 2);
            assert!(controls.iter().any(|control| control.name() == "Shared"));
            assert!(controls.iter().any(|control| control.name() == "Dormant"
                && matches!(control.value(), LabNumericValue::Dual(_))));
        }
        check_parked(&doc);
        assert!(
            doc.set_numeric_property_link(
                &property,
                LabNumericAxis::Whole,
                LabNumericLink::Create("Dormant".into())
            )
            .unwrap()
        );
        let revived = doc
            .lab_menu_adjusters(None)
            .0
            .into_iter()
            .find(|control| control.same_reference(&seed))
            .unwrap();
        assert_eq!(revived.value(), seed.value());
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 3);
        history.undo().unwrap();
        check_parked(&doc);
        let mut removal = style.style_edit().unwrap();
        assert!(removal.remove(0));
        assert!(
            doc.edit_style_removing_unused_synthesis_adjusters(
                &owner,
                None,
                &removal,
                "Remove parked texture"
            )
            .unwrap()
        );
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 2);
        history.undo().unwrap();
        check_parked(&doc);
        assert!(
            doc.edit_numeric_lab_adjuster(None, &shared, LabNumericValue::Number(73.))
                .unwrap()
        );
        let mut edit = style.style_edit().unwrap();
        edit.set_paint(0, StylePaintKind::Synthesis).unwrap();
        assert!(style.edit_style(edit, "Restore texture").unwrap());
        assert_eq!(doc.lab_menu_adjusters(None).0.len(), 3);
        assert!(
            doc.style_synthesis_number_adjuster(&texture(), None, Seed)
                .unwrap()
                .same_reference(&seed)
        );
        assert_eq!(texture().number_value(Some(0), PositionJitterX), Some(73.));
    }
}
