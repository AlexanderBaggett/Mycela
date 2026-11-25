//! Scene combination at the source `aws.k5` boundary.
//!
//! The caller evaluates both scene slots, in order, and provides the scenes
//! returned by that process. `aqr.g` performs the source scene-copy before this
//! boundary; this operation does not replace that distinct ownership step.
use super::{Scene, SceneError};

/// Combine already-evaluated scene operands without copying their model or
/// light roots. With two scenes the first is the actual result and is changed
/// before any light work is attempted. Each second-side list is snapshotted
/// separately immediately before its corresponding batch append, allowing the
/// first Changed callback to affect the subsequent light snapshot.
///
/// List wrapper identity is outside this API. Allocation and Changed callback
/// failures from each snapshot/append are retained as reached.
pub fn combine_3d(
    first: Option<Scene>,
    second: Option<Scene>,
) -> Result<Option<Scene>, SceneError> {
    match (first, second) {
        (None, None) => Ok(None),
        (Some(scene), None) | (None, Some(scene)) => Ok(Some(scene)),
        (Some(first), Some(second)) => {
            let models = second.try_models_snapshot()?;
            first.append_models_snapshot(&models)?;
            let lights = second.try_lights_snapshot()?;
            first.append_lights_snapshot(&lights)?;
            Ok(Some(first))
        }
    }
}

#[cfg(test)]
mod tests;
