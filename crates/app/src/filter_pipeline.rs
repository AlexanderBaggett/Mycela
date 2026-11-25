//! Actual ordered rendering, shared by the preview and owned export frames.
use crate::ForegroundError;
use mycela_engine::{
    ImageGeometry, InvertRenderSnapshot, PointwiseError, PointwiseFilter, PointwiseKind,
    PointwiseNodeRenderSnapshot, PointwiseRenderEffects, PointwiseRenderOptions,
    PointwiseRenderSnapshot, SharedImageMap, SharedInvertNode,
};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilterInput {
    Previous,
    Source,
    Filter(u64),
    None,
}
#[derive(Clone)]
pub(crate) struct FilterStage {
    id: u64,
    model: PointwiseFilter<ForegroundError>,
    inputs: [FilterInput; 4],
}
impl FilterStage {
    pub(crate) fn new<const N: usize>(
        id: u64,
        model: PointwiseFilter<ForegroundError>,
        inputs: [FilterInput; N],
    ) -> Self {
        assert!(N <= 4);
        let mut routes = [FilterInput::None; 4];
        routes[..N].copy_from_slice(&inputs);
        Self {
            id,
            model,
            inputs: routes,
        }
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn model(&self) -> &PointwiseFilter<ForegroundError> {
        &self.model
    }
    pub(crate) fn inputs(&self) -> &[FilterInput] {
        &self.inputs[..self.model.node().slots().len()]
    }
    #[cfg(test)]
    pub(crate) fn capture_render(
        &self,
        session: &mut mycela_engine::RenderCaptureSession<ForegroundError>,
        time: f64,
    ) -> Result<CapturedFilterStage, RenderError> {
        // These auxiliary sources are actual graph children and share one detached
        // node memo with the filter; do not infer them from supplied pixels.
        let input_sources = (1..=auxiliary_count(self.model.kind()))
            .map(|slot| {
                self.model
                    .node()
                    .slot(slot as i32)
                    .map_err(failure)?
                    .node()
                    .map(|node| session.capture_node(&node, time))
                    .transpose()
                    .map_err(failure)
            })
            .collect::<Result<Vec<_>, RenderError>>()?;
        Ok(CapturedFilterStage {
            id: self.id,
            input_sources,
            model: session
                .capture_filter_at_time(&self.model, time)
                .map_err(failure)?,
            inputs: self.inputs().to_vec(),
        })
    }
}

pub(crate) struct PendingFilterStage {
    id: u64,
    model: mycela_engine::PendingFilterRender<ForegroundError>,
    input_sources: Vec<Option<mycela_engine::PendingNodeRender<ForegroundError>>>,
    inputs: Vec<FilterInput>,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FrozenFilterValues {
    id: u64,
    inputs: Vec<FilterInput>,
    model: mycela_engine::FrozenRenderValues,
    sources: Vec<Option<mycela_engine::FrozenRenderValues>>,
}
impl FilterStage {
    pub(crate) fn freeze_render(
        &self,
        session: &mut mycela_engine::RenderFreezeSession<ForegroundError>,
        time: f64,
    ) -> Result<PendingFilterStage, RenderError> {
        let input_sources = (1..=auxiliary_count(self.model.kind()))
            .map(|slot| {
                self.model
                    .node()
                    .slot(slot as i32)
                    .map_err(failure)?
                    .node()
                    .map(|node| session.freeze_node(&node, time))
                    .transpose()
                    .map_err(failure)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PendingFilterStage {
            id: self.id,
            input_sources,
            model: session
                .freeze_filter_at_time(&self.model, time)
                .map_err(failure)?,
            inputs: self.inputs().to_vec(),
        })
    }
}
impl PendingFilterStage {
    pub(crate) fn frozen_values(&self) -> FrozenFilterValues {
        FrozenFilterValues {
            id: self.id,
            inputs: self.inputs.clone(),
            model: self.model.frozen_values().clone(),
            sources: self
                .input_sources
                .iter()
                .map(|v| v.as_ref().map(|v| v.frozen_values().clone()))
                .collect(),
        }
    }
    pub(crate) fn complete(
        self,
        batch: &mut mycela_engine::MaterializedRenderBatch<ForegroundError>,
    ) -> Result<CapturedFilterStage, RenderError> {
        let input_sources = self
            .input_sources
            .into_iter()
            .map(|v| v.map(|v| v.complete(batch)).transpose().map_err(failure))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CapturedFilterStage {
            id: self.id,
            input_sources,
            model: self.model.complete(batch).map_err(failure)?,
            inputs: self.inputs,
        })
    }
}

/// Positive geometry for a product render. The checked signed area is also the
/// engine's indexing domain; no arbitrary preview-size or memory budget is imposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderGeometry {
    width: i32,
    height: i32,
}
impl RenderGeometry {
    pub(crate) fn new(width: i32, height: i32) -> Result<Self, RenderError> {
        if width <= 0 || height <= 0 || width.checked_mul(height).is_none() {
            return Err(RenderError::InvalidGeometry);
        }
        let cells = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)));
        if cells.and_then(|n| n.checked_mul(4)).is_none() {
            return Err(RenderError::InvalidGeometry);
        }
        Ok(Self { width, height })
    }
    pub(crate) fn width(self) -> i32 {
        self.width
    }
    pub(crate) fn height(self) -> i32 {
        self.height
    }
    fn image(self) -> ImageGeometry {
        ImageGeometry::new(self.width, self.height)
    }
}
#[derive(Debug)]
pub(crate) enum RenderError {
    Cancelled,
    InvalidGeometry,
    Render(ForegroundError),
}
impl RenderError {
    pub(crate) fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}
impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Rendering was cancelled."),
            Self::InvalidGeometry => {
                f.write_str("Render dimensions must be positive and fit the image indexing range.")
            }
            Self::Render(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for RenderError {}
pub(crate) fn failure(error: impl fmt::Display) -> RenderError {
    RenderError::Render(ForegroundError::Operation(error.to_string()))
}
pub(crate) fn checkpoint(cancelled: &impl Fn() -> bool) -> Result<(), RenderError> {
    if cancelled() {
        Err(RenderError::Cancelled)
    } else {
        Ok(())
    }
}

/// The snapshot's engine is private and detached; refreshing Layer Maps here
/// changes only this export frame, including its actual remembered choice state.
#[derive(Clone)]
pub(crate) struct CapturedFilterStage {
    id: u64,
    model: PointwiseRenderSnapshot<ForegroundError>,
    input_sources: Vec<Option<PointwiseNodeRenderSnapshot<ForegroundError>>>,
    inputs: Vec<FilterInput>,
}
impl CapturedFilterStage {
    pub(crate) fn same_render_values(&self, other: &Self) -> bool {
        self.id == other.id
            && self.inputs == other.inputs
            && self.model.same_render_values(&other.model)
            && self.input_sources.len() == other.input_sources.len()
            && self
                .input_sources
                .iter()
                .zip(&other.input_sources)
                .all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.same_render_values(b),
                    _ => false,
                })
    }
}
fn auxiliary_count(kind: PointwiseKind) -> usize {
    match kind {
        PointwiseKind::MedianBlur
        | PointwiseKind::SmartBlur
        | PointwiseKind::VariableBlur
        | PointwiseKind::MatchColor => 1,
        PointwiseKind::MotionBlur
        | PointwiseKind::ShapeBlur
        | PointwiseKind::Smear
        | PointwiseKind::ArtBlur
        | PointwiseKind::Distort
        | PointwiseKind::Remap => 2,
        _ => 0,
    }
}

pub(crate) enum RenderStage<'a> {
    Live(&'a FilterStage, f64),
    Captured(&'a CapturedFilterStage),
}
impl RenderStage<'_> {
    fn id(&self) -> u64 {
        match self {
            Self::Live(s, _) => s.id(),
            Self::Captured(s) => s.id,
        }
    }
    fn kind(&self) -> PointwiseKind {
        match self {
            Self::Live(s, _) => s.model().kind(),
            Self::Captured(s) => s.model.kind(),
        }
    }
    fn inputs(&self) -> &[FilterInput] {
        match self {
            Self::Live(s, _) => s.inputs(),
            Self::Captured(s) => &s.inputs,
        }
    }
    fn refresh(&self, image: &SharedImageMap) -> Result<(), RenderError> {
        match self {
            Self::Live(s, _) => s
                .model
                .refresh_layer_map_options(Some(image))
                .map(|_| ())
                .map_err(failure),
            Self::Captured(s) => s
                .model
                .refresh_layer_map_options(Some(image))
                .map(|_| ())
                .map_err(failure),
        }
    }
    fn input_map(
        &self,
        slot: usize,
        geometry: RenderGeometry,
        cancelled: &impl Fn() -> bool,
    ) -> Result<SharedImageMap, RenderError> {
        let role = match (self.kind(), slot) {
            (PointwiseKind::Distort, 1) => "Direction",
            (PointwiseKind::Distort, 2) => "Distance",
            (PointwiseKind::Remap, 1) => "X",
            (PointwiseKind::Remap, 2) => "Y",
            (PointwiseKind::MatchColor, 1) => "Match to",
            (PointwiseKind::ArtBlur, 2) => "Driver",
            (PointwiseKind::Smear, 1) => "Direction",
            (PointwiseKind::Smear, 2) => "Distance",
            (_, 1) => "Radius",
            (PointwiseKind::ShapeBlur, 2) => "Shape",
            _ => "Angle",
        };
        let options = PointwiseRenderOptions::new(false, true);
        let result = match self {
            Self::Live(stage, time) => match stage
                .model
                .node()
                .slot(slot as i32)
                .map_err(failure)?
                .node()
            {
                Some(node) => crate::scene_backend::render_live(
                    &node,
                    geometry.width,
                    geometry.height,
                    *time,
                    cancelled,
                    options,
                ),
                None => return Ok(SharedImageMap::new(geometry.image())),
            },
            Self::Captured(stage) => match &stage.input_sources[slot - 1] {
                Some(node) => crate::scene_backend::render(
                    node,
                    geometry.width,
                    geometry.height,
                    cancelled,
                    options,
                    None,
                ),
                None => return Ok(SharedImageMap::new(geometry.image())),
            },
        };
        result.map_err(|error| match error {
            RenderError::Cancelled => RenderError::Cancelled,
            other => failure(format!("{role} map source: {other}")),
        })
    }

    fn render(
        &self,
        inputs: &[Option<SharedImageMap>],
        geometry: RenderGeometry,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<SharedImageMap>, RenderError> {
        let mut opts = PointwiseRenderOptions::new(false, true);
        if self.kind() == PointwiseKind::FullLight {
            let mut connected = [false; 4];
            for (slot, route) in self.inputs().iter().enumerate() {
                connected[slot] = *route != FilterInput::None;
            }
            opts = opts.with_supplied_slot_presence(connected);
        }
        // These are actual lazy source effects, not fixed-success providers.
        // Policy locks are released before the next kernel. Blur previews use
        // an owned background scene; live compatibility paths share the same
        // capacity/preflight contract and caller-provided sample time.
        let mut capacity = crate::memory_capacity::query;
        let mut preflight = crate::memory_capacity::preflight;
        let mut effects =
            PointwiseRenderEffects::new(&mut capacity, &mut preflight).with_cancel_check(cancelled);
        let execution = crate::export_render::generator_execution();
        match self {
            Self::Live(s, time) => s.model.render_supplied_at_time_with_effects(
                inputs,
                geometry.width,
                geometry.height,
                *time,
                execution,
                opts,
                &mut effects,
            ),
            Self::Captured(s) => s.model.render_with_execution_and_effects(
                inputs,
                geometry.width,
                geometry.height,
                execution,
                opts,
                &mut effects,
            ),
        }
        .map_err(|error| match error {
            PointwiseError::Cancelled => RenderError::Cancelled,
            other => failure(format!("{}: {other}", self.kind().name())),
        })
    }
}
pub(crate) enum RenderInvert<'a> {
    Live(&'a SharedInvertNode<ForegroundError>),
    Captured(&'a InvertRenderSnapshot),
}
impl RenderInvert<'_> {
    fn render(
        &self,
        image: &SharedImageMap,
        geometry: RenderGeometry,
    ) -> Result<SharedImageMap, RenderError> {
        match self {
            Self::Live(model) => model
                .render_shared(Some(image), geometry.width, geometry.height)
                .map_err(|e| failure(format!("{e:?}"))),
            Self::Captured(frame) => frame
                .render_shared(Some(image), geometry.width, geometry.height)
                .map_err(failure),
        }
    }
}
pub(crate) fn render_chain<'a>(
    image: SharedImageMap,
    invert: Option<RenderInvert<'_>>,
    stages: impl IntoIterator<Item = RenderStage<'a>>,
    geometry: RenderGeometry,
    cancelled: &impl Fn() -> bool,
) -> Result<SharedImageMap, RenderError> {
    render_chain_observed(image, invert, stages, geometry, cancelled, &mut Vec::new())
}
pub(crate) fn render_chain_observed<'a>(
    mut image: SharedImageMap,
    invert: Option<RenderInvert<'_>>,
    stages: impl IntoIterator<Item = RenderStage<'a>>,
    geometry: RenderGeometry,
    cancelled: &impl Fn() -> bool,
    layer_inputs: &mut Vec<(u64, SharedImageMap)>,
) -> Result<SharedImageMap, RenderError> {
    checkpoint(cancelled)?;
    let source = image.clone();
    let mut outputs = Vec::<(u64, SharedImageMap)>::new();
    if let Some(invert) = invert {
        image = invert.render(&image, geometry)?;
        checkpoint(cancelled)?;
    }
    for stage in stages {
        checkpoint(cancelled)?;
        let kind = stage.kind();
        if kind == PointwiseKind::LayerMaps {
            layer_inputs.push((stage.id(), image.clone()));
            // Preview already requests the source controls-only refresh at its render size.
            // Export uses the identical path, against its detached property owner.
            stage
                .refresh(&image)
                .map_err(|e| failure(format!("{}: {e}", kind.name())))?;
        }
        let mut inputs = if stage.inputs().len() > 1
            || matches!(
                kind,
                PointwiseKind::FreeTransform
                    | PointwiseKind::Kaleidoscope
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::RotateTile
            ) {
            stage.inputs().iter().enumerate().map(|(slot, route)| {
                if slot > 0 && slot <= auxiliary_count(kind) { return Ok(None); }
                let resolved = match route {
                    FilterInput::Source => source.clone(), FilterInput::Previous => image.clone(),
                    FilterInput::Filter(id) => outputs.iter().find(|(candidate, _)| candidate == id).map(|(_, output)| output.clone()).ok_or_else(|| failure(format!("{} input filter #{id} is unavailable. Choose an earlier filter, source, or no input.", kind.name())))?,
                    FilterInput::None if kind == PointwiseKind::OklabColor => return Ok(None),
                    FilterInput::None => SharedImageMap::new(geometry.image()),
                }; Ok(Some(resolved))
            }).collect::<Result<Vec<_>, RenderError>>()?
        } else {
            vec![Some(image)]
        };
        checkpoint(cancelled)?;
        for (slot, input) in inputs
            .iter_mut()
            .enumerate()
            .take(auxiliary_count(kind) + 1)
            .skip(1)
        {
            // Absence still supplies a separate fresh context map. Real retained
            // topology, not these pixel arrays, selects the kernel radius branch.
            *input = Some(stage.input_map(slot, geometry, cancelled)?);
            checkpoint(cancelled)?;
        }
        checkpoint(cancelled)?;
        image = stage
            .render(&inputs, geometry, cancelled)?
            .ok_or_else(|| failure("The filter returned no image."))?;
        outputs.push((stage.id(), image.clone()));
        checkpoint(cancelled)?;
    }
    Ok(image)
}
