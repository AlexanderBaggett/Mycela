//! Portable engine behavior characterized against the supplied Genetica assemblies.
//!
//! Current leaves are packed BGRA, numeric RGB/HSL color, in-memory image channels
//! and detached stored-value Solid Color rendering. The full texture engine and
//! editor remain in progress. Binary document primitives support the upcoming
//! property/curve/preset reader. Curve wire records preserve built-in data;
//! they do not establish sampling, editing or complete format support.
//! Gradient and property-wrapper records preserve retained/lazy wire state.
//! Detached Solid Color node writing preserves resolved values and metadata;
//! fixed Solid Color body/property loading preserves default source behavior.
//! Contributor wire records retain UTF-16 getter values, validated ticks and
//! source trim/partial-write behavior. Complete document dispatch/loading and
//! graph evaluation remain in progress. Limits/range records preserve exact wire
//! fields, partial mutation and retained nonwire BigIncrement. Detached scalar,
//! selection, dual, Color and vector adjuster values preserve source metadata
//! and partial wire behavior; full catalog dispatch remains in progress.

mod scene;
pub use scene::import::ImportedMesh;
pub use scene::uv_light::{UvLightSettings, UvLightShadow};
pub use scene::{
    MeshArray, MeshArrayElement, MeshAttribute, MeshBuffer, MeshIndexBuffer, MeshIndexStore,
    MeshModel, MeshSeal, MeshUv, MeshUvBuffer, MeshUvStore, MeshVector, MeshVectorBuffer,
    MeshVectorStore, PolygonGroup, PolygonIndexKey, PolygonIndexKeys, PolygonIndexKeysIter,
    PolygonTopology, Scene, SceneAllocationEffects, SceneAllocationRequest, SceneArrayKind,
    SceneChange, SceneChangeHandler, SceneError, SceneLight, SceneLightValues, SceneList,
    SceneListHandler, SceneMaterial, ScenePrimitiveKind, ScenePrimitiveParameters,
    SceneRotationOrder, SceneTransform, SceneTransformOrigin, SceneTransformParameters,
    SceneVector, combine_3d, generate_primitive,
};
mod binary;
mod bitmap;
mod change_notifications;
mod color;
mod color_binding;
mod color_input_events;
mod color_region;
mod color_swatch;
mod contributor_record;
mod curve_record;
mod curve_region;
mod decimal_arithmetic;
mod double_rounding;
mod flame_color_settings_record;
mod flame_postprocess_record;
mod flame_transform_record;
mod framework_cast;
mod gradient_animation;
mod history;
mod image_decode;
mod image_map;
pub use image_decode::{ImageDecodeError, decode_hdr, decode_pfm, decode_tga, probe_tga};
mod invert;
mod limits_equality;
mod limits_lifecycle;
mod limits_record;
mod live_curve;
mod live_double_property;
mod live_gradient;
mod live_solid_color;
mod matrix_arithmetic;
mod matrix_wire_record;
mod modern_property_record;
mod ordinary_events;
mod parameter_record_store;
pub use bitmap::{Bitmap, BitmapError, BitmapFormat, BitmapLayout, BitmapView};
mod preview_bitmap;
mod property_change_arguments;
mod property_prefix;
mod property_record;
mod reference_adjuster_record;
mod retained_color_payload;
mod rgb_color;
mod seeded_random;
mod selection;
mod selection_region;
mod solid_color;
mod solid_color_record;
mod source_float;
mod value_adjuster_record;
mod variable_spec;
pub use variable_spec::{
    NativeVariableFiles, VARIABLE_COLOR, VARIABLE_ENVIRONMENT, VARIABLE_INACTIVE, VARIABLE_NUMERIC,
    VARIABLE_SELECTION, VARIABLE_TEXTURE_SYNTHESIS, VariableFileService, VariableSpec,
    VariableSpecError,
};

pub use binary::{BinaryCursor, BinaryError, BinaryWriteObservation, BinaryWriter, DecimalBits};
pub use change_notifications::{
    ChangeDelegate, ChangeDispatchError, ChangeHandler, ChangeNotificationFailure,
    ChangeNotificationSnapshot, ChangeNotifier,
};
pub use color::{ChannelIndexError, ColorBgra};
pub use color_binding::{ColorBindingEffects, ColorBindingTimer, SharedColorBinding};
pub use color_input_events::{ColorInputDelegate, ColorInputEventArgs, ColorInputHandler};
pub use color_region::SharedColorInput;
/// Concrete default callback context for unconstrained input construction.
pub type ColorInput = SharedColorInput<DefaultContext>;
pub use color_swatch::{ColorSwatchError, ColorSwatchValue};
pub use contributor_record::{ContributorRecord, ContributorRecordError, ContributorTicks};
mod document_contributors;
pub use curve_record::{
    CurveControlRecord, CurveControlValue, CurveKind, CurveLayerRecord, CurvePointRecord,
    CurveRecord, CurveRecordError,
};
pub use decimal_arithmetic::DecimalArithmeticError;
pub use document_contributors::{
    Contributor, ContributorEffects, ContributorError, ContributorProfile, DocumentContributors,
};
pub use double_rounding::{DecimalRoundingError, round_decimal_places};
pub use flame_color_settings_record::FlameColorSettingsRecord;
pub use flame_postprocess_record::FlamePostprocessRecord;
pub use flame_transform_record::{
    FlameTransformError, FlameTransformRecord, FlameVariationWire, SharedMatrixRecord,
};
pub use image_map::{
    FloatBuffer, ImageError, ImageGeometry, ImageMap, NamedChannelSource, SharedFloatStore,
    SharedNamedChannel,
};
pub use invert::{InvertFloatWorker, StoredInvert};
pub use limits_lifecycle::{
    LimitsArithmeticProfile, LimitsDefinition, LimitsEdit, LimitsStep, LimitsValues, SharedLimits,
};
pub use limits_record::{CurveRangeRecord, LimitsRecord, RangeOptionsRecord, RangeRecordError};
pub use live_curve::{
    AnalyticCurveKind, CurveBoundsScaleError, CurveCache, CurveCachePoint, CurveControlDelegate,
    CurveControlHandle, CurveControlHandler, CurveControlKind, CurveError, CurveLayerHandle,
    CurveLayerKind, CustomPointHandle, CustomPointList, SharedCurve,
};
pub use live_double_property::{
    DoubleCallbackError, DoublePropertyDelegate, DoublePropertyError, DoublePropertyHandler,
    DoublePropertySnapshot, SharedDoubleProperty,
};
pub use live_solid_color::{
    LiveSolidColorError, SharedSolidColorNode, SharedSolidColorProperty, SolidColorCallbackError,
    SolidColorCloneMode, SolidColorNodeDelegate, SolidColorNodeHandler, SolidColorNodeSnapshot,
    SolidColorPropertyDelegate, SolidColorPropertyHandler, SolidColorPropertySnapshot,
};
pub use matrix_arithmetic::{MatrixOperation, NullMatrixPoints, matrix_coordinate_i32};
pub use matrix_wire_record::{MatrixWireError, MatrixWireRecord};
pub use modern_property_record::{
    ModernAdjustmentConfiguration, ModernPropertyConfiguration, ModernPropertyError,
    ModernPropertyRecord, ModernPropertyValue,
};
pub use parameter_record_store::{
    ParameterCallbackError, ParameterObserverError, ParameterPropertyId, ParameterRecordError,
    ParameterRecordStore, ParameterRegistrySnapshot, ParameterSetDelegate, ParameterSetHandler,
    ParameterSetId, ParameterSlotSnapshot, ParameterStore, ParameterStoreError,
    ParameterValueCursor, SharedParameterSet,
};
pub use preview_bitmap::{
    PreviewBitmap, PreviewBitmapError, PreviewBitmapFormat, PreviewBitmapOptions, preview_bitmap,
    preview_scalar_byte,
};
pub use property_change_arguments::{
    ChangeArgument, ColorUndoInfo, ColorUndoObjectValue, ContextChangeArgument,
    ContextColorUndoInfo, ContextPropertyChangeArgs, ContextUndoInfo, DoubleUndoInfo,
    PairUndoArray, PairUndoInfo, PropertyChangeArgs, SourceArgumentRoot, UndoInfo, UndoObjectValue,
};
pub use property_prefix::property_prefix_en_us;
pub use property_record::{
    AdjustmentComponentRecord, AdjustmentRecord, AnimationComponentRecord, AnimationRecord,
    GradientColorPoint, GradientRecord, GradientTransparencyPoint, PropertyRecordError,
};
pub use reference_adjuster_record::{
    ReferenceAdjusterError, ReferenceAdjusterKind, ReferenceAdjusterPayload,
    ReferenceAdjusterRecord, ReferenceValueHandle, SharedCurveRangeRecord, SharedCurveRecord,
    SharedGradientRecord, SharedRangeOptionsRecord, SharedReference,
};
pub use retained_color_payload::{ColorValue, ContextColorValue, DefaultContext};
pub use rgb_color::{HslColor, RgbChannelError, RgbColor};
pub use solid_color::{SolidColorChannelError, StoredSolidColor};
pub use solid_color_record::{
    PropertyBindingRecord, SolidColorProperty, SolidColorPropertyMetadata, SolidColorRecordError,
    SolidColorWireRecord,
};
pub use value_adjuster_record::{
    AdjusterMetadataRecord, NumberAdjusterRecord, NumericAdjusterMetadata, SelectionOptionRecord,
    ValueAdjusterError, ValueAdjusterKind, ValueAdjusterPayload, ValueAdjusterRecord,
    VectorAdjusterRecord,
};

pub use selection::{
    InvertRenderSnapshot, SelectionCloneMode, SelectionDelegate, SelectionError,
    SelectionEventArgs, SelectionHandler, SelectionRenderError, SelectionSender, SelectionSnapshot,
    SelectionStamp, SelectionValue, SharedInvertNode, SharedSelectionOptions,
    SharedSelectionProperty,
};

pub use gradient_animation::{
    AnimationCurves, AnimationError, AnimationFlags, AnimationLimits, GradientAnimation,
};
pub use live_gradient::{
    AlphaMixingPoint, ColorMixingPoint, GradientCache, GradientEntry, GradientError,
    GradientInteger, GradientPointList, SharedGradient,
};

mod source_object;
pub use source_object::SourceObject;
mod memory_writer;
pub use memory_writer::{MemoryWriterError, RetainedBinaryWriter, RetainedMemoryStream};

mod memory_capacity;
pub use memory_capacity::{
    MemoryAddressSpace, MemoryCapacityEffects, MemoryCapacityError, MemoryCapacityPolicy,
};

mod pointwise;
pub use image_map::ImageChannelObservation;
pub use pointwise::{
    ArithmeticOperation, ArithmeticWorker, CombineColorOperation, EnvironmentRenderOverride,
    FrozenRenderBatch, FrozenRenderValues, GradientCachedRenderMode, GradientFinalClone,
    GradientGenerator, GradientGeneratorChangeHandler, GradientGeneratorParameter,
    GradientGeneratorParameterKind, GradientRenderHandler, GradientRenderSnapshot, Histogram,
    HsbOperation, ImageGenerator, ImageGeneratorChangeHandler, ImageGeneratorKind,
    ImageGeneratorParameter, ImageGeneratorParameterKind, ImageGeneratorRenderSnapshot,
    ImportedImageContainer, ImportedImageDecodeError, ImportedImageNode, ImportedImageReporting,
    LegacyColorGradient, LevelsChannel, LevelsControlEdit, LevelsDraft, LevelsInputHistogram,
    LevelsPoints, LivePropertyWireError, MapArrayOperation, MappedVariableValue,
    MaterializedRenderBatch, NodeWireError, NodeWireReadPolicy, PendingFilterRender,
    PendingGradientRender, PendingNodeRender, PendingThumbnailRender, PointLightArguments,
    PointwiseCacheHandler, PointwiseCachedRenderMode, PointwiseChannelGuarantees,
    PointwiseCloneMode, PointwiseCustomObject, PointwiseError, PointwiseFilter,
    PointwiseFinalExportCache, PointwiseFinalGraph, PointwiseImportedImage,
    PointwiseInvertChangeSnapshot, PointwiseInvertError, PointwiseInvertHandler,
    PointwiseInvertProperty, PointwiseKind, PointwiseMatrix, PointwiseMatrixHandler, PointwiseNode,
    PointwiseNodeChangeHandler, PointwiseNodeRenderSnapshot, PointwiseParameter,
    PointwiseParameterValue, PointwisePreparationHandler, PointwisePropertyChange,
    PointwisePropertyHandler, PointwiseRegion, PointwiseRenderEffects, PointwiseRenderOptions,
    PointwiseRenderOutcome, PointwiseRenderSnapshot, PointwiseSlot, PointwiseSlotChange,
    PointwiseSlotChangeHandler, PointwiseSolidChangeHandler, PointwiseSolidEditError,
    PointwiseThumbnailCache, PointwiseThumbnailRender, RenderCaptureSession, RenderFreezeSession,
    RenderedImage, SharedDefaultInputMapping, SharedImageMap, SharedPropertyAdjustment,
    TemplateLabKind, combine_color_arrays, combine_iteration_count,
    combine_iteration_count_with_capacity_query, combine_map_arrays, create_node_by_identity,
    fix_map_contrast, hsb_arrays, hsb_to_rgb, interpolate_map_arrays, load_node_binary,
    point_light_image, render_tone_curve, rgb_to_hsb,
};

mod document_container;
mod environment_record;
pub use document_container::{
    ColorAdjusterLink, CurveAdjuster, CurveAdjusterLink, EnvironmentAdjuster,
    EnvironmentAdjusterLink, EnvironmentCatalogValue, EnvironmentColorLink, EnvironmentMapTarget,
    EnvironmentNumericMapping, FilterPreviewSample, GradientAdjusterLink, Gtx375GraphDocument,
    Gtx375Header, Gtx375WireDocument, GtxBranch, GtxCatalogRecord, GtxGraphError,
    GtxGraphLoadFailure, GtxIncompatibility, GtxLoadFailure, GtxPresetCollection, GtxPresetError,
    GtxPresetGroup, GtxPresetKind, GtxPresetRecord, GtxPresetValue, GtxReadError,
    LabAdjusterMetadata, LabColorAdjuster, LabColorValue, LabNumericAdjuster, LabNumericAxis,
    LabNumericLink, LabNumericMapping, LabNumericRange, LabNumericValue, LabPreset,
    LabPresetOperation, LabPresetSection, LabScalarDefinition, NumericCatalogValue,
    OverviewPresetMode, StyleSynthesisNumberMapping, StyleSynthesisNumberTarget, SynthesisAdjuster,
    SynthesisAdjusterLink, SynthesisDraftPreviewTarget, SynthesisPairTarget, UnsupportedGtx,
};
pub use environment_record::{
    EnvironmentBlendMode, EnvironmentColorField, EnvironmentDisplayTone, EnvironmentEditError,
    EnvironmentField, EnvironmentImage, EnvironmentImageProjection, EnvironmentLayerKind,
    EnvironmentLayerPreview, EnvironmentLight, EnvironmentMaskFill, EnvironmentMaskHemisphere,
    EnvironmentNumericReceiver, EnvironmentPreset, EnvironmentPreviewConverter, EnvironmentRecord,
    EnvironmentRenderSettings, EnvironmentResourceLibrary, EnvironmentTransformFilter,
    EnvironmentWireError,
};
pub use pointwise::LabNumericProperty;
pub use pointwise::{
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

// Procedural generator leaves; retained node/property/editor integration follows.
mod procedural_bricks;
pub use procedural_bricks::{BricksRenderOptions, render_bricks};
mod procedural_cells;
mod procedural_lattice;
mod procedural_noise;
mod procedural_sampling;
mod procedural_schedule;
mod procedural_sites;
pub use procedural_cells::{
    CellsRenderOptions, WoodRingsRenderOptions, render_cells, render_wood_rings,
};
pub use procedural_lattice::{SeededLattice, coordinate_hash as procedural_coordinate_hash};
pub use procedural_noise::{NoiseOptions, render_noise_with_preflight};
pub use procedural_sampling::resample_noise;
pub use procedural_schedule::ProceduralExecution;

mod matrix_kernel;
pub use matrix_kernel::{MatrixCoefficients, MatrixKernel, MatrixKernelError};
mod encoded_image;
pub use encoded_image::{
    ByteBuffer, EncodedByteStore, EncodedStorageEffects, EncodedStorageEnvironment,
    EncodedStorageNotice, EncodedStorageSnapshot, ImportedImageError, ImportedImageSaveError,
    ImportedImageWrite,
};
pub use pointwise::{
    MatrixRenderOptions, PixelateOptions, render_matrix, render_matrix_filter, render_pixelate,
    render_rotate,
};

pub use history::{
    ContributorHistoryDocument, ContributorHistoryHost, ContributorHistoryServices, History,
    HistoryCommand, HistoryCommandList, HistoryDirection, HistoryEffect, HistoryError,
    HistoryPropertyTarget, HistoryServices, HistoryTarget, HistoryTimestamp,
    UnavailableHistoryServices, WorkspaceHistoryRouter,
};

pub use property_change_arguments::{ChoiceUndoAnchor, ChoiceUndoInfo, ChoiceUndoValue};

pub use pointwise::{
    PointwiseHistorySlot, PointwiseInputGroup, PointwiseInputSlot, PointwiseTextureNodeLogic,
};

pub use pointwise::{
    ModernNodeAnimation, ModernNodeProperty, ModernNodePropertyHandler, ModernNodePropertyKind,
    ModernNodeValue, SceneClearLightsNode, SceneCombineNode, ScenePrimitiveNode,
    SceneTransformNode,
};
pub use property_change_arguments::{
    AnimationPropertyChangeArgs, AnimationUndoInfo, EnvironmentUndoInfo, ModernPropertyChangeArgs,
    ModernPropertyUndoInfo, ModernUndoValue,
};

// Execution-only scene backend; retained models remain engine-owned.
pub use pointwise::SceneImageRenderer;
pub use scene::{
    CameraChange, CameraDelegate, CameraError, CameraFrustumBound, CameraHandler,
    CameraMatrixFactory, CameraNormalTransform, CameraOrbitComponent, CameraTransform, SceneCamera,
    WeakSceneCamera,
};

pub use pointwise::{DEFAULT_SHADER_SOURCE, ShaderInput, ShaderProgram};

pub use pointwise::{SynthesisCatalogValue, SynthesisRecord};
