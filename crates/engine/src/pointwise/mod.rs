//! Retained pointwise kernels and original-mode CPU slot graphs. Handles retain
//! real image/parameter/cache authority. Scene/group scheduling, clone-mode
//! evaluation, positive property adjustment/ancestor contexts,
//! and positive editor hosts remain required beyond this original-mode seam.
mod adaptive;
mod adaptive_box;
mod art_blur;
mod atmosphere;
mod blur;
mod canvas;
pub use canvas::{
    CanvasAlignment, CanvasAreaEditor, CanvasAreaField, CanvasAreaSettings, CanvasDesignPlan,
    CanvasGridKind, CanvasHandleDrag, CanvasHandleModifiers, CanvasHandlePreview, CanvasItem,
    CanvasItemPlacement, CanvasLayerEditor, CanvasPath, CanvasPathEndpoint, CanvasPathHandle,
    CanvasPathPoint, CanvasPathSegment, CanvasPointKind, CanvasPrimitive, CanvasSelectionEdit,
    CanvasSelectionField, CanvasSelectionTransform, CanvasShapeBounds, CanvasShapeEditor,
    CanvasShapeField, CanvasShapeValues, CanvasSnapGrid, CanvasSymmetry, CanvasTransformFrame,
    CanvasTransformHandle, CanvasTransformOrientation, StyleControl, StyleControlKind, StyleCurve,
    StyleEdit, StyleNoiseScope, StylePaintKind, StyleSection, SynthesisColorField,
    SynthesisColorTarget, SynthesisCropHandle, SynthesisEdit, SynthesisField, SynthesisGeometry,
    SynthesisLayerKind, SynthesisNumberField, SynthesisPairField, SynthesisPatchResizeHandle,
    SynthesisPatchShape, SynthesisPatchTransform, SynthesisPatchTransformBounds,
    SynthesisPatchTransformField, SynthesisPlacementClipboard, SynthesisPlacementField,
    SynthesisPlacementGeometry, SynthesisPlacementTransform,
};
pub(crate) use canvas::{EnvironmentPaintEdit, SynthesisPaint, TextureEdit};
mod caustics;
mod checkerboard;
mod combine;
mod combine_arrays;
pub(crate) mod convert_environment;
pub(crate) mod convert_environment_cross;
#[cfg(test)]
mod convert_environment_test;
mod effect_maps;
pub(crate) mod environment_blur;
mod environment_optics;
mod equalize;
mod generator;
mod generator_kernel;
mod graph;
mod hair;
mod illuminate_g2;
mod lab_conversion;
mod legacy_bricks;
mod make_environment;
mod planar_optics;
mod point_light;
mod regions;
mod scene_modifier;
mod scene_primitive;
mod shadow;
mod shadow_g2;
mod stereogram;
mod template_lab;
pub use template_lab::TemplateLabKind;
mod wizard_color;
pub use scene_modifier::{SceneClearLightsNode, SceneCombineNode, SceneTransformNode};
mod scene_import;
pub use scene_primitive::{
    ModernNodeAnimation, ModernNodeProperty, ModernNodePropertyHandler, ModernNodePropertyKind,
    ModernNodeValue, ScenePrimitiveNode,
};
mod history_binding;
pub(crate) use history_binding::GraphConnectionEdit;
mod transform;
mod transform_distort;
mod transform_free;
mod transform_kaleidoscope;
mod transform_tile;
mod transmit;
pub use history_binding::{
    PointwiseHistorySlot, PointwiseInputGroup, PointwiseInputSlot, PointwiseTextureNodeLogic,
};
mod image;
pub(crate) mod image_resize;
mod imported_render;
mod invert_node;
mod layer_maps;
mod map_arrays;
mod map_variable;
pub use map_variable::MappedVariableValue;
mod match_color;
mod math;
mod median;
mod motion;
mod motion_patterns;
mod motion_sampling;
mod node_wire;
mod numeric;
pub(crate) mod numeric_adjustment;
mod oklab;
pub(crate) mod reference_adjustment;
pub use node_wire::{NodeWireError, NodeWireReadPolicy, create_node_by_identity, load_node_binary};
pub(crate) use node_wire::{NodeWireReadContext, load_node_binary_with_context, load_node_xml};
pub use numeric_adjustment::LabNumericProperty;
pub(crate) use property::wire::AdjustmentCaptureParts;
pub use property::wire::{LivePropertyWireError, SharedPropertyAdjustment};
mod parameter_ops;
pub(crate) use parameter_ops::choice_index_from_numeric;
mod polygon;
mod polygon_decimal;
pub(crate) mod property;
mod region;
mod render;
pub(crate) mod rotate_environment;
mod shape;
mod smear;
pub use render::PointwiseRenderEffects;
mod scene_image;
mod shader;
pub use shader::{DEFAULT_SHADER_SOURCE, ShaderInput, ShaderProgram};
mod uv_texture;
pub use scene_image::SceneImageRenderer;
mod rotate;
pub use rotate::render_rotate;
mod solid;
mod spatial;
pub use spatial::{
    MatrixRenderOptions, PixelateOptions, render_matrix, render_matrix_filter, render_pixelate,
};
mod colorize_g2;
pub use colorize_g2::LegacyColorGradient;
mod exposure_selector;
mod frequency_fractalize;
mod full_light;
mod tone_curve;

use crate::{AnimationError, GradientError, ImageError};
pub use combine::{
    CombineColorOperation, combine_color_arrays, combine_iteration_count,
    combine_iteration_count_with_capacity_query,
};
pub use effect_maps::fix_map_contrast;
pub use generator::{
    GradientCachedRenderMode, GradientFinalClone, GradientGenerator,
    GradientGeneratorChangeHandler, GradientGeneratorParameter, GradientGeneratorParameterKind,
    GradientRenderHandler, GradientRenderSnapshot, ImageGenerator, ImageGeneratorChangeHandler,
    ImageGeneratorKind, ImageGeneratorParameter, ImageGeneratorParameterKind,
    ImageGeneratorRenderSnapshot, ImportedImageNode,
};
pub use graph::{
    PointwiseCachedRenderMode, PointwiseChannelGuarantees, PointwiseFinalGraph, PointwiseNode,
    PointwiseNodeChangeHandler, PointwisePreparationHandler, PointwiseRenderOptions,
    PointwiseRenderOutcome, PointwiseSlot, PointwiseSlotChange, PointwiseSlotChangeHandler,
    RenderedImage, SharedDefaultInputMapping,
};
pub use image::SharedImageMap;
pub use invert_node::{
    PointwiseInvertChangeSnapshot, PointwiseInvertError, PointwiseInvertHandler,
    PointwiseInvertProperty,
};
pub use map_arrays::{MapArrayOperation, combine_map_arrays, interpolate_map_arrays};
pub use math::{
    ArithmeticOperation, ArithmeticWorker, HsbOperation, hsb_arrays, hsb_to_rgb, rgb_to_hsb,
};
pub use point_light::{PointLightArguments, point_light_image};
pub use property::levels_editor::{
    LevelsChannel, LevelsControlEdit, LevelsDraft, LevelsInputHistogram, LevelsPoints,
};
pub use property::{
    EnvironmentRenderOverride, FrozenRenderBatch, FrozenRenderValues, Histogram,
    ImportedImageContainer, ImportedImageDecodeError, ImportedImageReporting,
    MaterializedRenderBatch, PendingFilterRender, PendingGradientRender, PendingNodeRender,
    PointwiseCacheHandler, PointwiseCloneMode, PointwiseCustomObject, PointwiseFilter,
    PointwiseFinalExportCache, PointwiseImportedImage, PointwiseKind, PointwiseMatrix,
    PointwiseMatrixHandler, PointwiseNodeRenderSnapshot, PointwiseParameter,
    PointwiseParameterValue, PointwisePropertyChange, PointwisePropertyHandler, PointwiseRegion,
    PointwiseRenderSnapshot, RenderCaptureSession, RenderFreezeSession,
};
pub use solid::{PointwiseSolidChangeHandler, PointwiseSolidEditError};
use std::fmt;
pub use tone_curve::render_tone_curve;

#[derive(Clone, Debug, PartialEq)]
pub enum PointwiseError<E = ()> {
    Image(ImageError),
    ImportedImage(crate::ImportedImageDecodeError),
    Gradient(GradientError),
    /// Typed failure from an actual retained curve/control, not an animation proxy.
    Curve(std::sync::Arc<crate::CurveError<E>>),
    /// Actual retained animation owner/Curve error, preserving its typed cause.
    Animation(std::sync::Arc<AnimationError<E>>),
    /// Real retained Invert selection failure, without erasing its typed cause.
    Selection(crate::SelectionError<E>),
    /// Original Colorize interpolation wrapper, retaining its worker cause.
    Interpolation(ImageError),
    Scene(crate::SceneError),
    ModernProperty(crate::ModernPropertyError),
    PropertyNotification(crate::ChangeNotificationFailure),
    DecimalRounding(crate::DecimalRoundingError),
    Index,
    ArgumentIndex,
    NullReference,
    Arithmetic,
    DivideByZero,
    Argument,
    ArgumentNull,
    InvalidOperation,
    InvalidCast,
    Exception,
    Observer(E),
    /// Original zl.o wraps a final-clone kernel failure in m5.
    FinalRender(Box<Self>),
    /// The caller retired this render before it could publish an image.
    Cancelled,
    History(Box<crate::HistoryError<E>>),
    /// Caller-native ambient provider failure. This label makes no claim that
    /// an OS error is an original CLR exception or a property observer error.
    CapacityQuery(String),
    TemplateResource(String),
    Shader(String),
    UnsupportedCapability(&'static str),
}
impl<E> From<ImageError> for PointwiseError<E> {
    fn from(value: ImageError) -> Self {
        Self::Image(value)
    }
}
impl<E> From<GradientError> for PointwiseError<E> {
    fn from(value: GradientError) -> Self {
        Self::Gradient(value)
    }
}
impl<E> From<crate::CurveError<E>> for PointwiseError<E> {
    fn from(value: crate::CurveError<E>) -> Self {
        Self::Curve(std::sync::Arc::new(value))
    }
}
impl<E> From<AnimationError<E>> for PointwiseError<E> {
    fn from(value: AnimationError<E>) -> Self {
        Self::Animation(std::sync::Arc::new(value))
    }
}
impl<E> PointwiseError<E> {
    pub(crate) fn is_unsupported_capability(&self) -> bool {
        match self {
            Self::UnsupportedCapability(_) | Self::TemplateResource(_) | Self::Shader(_) => true,
            Self::Scene(error) => error.is_unsupported_capability(),
            Self::ImportedImage(error) => {
                error.source_class().is_none()
                    || matches!(
                        error,
                        crate::ImportedImageDecodeError::Storage(
                            crate::ImportedImageError::UnsupportedCapability(_)
                        )
                    )
            }
            Self::Animation(error) => error.is_unsupported_capability(),
            Self::Curve(error) => {
                matches!(error.as_ref(), crate::CurveError::UnsupportedCapability)
            }
            _ => false,
        }
    }
    pub fn source_class(&self) -> &'static str {
        match self {
            Self::Image(e) => e.source_class(),
            Self::Scene(e) => e.source_class(),
            Self::ModernProperty(e) => e.source_class(),
            Self::PropertyNotification(e) => e.source_class(),
            Self::DecimalRounding(e) => e.source_class(),
            Self::ImportedImage(error) => error.source_class().unwrap_or("UnsupportedCapability"),
            Self::Gradient(e) => e.source_class(),
            Self::Curve(e) => match e.as_ref() {
                crate::CurveError::NullReference => "NullReferenceException",
                crate::CurveError::Index => "IndexOutOfRangeException",
                crate::CurveError::ArgumentIndex => "ArgumentOutOfRangeException",
                crate::CurveError::InvalidBlend | crate::CurveError::InvalidData => "Exception",
                crate::CurveError::Overflow => "OverflowException",
                crate::CurveError::Binary(e) => e.source_class(),
                crate::CurveError::Observer(_) => "ObserverError",
                crate::CurveError::UnsupportedCapability => "UnsupportedCapability",
                crate::CurveError::WrongControl => "unproved-Rust-kind-error",
                crate::CurveError::Allocation => "unproved-allocation-error",
            },
            Self::Animation(e) => e.source_class(),
            Self::Selection(crate::SelectionError::Index) => "IndexOutOfRangeException",
            Self::Selection(crate::SelectionError::NullReference) => "NullReferenceException",
            Self::Selection(crate::SelectionError::InvalidCast) => "InvalidCastException",
            Self::Selection(crate::SelectionError::UnrecognizedText) => "Exception",
            Self::Selection(crate::SelectionError::Observer(_)) => "ObserverError",
            Self::Index => "IndexOutOfRangeException",
            Self::ArgumentIndex => "ArgumentOutOfRangeException",
            Self::NullReference => "NullReferenceException",
            Self::Arithmetic => "ArithmeticException",
            Self::DivideByZero => "DivideByZeroException",
            Self::Argument => "ArgumentException",
            Self::ArgumentNull => "ArgumentNullException",
            Self::InvalidCast => "InvalidCastException",
            Self::InvalidOperation => "InvalidOperationException",
            Self::Exception | Self::Interpolation(_) => "Exception",
            Self::Observer(_) => "ObserverError",
            Self::FinalRender(_) => "m5",
            Self::Cancelled => "Cancelled",
            Self::CapacityQuery(_) => "CapacityQueryError",
            Self::History(_) => "HistoryBindingError",
            Self::UnsupportedCapability(_) | Self::TemplateResource(_) => "UnsupportedCapability",
            Self::Shader(_) => "ShaderError",
        }
    }
}

pub(crate) fn check_render_cancelled<E>(
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    if cancelled() {
        Err(PointwiseError::Cancelled)
    } else {
        Ok(())
    }
}
impl<E: fmt::Debug> fmt::Display for PointwiseError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl<E: fmt::Debug + 'static> std::error::Error for PointwiseError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Image(e) | Self::Interpolation(e) => Some(e),
            Self::Gradient(e) => Some(e),
            Self::Animation(e) => Some(e.as_ref()),
            Self::Curve(e) => Some(e.as_ref()),
            Self::Selection(e) => Some(e),
            _ => None,
        }
    }
}

fn guard<T>(value: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
// Original ordered System.Math comparisons return the second operand on
// equality and propagate NaN, unlike Rust's non-NaN min/max selection.
pub(crate) fn min32(a: f32, b: f32) -> f32 {
    if a < b || a.is_nan() { a } else { b }
}
pub(crate) fn max32(a: f32, b: f32) -> f32 {
    if a > b || a.is_nan() { a } else { b }
}
pub(crate) fn min64(a: f64, b: f64) -> f64 {
    if a < b || a.is_nan() { a } else { b }
}
pub(crate) fn max64(a: f64, b: f64) -> f64 {
    if a > b || a.is_nan() { a } else { b }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod imported_image_test;

#[cfg(test)]
mod solid_test;

#[cfg(test)]
mod generator_test;

#[cfg(test)]
mod invert_node_test;

#[cfg(test)]
mod effect_maps_test;

#[cfg(test)]
mod graph_options_test;

#[cfg(test)]
mod layer_maps_test;

#[cfg(test)]
mod tone_curve_test;

#[cfg(test)]
mod combine_test;

#[cfg(test)]
mod combine_capacity_test;

pub use canvas::{SynthesisCatalogValue, SynthesisRecord};
