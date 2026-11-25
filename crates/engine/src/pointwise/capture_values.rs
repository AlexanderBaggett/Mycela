//! Immutable comparison operands captured before isolated renderer mutations.
//! Encounter-order IDs preserve alias arrangements without fresh addresses.
use super::*;
pub(crate) struct Comparison<E> {
    pub(super) memory_only: bool,
    curves: crate::live_curve::frozen::CurveValues,
    color_inputs: crate::color_region::ColorInputValues<E>,
    curve_roots: Vec<SharedCurve<E>>,
    gradients: Vec<SharedGradient>,
    points: Vec<crate::GradientEntry>,
    lists: Vec<crate::GradientPointList>,
    objects: Vec<PointwiseCustomObject<E>>,
    arrays: Vec<crate::MatrixCoefficients>,
    imported_containers: Vec<crate::ImportedImageContainer<E>>,
    encoded_stores: Vec<crate::EncodedByteStore>,
    encoded_bytes: Vec<crate::ByteBuffer>,
    images: Vec<SharedImageMap>,
    image_values: crate::image_map::ImageValues,
    options: Vec<SharedSelectionOptions>,
    limits: Vec<SharedLimits>,
    ranges: Vec<SharedRangeOptionsRecord>,
    curve_ranges: Vec<SharedCurveRangeRecord>,
    animations: Vec<GradientAnimation<E>>,
    animation_flags: Vec<crate::AnimationFlags>,
    animation_curves: Vec<crate::AnimationCurves<E>>,
    animation_limits: Vec<crate::AnimationLimits>,
    adjustments: Vec<super::super::wire::SharedPropertyAdjustment<E>>,
}
impl<E> Default for Comparison<E> {
    fn default() -> Self {
        Self {
            memory_only: false,
            curves: crate::live_curve::frozen::CurveValues::default(),
            color_inputs: crate::color_region::ColorInputValues::default(),
            curve_roots: Vec::new(),
            gradients: Vec::new(),
            points: Vec::new(),
            lists: Vec::new(),
            objects: Vec::new(),
            arrays: Vec::new(),
            imported_containers: Vec::new(),
            encoded_stores: Vec::new(),
            encoded_bytes: Vec::new(),
            images: Vec::new(),
            image_values: crate::image_map::ImageValues::default(),
            options: Vec::new(),
            limits: Vec::new(),
            ranges: Vec::new(),
            curve_ranges: Vec::new(),
            animations: Vec::new(),
            animation_flags: Vec::new(),
            animation_curves: Vec::new(),
            animation_limits: Vec::new(),
            adjustments: Vec::new(),
        }
    }
}
impl<E: 'static> Comparison<E> {
    pub(super) fn image(&mut self, image: &Option<SharedImageMap>, out: &mut Vec<u64>) {
        match image {
            None => out.push(0),
            Some(image) => {
                let id = self
                    .images
                    .iter()
                    .position(|old| old.same_reference(image))
                    .unwrap_or_else(|| {
                        self.images.push(image.clone());
                        self.images.len() - 1
                    });
                out.extend([1, id as u64]);
                self.image_values.image(image.capture_fields(), out);
            }
        }
    }

    pub(crate) fn limits(&mut self, l: &SharedLimits, out: &mut Vec<u64>) {
        let id = if let Some(i) = self.limits.iter().position(|v| v.same_reference(l)) {
            i
        } else {
            self.limits.push(l.clone());
            self.limits.len() - 1
        };
        let s = l.snapshot();
        out.extend([id as u64, s.decimal_places as u64, s.used as u64]);
        for d in [s.minimum, s.maximum, s.increment, s.big_increment] {
            out.extend(d.bits().map(u64::from));
        }
        match s.use_range {
            None => out.push(0),
            Some(r) => {
                let id = if let Some(id) = self.ranges.iter().position(|v| v.same_reference(&r)) {
                    id
                } else {
                    self.ranges.push(r.clone());
                    self.ranges.len() - 1
                };
                let r = r.copy_wire_value();
                out.extend([1, id as u64]);
                out.extend(r.minimum.bits().map(u64::from));
                out.extend(r.maximum.bits().map(u64::from));
                out.extend([r.decimal_places as u64, r.multiplier.to_bits()]);
                out.extend(r.flags.map(u64::from));
            }
        }
        match s.curve_range {
            None => out.push(0),
            Some(r) => {
                let id =
                    if let Some(id) = self.curve_ranges.iter().position(|v| v.same_reference(&r)) {
                        id
                    } else {
                        self.curve_ranges.push(r.clone());
                        self.curve_ranges.len() - 1
                    };
                let r = r.copy_wire_value();
                out.extend([1, id as u64]);
                out.extend(r.minimum.bits().map(u64::from));
                out.extend(r.maximum.bits().map(u64::from));
                out.push(r.wrap_y as u64);
            }
        }
    }
    pub(in crate::pointwise) fn object(
        &mut self,
        o: &Option<PointwiseCustomObject<E>>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        let Some(o) = o else {
            out.push(0);
            return Ok(());
        };
        let id = if let Some(i) = self.objects.iter().position(|v| v.same_reference(o)) {
            i
        } else {
            self.objects.push(o.clone());
            self.objects.len() - 1
        };
        out.extend([1, id as u64]);
        if let Ok([x, y]) = o.light_position() {
            out.extend([4, x.to_bits(), y.to_bits()]);
        } else if let Ok(fields) = o.legacy_tone_curve() {
            out.push(6);
            out.extend(fields.map(|value| u64::from(value.to_bits())));
        } else if let Ok(value) = o.legacy_gradient() {
            out.push(7);
            value.render_values(out);
        } else if let Ok(region) = o.region() {
            let (mode, x, y, size) = region.fields();
            out.extend([
                3,
                mode as u64,
                x.to_bits() as u64,
                y.to_bits() as u64,
                size.to_bits() as u64,
            ]);
            out.push(region.capture_owner().is_some() as u64);
        } else if let Ok(m) = o.matrix() {
            let k = m.captured_payload();
            out.extend([0, k.angle().to_bits(), k.radius().to_bits()]);
            match k.coefficients() {
                Some(a) => {
                    let id = if let Some(i) = self.arrays.iter().position(|v| v.same_reference(&a))
                    {
                        i
                    } else {
                        self.arrays.push(a.clone());
                        self.arrays.len() - 1
                    };
                    out.extend([1, id as u64, a.len() as u64]);
                    out.extend(a.values().into_iter().map(f64::to_bits));
                }
                None => out.push(0),
            }
        } else if let Some(h) = o.histogram() {
            out.push(1);
            self.histogram(&h, out);
        } else if let Ok(image) = o.imported_image() {
            out.push(2);
            self.imported_fields(&image, out, &mut Vec::new(), &mut Vec::new())?;
        }
        Ok(())
    }
    fn histogram(&mut self, h: &Histogram, out: &mut Vec<u64>) {
        out.extend([h.selected_channel() as u64, h.is_disabled() as u64]);
        for a in h.arrays() {
            self.image_values.buffer(&a, out);
        }
    }
    fn gradient(
        &mut self,
        g: &Option<SharedGradient>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        let Some(g) = g else {
            out.push(0);
            return Ok(());
        };
        let id = if let Some(i) = self.gradients.iter().position(|v| v.same_reference(g)) {
            i
        } else {
            self.gradients.push(g.clone());
            self.gradients.len() - 1
        };
        out.extend([1, id as u64, g.width() as u64]);
        for list in [g.color_points(), g.alpha_points()] {
            match list {
                Some(list) => {
                    let id =
                        if let Some(i) = self.lists.iter().position(|l| l.same_reference(&list)) {
                            i
                        } else {
                            self.lists.push(list.clone());
                            self.lists.len() - 1
                        };
                    out.extend([1, id as u64, list.len() as u64]);
                    for i in 0..list.len() {
                        let p = list.entry(i as i32)?;
                        let id = if let Some(i) =
                            self.points.iter().position(|v| v.same_reference(&p))
                        {
                            i
                        } else {
                            self.points.push(p.clone());
                            self.points.len() - 1
                        };
                        out.push(id as u64);
                        match p {
                            crate::GradientEntry::Null => out.push(0),
                            crate::GradientEntry::Color(p) => {
                                out.extend([
                                    1,
                                    p.coordinate() as u64,
                                    p.color().is_grayscale() as u64,
                                ]);
                                out.extend(p.color().argb().map(f64::to_bits));
                            }
                            crate::GradientEntry::Alpha(p) => {
                                out.extend([2, p.coordinate() as u64, p.value().to_bits()])
                            }
                            crate::GradientEntry::Integer(p) => out.extend([3, p.value() as u64]),
                        }
                    }
                }
                None => out.push(0),
            }
        }
        Ok(())
    }
    pub(crate) fn color_input(
        &mut self,
        input: Option<crate::SharedColorInput<E>>,
        out: &mut Vec<u64>,
    ) {
        match input {
            None => out.push(0),
            Some(input) => {
                out.push(1);
                let v = self.color_inputs.read(&input);
                out.push(v.len() as u64);
                out.extend(v);
            }
        }
    }
    pub(in crate::pointwise) fn options(
        &mut self,
        options: &SharedSelectionOptions,
        out: &mut Vec<u64>,
    ) {
        let id = if let Some(i) = self.options.iter().position(|v| v.same_reference(options)) {
            i
        } else {
            self.options.push(options.clone());
            self.options.len() - 1
        };
        out.push(id as u64);
        let entries = options.entries();
        out.push(entries.len() as u64);
        for value in entries {
            match value {
                None => out.push(0),
                Some(v) => {
                    out.push(v.len() as u64 + 1);
                    out.extend(v.into_iter().map(u64::from));
                }
            }
        }
    }
    fn curve_value(&mut self, curve: Option<SharedCurve<E>>, out: &mut Vec<u64>) {
        match curve {
            None => out.push(0),
            Some(c) => {
                let id = if let Some(i) = self.curve_roots.iter().position(|v| v.same_reference(&c))
                {
                    i
                } else {
                    self.curve_roots.push(c.clone());
                    self.curve_roots.len() - 1
                };
                out.extend([1, id as u64]);
                let values = self.curves.read(&c);
                out.push(values.len() as u64);
                out.extend(values);
            }
        }
    }
    fn flag_array(
        &mut self,
        flags: Option<crate::AnimationFlags>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        match flags {
            None => out.push(0),
            Some(flags) => {
                let id = if let Some(i) = self
                    .animation_flags
                    .iter()
                    .position(|v| v.same_reference(&flags))
                {
                    i
                } else {
                    self.animation_flags.push(flags.clone());
                    self.animation_flags.len() - 1
                };
                out.extend([1, id as u64, flags.len() as u64]);
                for i in 0..flags.len() {
                    out.push(
                        flags
                            .value(i as i32)
                            .map_err(|_| crate::AnimationError::<E>::Index)?
                            as u64,
                    );
                }
            }
        }
        Ok(())
    }
    fn curve_array(
        &mut self,
        curves: Option<crate::AnimationCurves<E>>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        match curves {
            None => out.push(0),
            Some(curves) => {
                let id = if let Some(i) = self
                    .animation_curves
                    .iter()
                    .position(|v| v.same_reference(&curves))
                {
                    i
                } else {
                    self.animation_curves.push(curves.clone());
                    self.animation_curves.len() - 1
                };
                out.extend([1, id as u64, curves.len() as u64]);
                for i in 0..curves.len() {
                    self.curve_value(curves.value(i as i32)?, out);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn animation(
        &mut self,
        a: Option<GradientAnimation<E>>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        match a {
            None => out.push(0),
            Some(a) => {
                let id = if let Some(i) = self.animations.iter().position(|v| v.same_reference(&a))
                {
                    i
                } else {
                    self.animations.push(a.clone());
                    self.animations.len() - 1
                };
                out.extend([1, id as u64, a.count() as u64]);
                self.flag_array(a.flags(), out)?;
                self.curve_array(a.curves(), out)?;
                match a.limits() {
                    None => out.push(0),
                    Some(limits) => {
                        let id = if let Some(i) = self
                            .animation_limits
                            .iter()
                            .position(|v| v.same_reference(&limits))
                        {
                            i
                        } else {
                            self.animation_limits.push(limits.clone());
                            self.animation_limits.len() - 1
                        };
                        out.extend([1, id as u64, limits.len() as u64]);
                        for i in 0..limits.len() {
                            match limits
                                .value(i as i32)
                                .map_err(|_| crate::AnimationError::<E>::Index)?
                            {
                                None => out.push(0),
                                Some(l) => {
                                    out.push(1);
                                    self.limits(&l, out);
                                }
                            }
                        }
                    }
                }
                self.gradient(&a.retained_gradient(), out)?;
            }
        }
        Ok(())
    }
    pub(crate) fn adjustment(
        &mut self,
        adjusted: Option<super::super::wire::SharedPropertyAdjustment<E>>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        match adjusted {
            None => out.push(0),
            Some(a) => {
                let id = if let Some(i) = self.adjustments.iter().position(|v| v.same_reference(&a))
                {
                    i
                } else {
                    self.adjustments.push(a.clone());
                    self.adjustments.len() - 1
                };
                let parts = a.capture_parts();
                out.extend([
                    1,
                    id as u64,
                    parts.count as u64,
                    parts.property_type as u64,
                    parts.mode as u64,
                ]);
                out.extend(parts.guid.map(u64::from));
                out.push(parts.bounds.len() as u64);
                for bounds in parts.bounds {
                    out.extend(bounds.map(f64::to_bits));
                }
                match parts.names {
                    None => out.push(0),
                    Some(names) => {
                        out.push(1);
                        self.options(&names, out);
                    }
                }
                self.flag_array(parts.flags, out)?;
                self.curve_array(parts.curves, out)?;
            }
        }
        Ok(())
    }
    pub(crate) fn metadata(
        &mut self,
        metadata: &super::super::wire::LegacyWireMetadata<E>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        let (index, name, animatable, adjustable) = metadata.capture_values();
        out.extend([index as u64, animatable as u64, adjustable as u64]);
        match name {
            None => out.push(0),
            Some(name) => {
                out.push(name.len() as u64 + 1);
                out.extend(name.into_iter().map(u64::from));
            }
        }
        self.adjustment(metadata.retained_adjustment(), out)
    }
    pub(super) fn filter(
        &mut self,
        model: &PointwiseFilter<E>,
        time: f64,
    ) -> Result<Arc<[u64]>, PointwiseError<E>> {
        let mut out = vec![
            time.to_bits(),
            model.factor().to_bits(),
            model.clone_mode() as u64,
        ];
        let name = model.kind().name();
        out.push(name.len() as u64);
        out.extend(name.bytes().map(u64::from));
        let canvas = model.canvas_document();
        let style = model.style();
        let synthesis = model.synthesis_texture();
        if canvas.is_some() || style.is_some() || synthesis.is_some() {
            let mut writer = crate::BinaryWriter::default();
            let written = if let Some(canvas) = canvas {
                canvas.append_synthesis_render_values(&mut out);
                canvas.write::<E>(&mut writer, false)
            } else if let Some(style) = style {
                style.append_synthesis_render_values(&mut out);
                style.write::<E>(&mut writer)
            } else {
                let synthesis = synthesis.ok_or(PointwiseError::NullReference)?;
                synthesis.append_render_values(&mut out);
                synthesis.write::<E>(&mut writer)
            };
            written.map_err(|error| match error {
                crate::NodeWireError::Binary(crate::BinaryError::Allocation) => {
                    crate::ImageError::Allocation
                }
                _ => crate::ImageError::Argument,
            })?;
            out.push(writer.bytes().len() as u64);
            for chunk in writer.bytes().chunks(8) {
                let mut bytes = [0; 8];
                bytes[..chunk.len()].copy_from_slice(chunk);
                out.push(u64::from_le_bytes(bytes));
            }
        }
        for p in model.retained_parameters() {
            let name = p.name();
            out.push(name.len() as u64);
            out.extend(name.into_iter().map(u64::from));
            if matches!(
                p.raw_value(),
                PointwiseParameterValue::Color(_)
                    | PointwiseParameterValue::Text(_)
                    | PointwiseParameterValue::Gradient(_)
                    | PointwiseParameterValue::Curve(_)
            ) {
                match p.retained_limits() {
                    Some(limits) => {
                        out.push(1);
                        self.limits(&limits, &mut out);
                    }
                    None => out.push(0),
                }
            }
            match p.raw_value() {
                PointwiseParameterValue::Number {
                    current,
                    previous,
                    limits,
                } => {
                    out.extend([
                        0,
                        current.to_bits(),
                        previous.to_bits(),
                        p.seed_modifier().to_bits(),
                    ]);
                    self.limits(&limits, &mut out);
                }
                PointwiseParameterValue::Choice {
                    current,
                    previous,
                    options,
                } => {
                    let id = if let Some(i) =
                        self.options.iter().position(|v| v.same_reference(&options))
                    {
                        i
                    } else {
                        self.options.push(options.clone());
                        self.options.len() - 1
                    };
                    out.extend([1, id as u64, current as u64, previous as u64]);
                    let entries = options.entries();
                    out.push(entries.len() as u64);
                    for s in entries {
                        match s {
                            Some(s) => {
                                out.push(s.len() as u64 + 1);
                                out.extend(s.into_iter().map(u64::from));
                            }
                            None => out.push(0),
                        }
                    }
                }
                PointwiseParameterValue::Color(v) => {
                    out.push(9);
                    out.extend(v.components().map(f64::to_bits));
                    self.color_input(v.input_reference(), &mut out);
                    if let Some(color) = p.wire_metadata().resolved_lab_color() {
                        out.push(1);
                        out.extend(color.map(f64::to_bits));
                    } else {
                        out.push(0);
                    }
                }
                PointwiseParameterValue::Text(t) => {
                    out.push(2);
                    match t {
                        Some(t) => {
                            out.push(t.len() as u64 + 1);
                            out.extend(t.into_iter().map(u64::from));
                        }
                        None => out.push(0),
                    }
                }
                PointwiseParameterValue::Variable(value) => {
                    out.extend([13, u64::from(value.mode())]);
                    for part in [value.name(), value.options()] {
                        match part {
                            None => out.push(0),
                            Some(units) => {
                                out.push(units.len() as u64 + 1);
                                out.extend(units.iter().copied().map(u64::from));
                            }
                        }
                    }
                    out.extend(value.numeric_flags().map(u64::from));
                    match value.resolved_options() {
                        None => out.push(0),
                        Some(entries) => {
                            out.push(entries.len() as u64 + 1);
                            for (label, path) in entries {
                                out.push(label.len() as u64);
                                out.extend(label.iter().copied().map(u64::from));
                                match path {
                                    None => out.push(0),
                                    Some(path) => {
                                        out.push(path.len() as u64 + 1);
                                        out.extend(path.iter().copied().map(u64::from));
                                    }
                                }
                            }
                        }
                    }
                }
                PointwiseParameterValue::Gradient(g) => {
                    out.push(3);
                    self.gradient(&g, &mut out)?;
                }
                PointwiseParameterValue::Curve(c) => {
                    out.push(4);
                    match c {
                        Some(c) => {
                            out.push(1);
                            let id = if let Some(i) =
                                self.curve_roots.iter().position(|r| r.same_reference(&c))
                            {
                                i
                            } else {
                                self.curve_roots.push(c.clone());
                                self.curve_roots.len() - 1
                            };
                            out.push(id as u64);
                            let values = self.curves.read(&c);
                            out.push(values.len() as u64);
                            out.extend(values);
                        }
                        None => out.push(0),
                    }
                }
                PointwiseParameterValue::Environment(value) => {
                    out.push(13);
                    let mut writer = crate::BinaryWriter::default();
                    value.save(&mut writer).map_err(|error| match error {
                        crate::EnvironmentWireError::Binary(crate::BinaryError::Allocation) => {
                            PointwiseError::Image(crate::ImageError::Allocation)
                        }
                        _ => PointwiseError::InvalidOperation,
                    })?;
                    out.push(writer.bytes().len() as u64);
                    for chunk in writer.bytes().chunks(8) {
                        let mut bytes = [0; 8];
                        bytes[..chunk.len()].copy_from_slice(chunk);
                        out.push(u64::from_le_bytes(bytes));
                    }
                }
                PointwiseParameterValue::Histogram(h) => {
                    out.push(5);
                    self.histogram(&h, &mut out);
                }
                PointwiseParameterValue::Pair {
                    current,
                    previous,
                    limits,
                } => {
                    out.push(6);
                    out.extend(current.map(f64::to_bits));
                    out.extend(previous.map(f64::to_bits));
                    self.limits(&limits, &mut out);
                }
                PointwiseParameterValue::Region { current, previous } => {
                    out.push(12);
                    self.object(&current, &mut out)?;
                    self.object(&previous, &mut out)?;
                }
                PointwiseParameterValue::Matrix { current, previous } => {
                    out.push(7);
                    self.object(&current, &mut out)?;
                    self.object(&previous, &mut out)?;
                }
            }
            let size = guard(&model.core.filters)[model.ix].parameters[p.ix]
                .extras
                .size
                .clone();
            match size {
                None => out.push(0),
                Some(size) => {
                    out.push(1);
                    out.extend(size.normalized.map(f64::to_bits));
                    out.push(size.scale.to_bits());
                    match size.flags {
                        None => out.push(0),
                        Some(flags) => {
                            out.push(1);
                            out.push(flags.len() as u64);
                            out.extend(flags.into_iter().map(u64::from));
                        }
                    }
                }
            }
            let a = guard(&model.core.filters)[model.ix].parameters[p.ix]
                .extras
                .animation
                .clone();
            self.animation(a, &mut out)?;
            let metadata = guard(&model.core.filters)[model.ix].parameters[p.ix]
                .extras
                .wire
                .clone();
            self.metadata(&metadata, &mut out)?;
        }
        Ok(out.into())
    }
}

#[path = "imported_capture_values.rs"]
mod imported;
