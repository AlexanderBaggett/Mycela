//! Evaluated aoy Pixelate and aip convolution. Actual retained os/graph/property
//! evaluation is separate; these leaves own no made-up context or source state.
#[path = "spatial_matrix.rs"]
mod matrix;
#[path = "spatial_pixelate.rs"]
mod pixelate;
use super::{PointwiseError, SharedImageMap};
use crate::{FloatBuffer, ImageError, MatrixKernelError};
pub use matrix::{MatrixRenderOptions, render_matrix, render_matrix_filter};
pub use pixelate::{PixelateOptions, render_pixelate};
fn read(source: Option<&FloatBuffer>, index: i32) -> Result<f32, ImageError> {
    source
        .ok_or(ImageError::NullReference)?
        .read(usize::try_from(index).map_err(|_| ImageError::Index)?)
}
fn write(destination: Option<&FloatBuffer>, index: i32, value: f32) -> Result<(), ImageError> {
    destination.ok_or(ImageError::NullReference)?.write(
        usize::try_from(index).map_err(|_| ImageError::Index)?,
        value,
    )
}
fn at(x: i32, y: i32, width: i32) -> i32 {
    x.wrapping_add(y.wrapping_mul(width))
}
fn payload<E>(error: MatrixKernelError) -> PointwiseError<E> {
    match error {
        MatrixKernelError::NullReference => PointwiseError::NullReference,
        MatrixKernelError::Index => PointwiseError::Index,
        MatrixKernelError::Allocation => ImageError::Allocation.into(),
        MatrixKernelError::Binary(_) => {
            PointwiseError::UnsupportedCapability("binary payload error in numerical kernel")
        }
    }
}
fn named_tail<E>(out: &SharedImageMap, input: &SharedImageMap) -> Result<(), PointwiseError<E>> {
    // Actual n2: named wrapper/data identity carry; NO RegionsMap copy.
    out.merge_effects(&[Some(input.clone())])?;
    Ok(())
}
/// Source wr.k8 evaluates iterations before input resolution, then the real
/// matrix root, independent angle, and factor-scaled radius in that order.
pub(super) fn retained_wizard_matrix<E: 'static>(
    filter: &super::PointwiseFilter<E>,
    input: Option<&SharedImageMap>,
    geometry: crate::ImageGeometry,
    iterations: i32,
    time: f64,
    execution: crate::ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    if iterations == 0 {
        return Ok(input.cloned());
    }
    let kernel = filter
        .parameter(0)?
        .matrix()?
        .map(|object| object.payload());
    let angle = filter.parameter(3)?.effective_value_at(time)? / 180.0 * std::f64::consts::PI;
    let factor = filter.factor();
    let radius = filter.parameter(2)?.effective_value_at(time)? / factor;
    matrix::render_matrix(
        input,
        kernel.as_ref(),
        &MatrixRenderOptions::new(iterations, angle, radius, execution),
        geometry,
    )
    .map(Some)
}

#[cfg(test)]
#[path = "spatial_test.rs"]
mod tests;

/// Live os getter seam: allocation/input/empty checks stay in the shared leaf.
pub(super) fn retained_pixelate<E: 'static>(
    filter: &super::PointwiseFilter<E>,
    input: Option<&SharedImageMap>,
    geometry: crate::ImageGeometry,
    time: f64,
    execution: crate::ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    pixelate::render_with_inputs(
        input,
        geometry,
        execution,
        &mut || {
            let factor = filter.factor();
            let p = filter.parameter(0)?;
            let x = p.value_at(0, time)? * factor;
            let y = p.value_at(1, time)? * factor;
            Ok([x, y])
        },
        &mut || filter.parameter(1)?.effective_index_at(time),
    )
}
pub(super) fn retained_matrix<E: 'static>(
    filter: &super::PointwiseFilter<E>,
    input: Option<&SharedImageMap>,
    geometry: crate::ImageGeometry,
    iterations: i32,
    execution: crate::ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    if iterations == 0 {
        return Ok(input.cloned());
    }
    let kernel = filter
        .parameter(0)?
        .matrix()?
        .ok_or(PointwiseError::NullReference)?
        .payload();
    let angle = kernel.angle() / 180.0 * std::f64::consts::PI;
    let radius = filter
        .parameter(0)?
        .matrix()?
        .ok_or(PointwiseError::NullReference)?
        .payload()
        .radius()
        / filter.factor();
    matrix::render_matrix(
        input,
        Some(&kernel),
        &MatrixRenderOptions::new(iterations, angle, radius, execution),
        geometry,
    )
    .map(Some)
}

#[cfg(test)]
#[path = "wizard_matrix_test.rs"]
mod wizard_matrix_tests;
