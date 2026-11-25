use super::graph::Payload;
use super::scene_primitive::{
    limits, make_placement_properties, make_property, property::PropertyRoot,
};
use super::{PointwiseError, PointwiseNode, RenderCaptureSession, guard};
use crate::{
    ImportedMesh, ModernNodeProperty, ModernNodePropertyKind, ModernNodeValue,
    ModernPropertyConfiguration, ModernPropertyRecord, ModernPropertyValue, Scene,
    SceneAllocationEffects, SceneTransform, SceneVector,
};
use std::sync::{Arc, Mutex};

pub(super) struct ImportState<E> {
    pub(super) properties: Vec<Arc<PropertyRoot<E>>>,
    pub(super) mesh: Mutex<Option<ImportedMesh>>,
}

impl<E: 'static> PointwiseNode<E> {
    pub fn is_import_3d(&self) -> bool {
        matches!(&self.core().payload, Payload::SceneImport { .. })
    }

    pub fn import_3d() -> Result<Self, PointwiseError<E>> {
        let mut properties = make_placement_properties()?;
        properties.push(make_property(
            "Max Smooth Angle",
            ModernPropertyValue::Double {
                current: 80.,
                previous: 80.,
            },
            vec![limits(3, 1, 0, 0, 180)?],
            ModernNodePropertyKind::Double,
        )?);
        for label in ["Auto Center", "Auto Scale"] {
            let mut property = ModernPropertyRecord::new(
                ModernPropertyValue::Boolean(true),
                ModernPropertyConfiguration {
                    can_create_animation: false,
                    adjustment: None,
                },
            )
            .map_err(PointwiseError::ModernProperty)?;
            property.name = Some(label.encode_utf16().collect());
            properties.push(PropertyRoot::new(
                property,
                vec![limits(0, 1, 0, 0, 1)?],
                ModernNodePropertyKind::Boolean,
            )?);
        }
        Ok(Self::publish_scene_import(properties, None))
    }

    pub fn import_3d_properties(&self) -> Option<Vec<ModernNodeProperty<E>>> {
        let Payload::SceneImport { state } = &self.core().payload else {
            return None;
        };
        Some(
            state
                .properties
                .iter()
                .map(|root| ModernNodeProperty::from_root(self.clone(), root.clone()))
                .collect(),
        )
    }

    pub fn imported_mesh(&self) -> Option<ImportedMesh> {
        let Payload::SceneImport { state } = &self.core().payload else {
            return None;
        };
        guard(&state.mesh).clone()
    }

    /// Returns an independent node whose properties are copied and whose model
    /// asset can participate in the document's normal replace and Undo command.
    pub fn with_imported_mesh(&self, mesh: ImportedMesh) -> Result<Self, PointwiseError<E>> {
        self.copy_import_3d(Some(mesh))
    }

    /// Copies the node's placement properties while clearing its embedded model.
    pub fn without_imported_mesh(&self) -> Result<Self, PointwiseError<E>> {
        self.copy_import_3d(None)
    }

    fn copy_import_3d(&self, mesh: Option<ImportedMesh>) -> Result<Self, PointwiseError<E>> {
        let Payload::SceneImport { state } = &self.core().payload else {
            return Err(PointwiseError::InvalidCast);
        };
        state.copy(mesh, true, None)
    }

    pub(super) fn render_imported_scene(
        &self,
        time: f64,
        allocation: &mut SceneAllocationEffects<'_>,
    ) -> Result<Option<Scene>, PointwiseError<E>> {
        let Some(mesh) = self.imported_mesh() else {
            return Ok(None);
        };
        let properties = self
            .import_3d_properties()
            .ok_or(PointwiseError::InvalidCast)?;
        let [size, location, orientation, angle, center, scale] = properties.as_slice() else {
            return Err(PointwiseError::InvalidCast);
        };
        let (
            ModernNodeValue::Vector3D(size),
            ModernNodeValue::Vector3D(location),
            ModernNodeValue::Vector3D(orientation),
            ModernNodeValue::Double(angle),
            ModernNodeValue::Boolean(center),
            ModernNodeValue::Boolean(scale),
        ) = (
            size.evaluated_value(time)?,
            location.evaluated_value(time)?,
            orientation.evaluated_value(time)?,
            angle.evaluated_value(time)?,
            center.value(),
            scale.value(),
        )
        else {
            return Err(PointwiseError::InvalidCast);
        };
        let placement = SceneTransform::from_components(
            SceneVector::new(size[0], size[1], size[2]),
            SceneVector::new(location[0], location[1], location[2]),
            SceneVector::new(orientation[0], orientation[1], orientation[2]),
        );
        mesh.scene(center, scale, angle, placement, allocation)
            .map(Some)
            .map_err(PointwiseError::Scene)
    }
}

impl<E: 'static> ImportState<E> {
    pub(super) fn copy(
        &self,
        mesh: Option<ImportedMesh>,
        original: bool,
        mut capture: Option<&mut RenderCaptureSession<E>>,
    ) -> Result<PointwiseNode<E>, PointwiseError<E>> {
        let properties = self
            .properties
            .iter()
            .map(|property| property.copied(original, capture.as_deref_mut()))
            .collect::<Result<_, _>>()?;
        Ok(PointwiseNode::publish_scene_import(properties, mesh))
    }
}
