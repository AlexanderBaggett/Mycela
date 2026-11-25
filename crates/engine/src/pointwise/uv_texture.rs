use super::graph::Payload;
use super::scene_primitive::{
    limits, make_boolean_property, make_percent_property, make_placement_properties,
    make_placement_vector_property, make_property, make_selected_property,
};
use super::{PointwiseError, PointwiseNode};
use crate::scene::uv_multi_project::MultiProjection;
use crate::scene::uv_occlusion::Occlusion;
use crate::scene::uv_project::{Projection, ProjectionMode};
use crate::{
    ModernNodeProperty, ModernNodePropertyKind, ModernNodeValue, ModernPropertyValue,
    UvLightSettings,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UvTextureKind {
    Apply,
    AmbientOcclusion,
    Curvature,
    EnvironmentLight,
    EnvironmentOcclusion,
    Light,
    MultiProject,
    Project,
    Thickness,
}

impl UvTextureKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Apply => "UV Apply",
            Self::AmbientOcclusion => "UV Ambient Occlusion",
            Self::Curvature => "UV Curvature",
            Self::EnvironmentLight => "UV Environment Light",
            Self::EnvironmentOcclusion => "UV Environment Occlusion",
            Self::Light => "UV Light",
            Self::MultiProject => "UV Multi Project",
            Self::Project => "UV Project",
            Self::Thickness => "UV Thickness",
        }
    }

    pub(super) fn capture_tag(self) -> u64 {
        match self {
            Self::Apply => 9,
            Self::Project => 10,
            Self::MultiProject => 11,
            Self::Curvature => 12,
            Self::Light => 13,
            Self::Thickness => 14,
            Self::AmbientOcclusion => 16,
            Self::EnvironmentOcclusion => 17,
            Self::EnvironmentLight => 18,
        }
    }

    pub(super) fn image_port(self) -> Option<&'static str> {
        match self {
            Self::Apply | Self::Project | Self::MultiProject => Some("Texture"),
            Self::EnvironmentLight | Self::EnvironmentOcclusion => Some("Environment"),
            _ => None,
        }
    }
}

impl<E: 'static> PointwiseNode<E> {
    pub fn uv_apply() -> Result<Self, PointwiseError<E>> {
        Ok(Self::publish_uv_texture(
            UvTextureKind::Apply,
            vec![make_selected_property(
                "Output",
                &["UV Map", "Textured Model"],
                0,
            )?],
        ))
    }

    pub fn uv_ambient_occlusion() -> Result<Self, PointwiseError<E>> {
        Self::uv_occlusion(UvTextureKind::AmbientOcclusion)
    }

    pub fn uv_environment_occlusion() -> Result<Self, PointwiseError<E>> {
        Self::uv_occlusion(UvTextureKind::EnvironmentOcclusion)
    }

    pub fn uv_environment_light() -> Result<Self, PointwiseError<E>> {
        Ok(Self::publish_uv_texture(
            UvTextureKind::EnvironmentLight,
            vec![make_selected_property(
                "Output",
                &["UV Map", "Textured Model"],
                0,
            )?],
        ))
    }

    fn uv_occlusion(kind: UvTextureKind) -> Result<Self, PointwiseError<E>> {
        Ok(Self::publish_uv_texture(
            kind,
            vec![
                make_property(
                    "Sample Density",
                    ModernPropertyValue::Int32 {
                        current: 20,
                        previous: 20,
                    },
                    vec![limits(0, 1, 0, 0, 100)?],
                    ModernNodePropertyKind::Int32,
                )?,
                make_selected_property("Output", &["UV Map", "Textured Model"], 0)?,
            ],
        ))
    }

    pub fn uv_project() -> Result<Self, PointwiseError<E>> {
        let mut properties = make_placement_properties()?;
        properties.extend([
            make_selected_property(
                "Projection",
                &["Cylindrical", "Planar", "Shrink Wrap", "Spherical"],
                0,
            )?,
            make_boolean_property("Tile", true)?,
            make_selected_property("Side", &["Both", "Front", "Back"], 0)?,
            make_selected_property("Output", &["UV Map", "Textured Model"], 0)?,
        ]);
        Ok(Self::publish_uv_texture(UvTextureKind::Project, properties))
    }

    pub fn uv_curvature() -> Result<Self, PointwiseError<E>> {
        Ok(Self::publish_uv_texture(
            UvTextureKind::Curvature,
            vec![make_selected_property(
                "Output",
                &["UV Map", "Textured Model"],
                0,
            )?],
        ))
    }

    pub fn uv_thickness() -> Result<Self, PointwiseError<E>> {
        Ok(Self::publish_uv_texture(
            UvTextureKind::Thickness,
            vec![make_selected_property(
                "Output",
                &["UV Map", "Textured Model"],
                0,
            )?],
        ))
    }

    pub fn uv_light() -> Result<Self, PointwiseError<E>> {
        Ok(Self::publish_uv_texture(
            UvTextureKind::Light,
            vec![
                make_placement_vector_property("Location", [0.; 3], false, 0)?,
                make_percent_property("Shadow Strength", 0.)?,
                make_percent_property("Global Illumination", 0.)?,
                make_selected_property("Output", &["UV Map", "Textured Model"], 0)?,
            ],
        ))
    }

    pub fn uv_multi_project() -> Result<Self, PointwiseError<E>> {
        let mut properties = make_placement_properties()?;
        properties.extend([
            make_selected_property("Sides", &["Four", "Six", "Eight", "Twelve", "Twenty"], 0)?,
            make_percent_property("Feather", 20.)?,
            make_boolean_property("Tile", true)?,
            make_selected_property("Output", &["UV Map", "Textured Model"], 0)?,
        ]);
        Ok(Self::publish_uv_texture(
            UvTextureKind::MultiProject,
            properties,
        ))
    }

    pub fn uv_texture_properties(&self) -> Option<Vec<ModernNodeProperty<E>>> {
        let Payload::UvTexture { properties, .. } = &self.core().payload else {
            return None;
        };
        Some(
            properties
                .iter()
                .map(|root| ModernNodeProperty::from_root(self.clone(), root.clone()))
                .collect(),
        )
    }

    pub fn uv_apply_output(&self) -> Option<ModernNodeProperty<E>> {
        let Payload::UvTexture {
            kind: UvTextureKind::Apply,
            ..
        } = &self.core().payload
        else {
            return None;
        };
        self.uv_texture_output()
    }

    pub fn uv_texture_output(&self) -> Option<ModernNodeProperty<E>> {
        let Payload::UvTexture { properties, .. } = &self.core().payload else {
            return None;
        };
        properties
            .last()
            .map(|root| ModernNodeProperty::from_root(self.clone(), root.clone()))
    }

    pub(super) fn uv_bakes_map(&self, time: f64) -> Result<bool, PointwiseError<E>> {
        let output = self
            .uv_texture_output()
            .ok_or(PointwiseError::InvalidCast)?;
        Ok(matches!(
            output.evaluated_value(time)?,
            ModernNodeValue::Int32(0)
        ))
    }

    pub(super) fn uv_has_texture_input(&self) -> bool {
        matches!(&self.core().payload, Payload::UvTexture { kind, .. } if kind.image_port().is_some())
    }

    pub(super) fn uv_uses_geometry_only(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::UvTexture {
                kind: UvTextureKind::Curvature | UvTextureKind::Light | UvTextureKind::Thickness,
                ..
            }
        )
    }

    pub(super) fn uv_bakes_mesh_material(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::UvTexture {
                kind: UvTextureKind::AmbientOcclusion
                    | UvTextureKind::Curvature
                    | UvTextureKind::EnvironmentLight
                    | UvTextureKind::EnvironmentOcclusion
                    | UvTextureKind::Light
                    | UvTextureKind::Thickness,
                ..
            }
        )
    }

    pub(super) fn uv_occlusion_at(
        &self,
        time: f64,
    ) -> Result<Option<Occlusion>, PointwiseError<E>> {
        let Payload::UvTexture { kind, properties } = &self.core().payload else {
            return Ok(None);
        };
        let environment = match kind {
            UvTextureKind::AmbientOcclusion => false,
            UvTextureKind::EnvironmentOcclusion => true,
            _ => return Ok(None),
        };
        let property = ModernNodeProperty::from_root(self.clone(), properties[0].clone());
        let ModernNodeValue::Int32(density) = property.evaluated_value(time)? else {
            return Err(PointwiseError::InvalidCast);
        };
        Ok(Some(Occlusion {
            density,
            environment,
        }))
    }

    pub(super) fn is_uv_environment_light(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::UvTexture {
                kind: UvTextureKind::EnvironmentLight,
                ..
            }
        )
    }

    pub(super) fn is_uv_light(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::UvTexture {
                kind: UvTextureKind::Light,
                ..
            }
        )
    }

    pub(super) fn is_uv_thickness(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::UvTexture {
                kind: UvTextureKind::Thickness,
                ..
            }
        )
    }

    pub(super) fn is_uv_vertex_map(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::UvTexture {
                kind: UvTextureKind::AmbientOcclusion
                    | UvTextureKind::Curvature
                    | UvTextureKind::EnvironmentOcclusion
                    | UvTextureKind::Thickness,
                ..
            }
        )
    }

    pub(super) fn uv_light_at(
        &self,
        time: f64,
    ) -> Result<Option<UvLightSettings>, PointwiseError<E>> {
        let Payload::UvTexture {
            kind: UvTextureKind::Light,
            properties,
        } = &self.core().payload
        else {
            return Ok(None);
        };
        let value = |index: usize| {
            ModernNodeProperty::from_root(self.clone(), properties[index].clone())
                .evaluated_value(time)
        };
        match (value(0)?, value(1)?, value(2)?) {
            (
                ModernNodeValue::Vector3D(position),
                ModernNodeValue::Double(shadow),
                ModernNodeValue::Double(illumination),
            ) => Ok(Some(UvLightSettings::new(position, shadow, illumination))),
            _ => Err(PointwiseError::InvalidCast),
        }
    }

    pub(super) fn uv_projection_at(
        &self,
        time: f64,
    ) -> Result<Option<Projection>, PointwiseError<E>> {
        let Payload::UvTexture { kind, properties } = &self.core().payload else {
            return Ok(None);
        };
        let get = |index: usize| {
            ModernNodeProperty::from_root(self.clone(), properties[index].clone())
                .evaluated_value(time)
        };
        let vector = |index| match get(index)? {
            ModernNodeValue::Vector3D(value) => Ok(value),
            _ => Err(PointwiseError::InvalidCast),
        };
        let integer = |index| match get(index)? {
            ModernNodeValue::Int32(value) => Ok(value),
            _ => Err(PointwiseError::InvalidCast),
        };
        let boolean = |index| match get(index)? {
            ModernNodeValue::Boolean(value) => Ok(value),
            _ => Err(PointwiseError::InvalidCast),
        };
        let (mode, tile) = match kind {
            UvTextureKind::Apply
            | UvTextureKind::AmbientOcclusion
            | UvTextureKind::Curvature
            | UvTextureKind::EnvironmentLight
            | UvTextureKind::EnvironmentOcclusion
            | UvTextureKind::Light
            | UvTextureKind::Thickness => {
                return Ok(None);
            }
            UvTextureKind::MultiProject => {
                let ModernNodeValue::Double(feather) = get(4)? else {
                    return Err(PointwiseError::InvalidCast);
                };
                let multi = MultiProjection::new(integer(3)?, (feather / 100.) as f32)
                    .ok_or(PointwiseError::Argument)?;
                (ProjectionMode::Multi(multi), boolean(5)?)
            }
            UvTextureKind::Project => {
                let mode = match integer(3)? {
                    0 => ProjectionMode::Cylindrical,
                    1 => ProjectionMode::Planar { side: integer(5)? },
                    2 => ProjectionMode::ShrinkWrap,
                    3 => ProjectionMode::Spherical,
                    _ => return Err(PointwiseError::Argument),
                };
                (mode, boolean(4)?)
            }
        };
        Ok(Some(Projection::new(
            vector(0)?,
            vector(1)?,
            vector(2)?,
            mode,
            tile,
        )))
    }
}
