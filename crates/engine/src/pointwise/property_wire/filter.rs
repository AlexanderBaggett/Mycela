use super::*;
impl<E> PointwiseParameter<E> {
    pub(crate) fn raw_value(&self) -> PointwiseParameterValue<E> {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .value
            .clone()
    }
    pub(crate) fn wire_metadata(&self) -> LegacyWireMetadata<E> {
        guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
            .extras
            .wire
            .clone()
    }
    pub fn binding_index(&self) -> i32 {
        self.wire_metadata().binding_index()
    }
    pub fn binding_name(&self) -> Option<Vec<u16>> {
        self.wire_metadata().binding_name()
    }
    pub fn set_binding_index(&self, index: i32) {
        self.wire_metadata().set_binding_index(index);
    }
    pub fn set_binding_name(&self, name: Option<&[u16]>) {
        self.wire_metadata().set_binding_name(name);
    }
    pub fn is_animatable(&self) -> bool {
        self.wire_metadata().animatable()
    }
    pub fn is_adjustable(&self) -> bool {
        self.wire_metadata().adjustable()
    }
    pub fn retained_adjustment(&self) -> Option<SharedPropertyAdjustment<E>> {
        self.wire_metadata().retained_adjustment()
    }
}
impl<E: 'static> PointwiseFilter<E> {
    pub fn ensure_property_evaluation_supported(&self) -> Result<(), PointwiseError<E>> {
        for p in self.parameters() {
            p.ensure_property_evaluation_supported()?;
        }
        Ok(())
    }
}
impl<E: 'static> PointwiseParameter<E> {
    pub(crate) fn new_environment_adjustment(
        &self,
        name: &[u16],
        group_id: [u8; 16],
    ) -> Result<SharedPropertyAdjustment<E>, LivePropertyWireError<E>> {
        self.wire_metadata()
            .new_reference_adjustment(9, name, group_id)
    }

    pub(crate) fn new_curve_adjustment(
        &self,
        name: &[u16],
        group_id: [u8; 16],
    ) -> Result<SharedPropertyAdjustment<E>, LivePropertyWireError<E>> {
        self.wire_metadata()
            .new_reference_adjustment(8, name, group_id)
    }

    /// Source os.k(int): publish the binding index before refreshing its value.
    pub fn set_binding_index_notifying(&self, index: i32) -> Result<(), PointwiseError<E>> {
        if self.binding_index() != index {
            self.set_binding_index(index);
            self.refresh_binding_after_edit()?;
        }
        Ok(())
    }
    /// Source os.f(string): null and empty are equivalent for change detection.
    pub fn set_binding_name_notifying(
        &self,
        name: Option<&[u16]>,
    ) -> Result<(), PointwiseError<E>> {
        let old = self.binding_name();
        let old_empty = old.as_deref().is_none_or(<[u16]>::is_empty);
        let new_empty = name.is_none_or(<[u16]>::is_empty);
        if old.as_deref() != name && !(old_empty && new_empty) {
            self.set_binding_name(name);
            self.refresh_binding_after_edit()?;
        }
        Ok(())
    }
    pub fn ensure_property_evaluation_supported(&self) -> Result<(), PointwiseError<E>> {
        if matches!(self.raw_value(), PointwiseParameterValue::Environment(_)) {
            return Ok(());
        }
        if self.binding_index() >= 0 && self.animated_binding_supported()? {
            return Ok(());
        }
        self.wire_metadata().ensure_evaluation_supported()
    }
    pub(in crate::pointwise) fn adjustment_spec(
        &self,
    ) -> Result<AdjustmentSpec, LivePropertyWireError<E>> {
        let property_type = match self.raw_value() {
            PointwiseParameterValue::Number { .. } | PointwiseParameterValue::Choice { .. } => 0,
            PointwiseParameterValue::Pair { .. } => 1,
            PointwiseParameterValue::Text(_) => 2,
            PointwiseParameterValue::Variable(_) => 5,
            PointwiseParameterValue::Matrix { .. }
            | PointwiseParameterValue::Region { .. }
            | PointwiseParameterValue::Histogram(_) => 3,
            PointwiseParameterValue::Color(_) => 6,
            PointwiseParameterValue::Gradient(_) => 7,
            PointwiseParameterValue::Curve(_) => 8,
            PointwiseParameterValue::Environment(_) => 9,
        };
        Ok(AdjustmentSpec {
            property_type,
            count: if matches!(self.raw_value(), PointwiseParameterValue::Pair { .. }) {
                2
            } else {
                1
            },
            limits: if matches!(self.raw_value(), PointwiseParameterValue::Color(_)) {
                // os24d0 uses ordinary d, not the separate Color animation limits.
                self.retained_limits()
            } else {
                self.animation_limits()?
            },
        })
    }
    pub fn adjustment(
        &self,
    ) -> Result<Option<SharedPropertyAdjustment<E>>, LivePropertyWireError<E>> {
        let s = self.adjustment_spec()?;
        self.wire_metadata()
            .adjustment(s.property_type, s.count, s.limits.as_ref())
    }
    pub fn save_binary(&self, writer: &mut BinaryWriter) -> Result<(), LivePropertyWireError<E>> {
        self.save_binary_with_layout(writer, false)
    }
    pub(in crate::pointwise) fn save_binary_with_layout(
        &self,
        writer: &mut BinaryWriter,
        project: bool,
    ) -> Result<(), LivePropertyWireError<E>> {
        save_code(writer, &self.name())?;
        match self.raw_value() {
            PointwiseParameterValue::Number { .. } => writer.write_f64(self.number())?,
            PointwiseParameterValue::Color(value) => {
                for component in value.components() {
                    writer.write_f64(component)?;
                }
            }
            PointwiseParameterValue::Pair { .. } => {
                writer.write_f64(self.pair()?[0])?;
                writer.write_f64(self.pair()?[1])?;
            }
            PointwiseParameterValue::Choice { .. } => {
                if let Some((index, _)) = self.remembered_selection() {
                    writer.write_i16(index as i16)?;
                    writer.write_utf16(
                        self.remembered_selection()
                            .and_then(|m| m.1)
                            .as_deref()
                            .unwrap_or_default(),
                    )?;
                } else {
                    writer.write_optional_utf16(self.text()?.as_deref())?;
                }
            }
            PointwiseParameterValue::Text(_) => {
                writer.write_optional_utf16(self.text()?.as_deref())?
            }
            PointwiseParameterValue::Variable(value) => value.save(writer)?,
            PointwiseParameterValue::Gradient(_) => self.gradient()?.save(writer)?,
            PointwiseParameterValue::Curve(_) => self.curve()?.save(writer)?,
            PointwiseParameterValue::Environment(_) => {
                self.effective_environment()?
                    .save(writer)
                    .map_err(LivePropertyWireError::Environment)?;
            }
            PointwiseParameterValue::Matrix { current, .. }
            | PointwiseParameterValue::Region { current, .. } => save_custom(
                current.as_ref().ok_or(PointwiseError::NullReference)?,
                writer,
                project,
            )?,
            PointwiseParameterValue::Histogram(_) => {
                return Err(LivePropertyWireError::UnsupportedCapability(
                    "detached Histogram property has no aoi owner",
                ));
            }
        }
        self.wire_metadata()
            .save_tail(writer, || Ok(Some(self.animation()?)))
    }
    pub fn load_binary(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), LivePropertyWireError<E>> {
        self.load_binary_with_variable_service(
            reader,
            version,
            &[],
            &mut UnavailableVariableFiles(std::marker::PhantomData),
        )
    }
    /// A real host can supply directory enumeration for wildcard selections.
    pub fn load_binary_with_variable_service<
        S: crate::VariableFileService<Error = PointwiseError<E>> + ?Sized,
    >(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        base: &[u16],
        service: &mut S,
    ) -> Result<(), LivePropertyWireError<E>> {
        let legacy = self.owner.kind() == PointwiseKind::ClearMaps
            && self.name() == text("Selection")
            && version < 3.1;
        check_code(reader, &self.name(), legacy)?;
        match self.raw_value() {
            PointwiseParameterValue::Number { .. } => {
                let value = reader.read_f64()?;
                validate_wire_number(value, &self.limits()?)?;
                self.set_number(value)?;
            }
            PointwiseParameterValue::Color(_) => {
                let r = reader.read_f64()?;
                let g = reader.read_f64()?;
                let b = reader.read_f64()?;
                self.set_color(crate::ContextColorValue::unlinked([r, g, b]))?;
            }
            PointwiseParameterValue::Pair { .. } => {
                for axis in 0..2 {
                    let value = reader.read_f64()?;
                    validate_wire_number(value, &self.limits()?)?;
                    self.set_axis(axis, value)?;
                }
            }
            PointwiseParameterValue::Choice { .. } => {
                if self.remembered_selection().is_some() {
                    let index = i32::from(reader.read_i16()?);
                    guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                        .choice_memory
                        .as_mut()
                        .ok_or(PointwiseError::NullReference)?
                        .index = index;
                    let name = reader.read_string()?;
                    guard(&self.owner.core.filters)[self.owner.ix].parameters[self.ix]
                        .choice_memory
                        .as_mut()
                        .ok_or(PointwiseError::NullReference)?
                        .text = Some(name);
                    self.resolve_wire_selection()?;
                } else {
                    let mut name = reader.read_string()?;
                    if legacy {
                        name = match String::from_utf16_lossy(&name).as_str() {
                            "Keep None" => text("None"),
                            "Keep First" => text("First"),
                            "Keep Last" => text("Last"),
                            _ => name,
                        };
                    }
                    self.set_choice_wire_text(&name)?;
                }
            }
            PointwiseParameterValue::Text(_) => self.set_text(Some(&reader.read_string()?))?,
            PointwiseParameterValue::Variable(mut value) => {
                value
                    .load(reader, base, service)
                    .map_err(|error| match error {
                        crate::VariableSpecError::Binary(error) => {
                            LivePropertyWireError::Binary(error)
                        }
                        crate::VariableSpecError::Host(error) => {
                            LivePropertyWireError::Property(error)
                        }
                        crate::VariableSpecError::ConflictingBounds => {
                            LivePropertyWireError::Property(PointwiseError::Exception)
                        }
                    })?;
                self.set_variable(value)?;
            }
            PointwiseParameterValue::Gradient(_) => {
                let value = SharedGradient::new();
                value.load(reader)?;
                self.set_gradient(Some(value))?;
            }
            PointwiseParameterValue::Curve(_) => {
                let value = SharedCurve::new(
                    self.curve()?.minimum(),
                    self.curve()?.maximum(),
                    self.curve()?.is_wrapped(),
                );
                value.load(reader, version)?;
                self.set_curve(Some(value))?;
            }
            PointwiseParameterValue::Environment(_) => {
                let value = crate::EnvironmentRecord::read(reader, version)
                    .map_err(LivePropertyWireError::Environment)?;
                self.set_environment(Arc::new(value))?;
            }
            PointwiseParameterValue::Matrix { current, .. }
            | PointwiseParameterValue::Region { current, .. } => {
                load_custom(
                    current.as_ref().ok_or(PointwiseError::NullReference)?,
                    reader,
                    version,
                )?;
                self.refresh_wire_previous()?;
            }
            PointwiseParameterValue::Histogram(_) => {
                return Err(LivePropertyWireError::UnsupportedCapability(
                    "detached Histogram property has no aoi owner",
                ));
            }
        }
        self.wire_metadata().load_tail(
            reader,
            version,
            || Ok(Some(self.animation()?)),
            || self.adjustment_spec(),
        )
    }
    fn set_choice_wire_text(&self, name: &[u16]) -> Result<(), LivePropertyWireError<E>> {
        if self.text()?.as_deref() == Some(name) {
            return Ok(());
        }
        let options = self.options()?.entries();
        let index = options
            .iter()
            .position(|v| v.as_deref() == Some(name))
            .ok_or_else(|| LivePropertyWireError::UnknownChoice {
                node: self.owner.kind().name(),
                property: String::from_utf16_lossy(&self.name()),
                choice: String::from_utf16_lossy(name),
            })? as i32;
        self.set_index_from_wire(index).map_err(Into::into)
    }
    fn resolve_wire_selection(&self) -> Result<(), PointwiseError<E>> {
        let options = self.options()?.entries();
        let (index, name) = self
            .remembered_selection()
            .ok_or(PointwiseError::NullReference)?;
        let found = if index < options.len() as i32 {
            let at = usize::try_from(index).map_err(|_| PointwiseError::Index)?;
            if options[at] == name {
                Some(index)
            } else {
                None
            }
        } else {
            None
        };
        let value = found
            .or_else(|| options.iter().position(|v| v == &name).map(|i| i as i32))
            .unwrap_or(index);
        {
            let mut s = guard(&self.owner.core.filters);
            if let PointwiseParameterValue::Choice { current, .. } =
                &mut s[self.owner.ix].parameters[self.ix].value
            {
                *current = value;
            }
        }
        self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
            crate::ContextPropertyChangeArgs::with_flags(false, true),
        )))
    }
    fn refresh_wire_previous(&self) -> Result<(), PointwiseError<E>> {
        let current = match self.raw_value() {
            PointwiseParameterValue::Matrix { current, .. }
            | PointwiseParameterValue::Region { current, .. } => {
                current.ok_or(PointwiseError::NullReference)?
            }
            _ => return Err(PointwiseError::NullReference),
        };
        let previous = current.source_clone()?.raw();
        let old = {
            let mut s = guard(&self.owner.core.filters);
            if let PointwiseParameterValue::Matrix { previous: slot, .. }
            | PointwiseParameterValue::Region { previous: slot, .. } =
                &mut s[self.owner.ix].parameters[self.ix].value
            {
                slot.replace(previous)
            } else {
                return Err(PointwiseError::NullReference);
            }
        };
        drop(old);
        Ok(())
    }
}
pub(crate) fn save_custom<E: 'static>(
    object: &PointwiseCustomObject<E>,
    writer: &mut BinaryWriter,
    project: bool,
) -> Result<(), LivePropertyWireError<E>> {
    if object.is_light_position() {
        writer.write_f64(object.light_position_x()?)?;
        writer.write_f64(object.light_position_y()?)?;
    } else if let Ok(fields) = object.legacy_tone_curve() {
        for value in fields {
            writer.write_f32(value)?;
        }
    } else if let Ok(value) = object.legacy_gradient() {
        value.write(writer, project)?;
    } else if let Ok(r) = object.region() {
        r.save(writer)?;
    } else if let Ok(m) = object.matrix() {
        m.payload()
            .save(writer)
            .map_err(LivePropertyWireError::Matrix)?;
    } else if let Some(h) = object.histogram() {
        for i in 0..4 {
            for a in h.arrays() {
                writer.write_f32(a.read(i).map_err(PointwiseError::from)?)?;
            }
        }
    } else {
        object
            .imported_image()?
            .save(writer)
            .map_err(LivePropertyWireError::Imported)?;
    }
    Ok(())
}
pub(crate) fn load_custom<E: 'static>(
    object: &PointwiseCustomObject<E>,
    reader: &mut BinaryCursor<'_>,
    version: f64,
) -> Result<(), LivePropertyWireError<E>> {
    if object.is_light_position() {
        object.set_light_position_x(reader.read_f64()?)?;
        object.set_light_position_y(reader.read_f64()?)?;
    } else if object.legacy_tone_curve().is_ok() {
        object.load_legacy_tone_curve(reader)?;
    } else if object.legacy_color_gradient_summary().is_some() {
        object.load_legacy_gradient(reader)?;
    } else if let Ok(r) = object.region() {
        r.load(reader, version)?;
    } else if let Ok(m) = object.matrix() {
        m.payload()
            .load(reader, version)
            .map_err(LivePropertyWireError::Matrix)?;
    } else if let Some(h) = object.histogram() {
        for i in 0..4 {
            for a in h.arrays() {
                let v = reader.read_f32()?;
                a.write(i, v).map_err(PointwiseError::from)?;
            }
        }
    } else {
        object
            .imported_image()?
            .load(reader, version)
            .map_err(LivePropertyWireError::Imported)?;
    }
    Ok(())
}

struct UnavailableVariableFiles<E>(std::marker::PhantomData<E>);
impl<E> crate::VariableFileService for UnavailableVariableFiles<E>
where
    E: 'static,
{
    type Error = PointwiseError<E>;
    fn files(&mut self, _: &[u16]) -> Result<Vec<Vec<u16>>, Self::Error> {
        Err(PointwiseError::UnsupportedCapability(
            "Wizard variable directory service",
        ))
    }
}
