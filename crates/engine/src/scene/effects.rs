//! Scene operations corresponding to the source's `ab2` and `aet` effects.
//! These operate on the retained scene, not an alternate editing graph.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneTransformOrigin {
    World,
    AllModels,
    EachModel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneRotationOrder {
    Beginning,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneTransformParameters {
    scale: SceneVector,
    movement: SceneVector,
    rotation: SceneVector,
    origin: SceneTransformOrigin,
    rotation_order: SceneRotationOrder,
}

impl SceneTransformParameters {
    pub fn new(
        scale: SceneVector,
        movement: SceneVector,
        rotation: SceneVector,
        origin: SceneTransformOrigin,
        rotation_order: SceneRotationOrder,
    ) -> Self {
        Self {
            scale,
            movement,
            rotation,
            origin,
            rotation_order,
        }
    }

    fn matrix(self, center: Option<MeshVector>) -> SceneTransform {
        let mut result = SceneTransform::default();
        if let Some(center) = center {
            result = result.then(SceneTransform::translation(SceneVector::new(
                0.0 - f64::from(center.x),
                0.0 - f64::from(center.y),
                0.0 - f64::from(center.z),
            )));
        }
        let scale = SceneTransform::scale(self.scale);
        let rotation = SceneTransform::orientation(self.rotation);
        result = match self.rotation_order {
            SceneRotationOrder::Beginning => result.then(rotation).then(scale),
            SceneRotationOrder::End => result.then(scale).then(rotation),
        };
        if let Some(center) = center {
            result = result.then(SceneTransform::translation(SceneVector::new(
                f64::from(center.x),
                f64::from(center.y),
                f64::from(center.z),
            )));
        }
        result.then(SceneTransform::translation(self.movement))
    }
}

// awx.a(c1 / IEnumerable<c1>): use the actual readonly position buffers and
// temporary-seal each reached current position store after its scan. Comparisons
// deliberately do not use f32::min/max, which have different NaN behavior.
fn center(models: &[MeshModel]) -> Result<MeshVector, SceneError> {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for model in models {
        let positions = model.positions().ok_or(SceneError::NullReference)?;
        let buffer = positions.data(true)?;
        for index in 0..buffer.len() {
            let point = buffer.read(index as i32)?;
            for (axis, value) in point.values().into_iter().enumerate() {
                if value < min[axis] {
                    min[axis] = value;
                }
                if value > max[axis] {
                    max[axis] = value;
                }
            }
        }
        model
            .positions()
            .ok_or(SceneError::NullReference)?
            .seal(true);
    }
    Ok(MeshVector::new(
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ))
}

impl Scene {
    /// Apply Transform 3D's source order to the actual model and array owners.
    /// Errors retain the already transformed prefix of models and attributes.
    pub fn apply_transform_3d(
        &self,
        parameters: SceneTransformParameters,
    ) -> Result<(), SceneError> {
        let models = self.models();
        match parameters.origin {
            SceneTransformOrigin::EachModel => {
                for model in models {
                    let pivot = center(std::slice::from_ref(&model))?;
                    model.transform(parameters.matrix(Some(pivot)))?;
                }
            }
            origin => {
                let pivot = match origin {
                    SceneTransformOrigin::World => None,
                    SceneTransformOrigin::AllModels => Some(center(&models)?),
                    SceneTransformOrigin::EachModel => unreachable!(),
                };
                let matrix = parameters.matrix(pivot);
                for model in models {
                    model.transform(matrix)?;
                }
            }
        }
        Ok(())
    }

    /// Clear Lights replaces the actual light list with source `aet`'s ambient
    /// light. Each list operation notifies separately and a failing callback
    /// retains the already committed prefix.
    pub fn clear_lights(&self) -> Result<(), SceneError> {
        self.light_list().clear()?;
        let light = SceneLight::new(SceneLightValues::new(
            true,
            0,
            MeshVector::default(),
            [1.; 3],
            [0.; 3],
            [0.; 3],
            [1., 0., 0.],
            MeshVector::default(),
            180.,
            1.,
        ));
        self.light_list().add(light)
    }
}

#[cfg(test)]
mod tests;
