//! Actual y2 owner and generic os custom binding over the existing generator,
//! paired properties, aoi bodies and graph sponsors. No decoded-image input.
#[path = "imported_capture.rs"]
pub(super) mod capture;
use super::*;
use crate::pointwise::property::CustomPropertyBinding;
use crate::{
    PointwiseCustomObject, PointwiseImportedImage, PointwiseRenderOptions, ProceduralExecution,
};

pub(super) struct ImportedState<E> {
    current: Option<PointwiseCustomObject<E>>,
    previous: Option<PointwiseCustomObject<E>>,
    binding: Option<Arc<CustomPropertyBinding<E>>>,
    choices: [Selection; 2],
}
impl<E> ImportedState<E> {
    pub(super) fn new() -> Self {
        let current = PointwiseImportedImage::<E>::new();
        let previous = current.source_clone();
        let selection = |names: &[&str]| Selection {
            current: 1,
            previous: 1,
            options: SharedSelectionOptions::new(
                names
                    .iter()
                    .map(|name| Some(name.encode_utf16().collect()))
                    .collect(),
            ),
        };
        Self {
            current: Some(current.custom_object().raw()),
            previous: Some(previous.custom_object().raw()),
            binding: None,
            choices: [
                selection(&["Yes", "No"]),
                selection(&["None", "Linear", "Cosine", "Cubic"]),
            ],
        }
    }
}
impl<E> GeneratorState<E> {
    pub(super) fn choice(&self, kind: GradientGeneratorParameterKind) -> &Selection {
        match kind {
            GradientGeneratorParameterKind::ForceSquareRatio => {
                &self
                    .imported
                    .as_ref()
                    .expect("typed imported choice")
                    .choices[0]
            }
            GradientGeneratorParameterKind::UpsizeSmoothing => {
                &self
                    .imported
                    .as_ref()
                    .expect("typed imported choice")
                    .choices[1]
            }
            _ => &self.selection,
        }
    }
    pub(super) fn choice_mut(&mut self, kind: GradientGeneratorParameterKind) -> &mut Selection {
        match kind {
            GradientGeneratorParameterKind::ForceSquareRatio => {
                &mut self
                    .imported
                    .as_mut()
                    .expect("typed imported choice")
                    .choices[0]
            }
            GradientGeneratorParameterKind::UpsizeSmoothing => {
                &mut self
                    .imported
                    .as_mut()
                    .expect("typed imported choice")
                    .choices[1]
            }
            _ => &mut self.selection,
        }
    }
}
/// Registered Imported Image graph node. Clone retains node identity; the
/// source_clone method performs the actual source constructor/copy chain.
pub struct ImportedImageNode<E = ()> {
    generator: GradientGenerator<E>,
}
impl<E> Clone for ImportedImageNode<E> {
    fn clone(&self) -> Self {
        Self {
            generator: self.generator.clone(),
        }
    }
}
impl<E> fmt::Debug for ImportedImageNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImportedImageNode({:?})", self.generator)
    }
}
impl<E: 'static> ImportedImageNode<E> {
    pub fn new() -> Result<Self, PointwiseError<E>> {
        Ok(Self {
            generator: GradientGenerator::with_kind(ImageGeneratorKind::ImportedImage)?,
        })
    }
    pub(in crate::pointwise) fn from_node(node: &PointwiseNode<E>) -> Option<Self> {
        let generator = node.image_generator_handle()?;
        (generator.kind() == ImageGeneratorKind::ImportedImage).then_some(Self { generator })
    }
    pub fn node(&self) -> PointwiseNode<E> {
        self.generator.node()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.generator.same_reference(&other.generator)
    }
    pub fn parameters(&self) -> Vec<GradientGeneratorParameter<E>> {
        self.generator.parameters()
    }
    pub fn parameter(&self, kind: GradientGeneratorParameterKind) -> GradientGeneratorParameter<E> {
        self.generator.parameter(kind)
    }
    pub fn source_clone(&self) -> Result<Self, PointwiseError<E>> {
        Ok(Self {
            generator: self.generator.source_clone()?,
        })
    }
    pub fn render_unbound(
        &self,
        width: i32,
        height: i32,
        time: f64,
        options: PointwiseRenderOptions,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.generator
            .render_imported(width, height, time, options, execution)
    }
}
impl<E> GradientGenerator<E> {
    pub(super) fn bind_imported(&self) {
        let binding = PointwiseCustomObject::bind_node(
            &self.node,
            GradientGeneratorParameterKind::Image.ix(),
        );
        let state = self.state();
        let mut state = guard(&state);
        let imported = state
            .imported
            .as_mut()
            .expect("actual Imported Image state");
        if let Some(current) = &imported.current {
            current.subscribe(binding.clone());
        }
        imported.binding = Some(binding);
    }
    pub(super) fn copy_imported_from(&self, source: &Self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let source_state = source.state();
        let choices = {
            let state = guard(&source_state);
            let actual = state.imported.as_ref().ok_or(PointwiseError::InvalidCast)?;
            std::array::from_fn(|i| Selection {
                current: actual.choices[i].current,
                previous: actual.choices[i].previous,
                options: actual.choices[i].options.clone(),
            })
        };
        let mut imported = ImportedState::new();
        imported.choices = choices;
        guard(&self.state()).imported = Some(imported);
        if self.copy_imported_value_from(source)? {
            self.node.merge_region(&source.node);
        }
        self.bind_imported();
        Ok(())
    }

    pub(super) fn copy_imported_value_from(
        &self,
        source: &Self,
    ) -> Result<bool, PointwiseError<E>> {
        // Both the os copy constructor and CloneFrom keep the destination mn
        // and copy its fields through cj. ba then copies the committed value.
        let image = GradientGeneratorParameterKind::Image;
        let Some(current) = source.parameter(image).custom_value()? else {
            return Ok(false);
        };
        let destination = self
            .parameter(image)
            .custom_value()?
            .ok_or(PointwiseError::NullReference)?
            .imported_image()?;
        destination.copy_fields_from(&current.imported_image()?);
        let shares_container = destination.container().is_some();
        let previous = destination.source_clone().custom_object().raw();
        let old = guard(&self.state())
            .imported
            .as_mut()
            .ok_or(PointwiseError::InvalidCast)?
            .previous
            .replace(previous);
        drop(old);
        Ok(shares_container)
    }
    pub(in crate::pointwise) fn render_imported(
        &self,
        width: i32,
        height: i32,
        time: f64,
        options: PointwiseRenderOptions,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_property_evaluation_supported()?;
        let output = SharedImageMap::new(crate::ImageGeometry::new(width, height));
        let object = self
            .parameter(GradientGeneratorParameterKind::Image)
            .custom_value()?
            .ok_or(PointwiseError::NullReference)?;
        let decoded = object
            .imported_image()?
            .image()
            .map_err(PointwiseError::ImportedImage)?;
        let Some(decoded) = decoded else {
            output.set_constant(3, 1.0)?;
            return Ok(output);
        };
        decoded.seal_permanently()?;
        let force = self
            .parameter(GradientGeneratorParameterKind::ForceSquareRatio)
            .effective_text_at(time)?;
        let aspect = !force
            .as_deref()
            .is_some_and(|text| text.iter().copied().eq("Yes".encode_utf16()))
            && decoded.geometry().width() != decoded.geometry().height();
        let smoothing = self
            .parameter(GradientGeneratorParameterKind::UpsizeSmoothing)
            .effective_text_at(time)?;
        let mode = match smoothing.as_deref() {
            Some(text) if text.iter().copied().eq("None".encode_utf16()) => 0,
            Some(text) if text.iter().copied().eq("Linear".encode_utf16()) => 1,
            Some(text) if text.iter().copied().eq("Cosine".encode_utf16()) => 2,
            _ => 3,
        };
        let mut rw = width;
        let mut rh = height;
        if aspect {
            if decoded.geometry().width() > decoded.geometry().height() {
                rh = crate::framework_cast::integer(
                    f64::from(height) * f64::from(decoded.geometry().height())
                        / f64::from(decoded.geometry().width()),
                )
                .max(1);
            } else {
                rw = crate::framework_cast::integer(
                    f64::from(width) * f64::from(decoded.geometry().width())
                        / f64::from(decoded.geometry().height()),
                )
                .max(1);
            }
        }
        let resized =
            crate::pointwise::image_resize::resize_with_mode(&decoded, rw, rh, mode, execution)?;
        let image = if aspect {
            crate::pointwise::imported_render::pad(&resized, width, height, rw, rh, execution)?
        } else {
            resized
        };
        // Source evaluates each property component in order only after padding.
        let size = self.parameter(GradientGeneratorParameterKind::Size);
        let sx = size.value_at(0, time)?;
        let sy = size.value_at(1, time)?;
        let location = self.parameter(GradientGeneratorParameterKind::Location);
        let x = location.raw_value_at(0, time)?;
        let y = location.raw_value_at(1, time)?;
        crate::pointwise::imported_render::affine(
            &image,
            [sx, sy],
            [x, y],
            options.is_channel_rendering(),
            execution,
        )
    }
}
impl<E> GradientGeneratorParameter<E> {
    fn imported_slot(&self) -> Result<(), PointwiseError<E>> {
        if self.owner.kind() == ImageGeneratorKind::ImportedImage
            && self.kind == GradientGeneratorParameterKind::Image
        {
            Ok(())
        } else {
            Err(PointwiseError::NullReference)
        }
    }
    pub fn custom_value(&self) -> Result<Option<PointwiseCustomObject<E>>, PointwiseError<E>> {
        self.imported_slot()?;
        let value = guard(&self.owner.state())
            .imported
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .current
            .clone();
        Ok(value.map(|value| value.external()))
    }
    pub fn previous_custom_value(
        &self,
    ) -> Result<Option<PointwiseCustomObject<E>>, PointwiseError<E>> {
        self.imported_slot()?;
        let value = guard(&self.owner.state())
            .imported
            .as_ref()
            .ok_or(PointwiseError::NullReference)?
            .previous
            .clone();
        Ok(value.map(|value| value.external()))
    }
    pub fn set_custom_value(
        &self,
        value: Option<&PointwiseCustomObject<E>>,
        _temporary: bool,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.imported_slot()?;
        let old = self.custom_value()?;
        if match (&old, value) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_reference(b),
            _ => false,
        } {
            return Ok(());
        }
        let binding = guard(&self.owner.state())
            .imported
            .as_ref()
            .and_then(|s| s.binding.clone())
            .ok_or(PointwiseError::NullReference)?;
        if let Some(old) = &old {
            old.unsubscribe(&binding);
        }
        let old = {
            let state = self.owner.state();
            let mut state = guard(&state);
            std::mem::replace(
                &mut state
                    .imported
                    .as_mut()
                    .ok_or(PointwiseError::NullReference)?
                    .current,
                value.map(PointwiseCustomObject::raw),
            )
        };
        drop(old);
        if let Some(value) = value {
            for owner in value.owner_nodes() {
                self.owner.node.merge_region(&owner);
            }
            value.subscribe(binding.clone());
            value.attach_route(&binding);
        }
        self.imported_custom_event(false)
    }
    pub(in crate::pointwise) fn imported_custom_event(
        &self,
        speed: bool,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.imported_slot()?;
        let handlers = guard(&self.owner.state()).handlers[self.kind.ix()].clone();
        self.owner.notify(
            self.kind,
            handlers,
            if speed {
                Arc::new(ContextChangeArgument::Property(
                    ContextPropertyChangeArgs::with_speed(true),
                ))
            } else {
                ordinary_args()
            },
        )?;
        if !speed {
            let current = self.custom_value()?.ok_or(PointwiseError::NullReference)?;
            let previous = current.source_clone()?.raw();
            let old = {
                let state = self.owner.state();
                let mut state = guard(&state);
                state
                    .imported
                    .as_mut()
                    .ok_or(PointwiseError::NullReference)?
                    .previous
                    .replace(previous)
            };
            drop(old);
        }
        Ok(())
    }
}

impl<E: 'static> GradientGeneratorParameter<E> {
    pub(in crate::pointwise::generator) fn refresh_wire_previous(
        &self,
    ) -> Result<(), PointwiseError<E>> {
        self.imported_slot()?;
        let current = self.custom_value()?.ok_or(PointwiseError::NullReference)?;
        let previous = current.source_clone()?.raw();
        let old = {
            let state = self.owner.state();
            let mut s = guard(&state);
            s.imported
                .as_mut()
                .ok_or(PointwiseError::NullReference)?
                .previous
                .replace(previous)
        };
        drop(old);
        Ok(())
    }
}
