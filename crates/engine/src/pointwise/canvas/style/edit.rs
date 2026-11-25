//! Detached drafts for the saved Style pass stack.
use super::*;
use crate::{
    ContextChangeArgument, GradientColorPoint, HistoryDirection, HistoryError, PointwiseNode,
    SharedCurve, SharedGradient, SynthesisEdit,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StylePaintKind {
    Solid,
    Input,
    Copy,
    Adjust,
    Blur,
    Refraction,
    Environment,
    Gradient,
    Noise,
    Synthesis,
}

impl StylePaintKind {
    pub const ALL: [Self; 10] = [
        Self::Solid,
        Self::Input,
        Self::Copy,
        Self::Adjust,
        Self::Blur,
        Self::Refraction,
        Self::Environment,
        Self::Gradient,
        Self::Noise,
        Self::Synthesis,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid color",
            Self::Input => "Input texture",
            Self::Copy => "Copy composite",
            Self::Adjust => "Adjust color",
            Self::Blur => "Blur composite",
            Self::Refraction => "Refraction",
            Self::Environment => "Environment reflection",
            Self::Gradient => "Gradient",
            Self::Noise => "Noise",
            Self::Synthesis => "Texture Synthesis",
        }
    }

    fn default_paint(self) -> Result<Paint, String> {
        let bevel = || Bevel {
            feather: [10., 0.],
            curve: identity_curve(),
            noise: None,
            inactive_noise: None,
        };
        Ok(match self {
            Self::Solid => CanvasStyle::default().passes.remove(0).paint,
            Self::Input => Paint::Input {
                offset: [0.; 2],
                rotation: 0,
            },
            Self::Copy => Paint::Copy {
                offset: [0.; 2],
                rotation: 0,
            },
            Self::Adjust => Paint::Adjust(ColorAdjustment {
                argb: [1.; 4],
                brightness: 0.,
                contrast: 0.,
                hue: 0.,
                saturation: 0.,
            }),
            Self::Blur => Paint::Blur { radius: 10. },
            Self::Refraction => Paint::Refract(Box::new(RefractionPaint {
                bevel: bevel(),
                index: 1.333,
                depth: 10.,
                affects_height: true,
            })),
            Self::Environment => Paint::Environment(Box::new(EnvironmentPaint {
                bevel: bevel(),
                map: Arc::new(EnvironmentRecord::default()),
                adjustment: None,
                resolution: -1,
                depth: 10.,
                affects_height: true,
                flatten_color: true,
            })),
            Self::Gradient => Paint::Gradient(Box::new(GradientPaint {
                noise: None,
                inactive_noise: None,
                field: Some(default_gradient_field()),
                gradient: default_gradient(),
            })),
            Self::Noise => {
                let lab = crate::pointwise::node_wire::default_style_noise_lab()
                    .map_err(|error| format!("Couldn’t create the default Noise Lab. {error:?}"))?;
                Paint::Noise(Box::new(NoisePaint {
                    lab,
                    gradient: default_gradient(),
                }))
            }
            Self::Synthesis => Paint::Synthesis(SynthesisPaint::default()),
        })
    }
}

impl Paint {
    fn editor_kind(&self) -> StylePaintKind {
        match self {
            Self::Solid { .. } => StylePaintKind::Solid,
            Self::Input { .. } => StylePaintKind::Input,
            Self::Copy { .. } => StylePaintKind::Copy,
            Self::Adjust(_) => StylePaintKind::Adjust,
            Self::Blur { .. } => StylePaintKind::Blur,
            Self::Refract(_) => StylePaintKind::Refraction,
            Self::Environment(_) => StylePaintKind::Environment,
            Self::Gradient(_) => StylePaintKind::Gradient,
            Self::Noise(_) => StylePaintKind::Noise,
            Self::Synthesis(_) => StylePaintKind::Synthesis,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StyleSection {
    Area,
    #[default]
    Effect,
    Blend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleCurve {
    ShapeFeather,
    ShadowFeather,
    GradientMask,
    GradientField,
    Bevel,
}

impl StyleCurve {
    pub const ALL: [Self; 5] = [
        Self::ShapeFeather,
        Self::ShadowFeather,
        Self::GradientMask,
        Self::GradientField,
        Self::Bevel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ShapeFeather => "Shape feather curve",
            Self::ShadowFeather => "Shadow feather curve",
            Self::GradientMask => "Gradient mask curve",
            Self::GradientField => "Gradient field curve",
            Self::Bevel => "Bevel curve",
        }
    }

    pub fn section(self) -> StyleSection {
        match self {
            Self::ShapeFeather | Self::ShadowFeather | Self::GradientMask => StyleSection::Area,
            Self::GradientField | Self::Bevel => StyleSection::Effect,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleNoiseScope {
    Mask,
    Gradient,
    Bevel,
    Paint,
}

impl StyleNoiseScope {
    pub fn for_section(section: StyleSection, paint: StylePaintKind) -> Option<Self> {
        match (section, paint) {
            (StyleSection::Area, _) => Some(Self::Mask),
            (StyleSection::Effect, StylePaintKind::Gradient) => Some(Self::Gradient),
            (StyleSection::Effect, StylePaintKind::Environment | StylePaintKind::Refraction) => {
                Some(Self::Bevel)
            }
            (StyleSection::Effect, StylePaintKind::Noise) => Some(Self::Paint),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mask => "Noise mask",
            Self::Gradient => "Gradient noise",
            Self::Bevel => "Bevel noise",
            Self::Paint => "Noise paint",
        }
    }
}

#[derive(Clone, Copy)]
pub enum StyleControlKind {
    Number([f64; 2]),
    Toggle,
    Choice(&'static [&'static str]),
}

#[derive(Clone, Copy)]
pub struct StyleControl {
    label: &'static str,
    value: f64,
    kind: StyleControlKind,
}

impl StyleControl {
    pub fn label(self) -> &'static str {
        self.label
    }
    pub fn value(self) -> f64 {
        self.value
    }
    pub fn kind(self) -> StyleControlKind {
        self.kind
    }

    fn accepts(self, value: f64) -> bool {
        value.is_finite()
            && (value == self.value
                || match self.kind {
                    StyleControlKind::Number([min, max]) => (min..=max).contains(&value),
                    StyleControlKind::Toggle => matches!(value, 0. | 1.),
                    StyleControlKind::Choice(options) => {
                        value.fract() == 0. && value >= 0. && value < options.len() as f64
                    }
                })
    }
}

#[derive(Clone)]
pub struct StyleEdit {
    before: Arc<CanvasStyle>,
    value: CanvasStyle,
    changed: bool,
}

impl StyleEdit {
    fn noise_record(&self, index: usize, scope: StyleNoiseScope) -> Option<&Arc<[u8]>> {
        let pass = self.value.passes.get(index)?;
        match (scope, &pass.paint) {
            (StyleNoiseScope::Mask, _) => Some(&pass.noise_mask.as_ref()?.lab),
            (StyleNoiseScope::Gradient, Paint::Gradient(paint)) => Some(&paint.noise.as_ref()?.lab),
            (StyleNoiseScope::Bevel, Paint::Environment(paint)) => {
                Some(&paint.bevel.noise.as_ref()?.lab)
            }
            (StyleNoiseScope::Bevel, Paint::Refract(paint)) => {
                Some(&paint.bevel.noise.as_ref()?.lab)
            }
            (StyleNoiseScope::Paint, Paint::Noise(paint)) => Some(&paint.lab),
            _ => None,
        }
    }

    pub fn has_noise_lab(&self, index: usize, scope: StyleNoiseScope) -> bool {
        self.noise_record(index, scope).is_some()
    }

    pub fn open_noise_lab<E: 'static>(
        &self,
        index: usize,
        scope: StyleNoiseScope,
    ) -> Result<PointwiseNode<E>, NodeWireError<E>> {
        let bytes = self
            .noise_record(index, scope)
            .ok_or(PointwiseError::InvalidOperation)?;
        let mut reader = BinaryCursor::new(bytes)?;
        let lab = read_style_noise_lab(&mut reader, 3.75, None)?;
        if reader.position()? != reader.length()? {
            return Err(BinaryError::Count.into());
        }
        Ok(lab)
    }

    pub fn set_noise_enabled(
        &mut self,
        index: usize,
        scope: StyleNoiseScope,
        enabled: bool,
    ) -> Result<bool, String> {
        let pass = self
            .value
            .passes
            .get_mut(index)
            .ok_or("The selected Style pass is unavailable.")?;
        let (noise, inactive, strength) = match (scope, &mut pass.paint) {
            (StyleNoiseScope::Mask, _) => {
                (&mut pass.noise_mask, &mut pass.inactive.noise_mask, 10.)
            }
            (StyleNoiseScope::Gradient, Paint::Gradient(paint)) => {
                (&mut paint.noise, &mut paint.inactive_noise, 30.)
            }
            (StyleNoiseScope::Bevel, Paint::Environment(paint)) => {
                (&mut paint.bevel.noise, &mut paint.bevel.inactive_noise, 5.)
            }
            (StyleNoiseScope::Bevel, Paint::Refract(paint)) => {
                (&mut paint.bevel.noise, &mut paint.bevel.inactive_noise, 5.)
            }
            _ => return Err("The selected Style pass has no optional noise here.".into()),
        };
        if enabled == noise.is_some() {
            return Ok(false);
        }
        if enabled {
            *noise = Some(match inactive.take() {
                Some(previous) => previous,
                None => StyleNoise {
                    strength,
                    lab: crate::pointwise::node_wire::default_style_noise_lab()
                        .map_err(|error| error.to_string())?,
                    affects_opacity: false,
                },
            });
        } else {
            *inactive = noise.take();
        }
        self.value.selected = index as i32;
        self.changed = true;
        Ok(true)
    }

    pub fn set_noise_lab<E: 'static>(
        &mut self,
        index: usize,
        scope: StyleNoiseScope,
        lab: &PointwiseNode<E>,
    ) -> Result<bool, NodeWireError<E>> {
        let pass = self
            .value
            .passes
            .get_mut(index)
            .ok_or(PointwiseError::InvalidOperation)?;
        let record = match (scope, &mut pass.paint) {
            (StyleNoiseScope::Mask, _) => pass.noise_mask.as_mut().map(|noise| &mut noise.lab),
            (StyleNoiseScope::Gradient, Paint::Gradient(paint)) => {
                paint.noise.as_mut().map(|noise| &mut noise.lab)
            }
            (StyleNoiseScope::Bevel, Paint::Environment(paint)) => {
                paint.bevel.noise.as_mut().map(|noise| &mut noise.lab)
            }
            (StyleNoiseScope::Bevel, Paint::Refract(paint)) => {
                paint.bevel.noise.as_mut().map(|noise| &mut noise.lab)
            }
            (StyleNoiseScope::Paint, Paint::Noise(paint)) => Some(&mut paint.lab),
            _ => None,
        }
        .ok_or(PointwiseError::InvalidOperation)?;
        let bytes = crate::pointwise::node_wire::write_style_noise_lab(lab)?;
        if bytes.as_ref() == record.as_ref() {
            return Ok(false);
        }
        *record = bytes;
        self.value.selected = index as i32;
        self.changed = true;
        Ok(true)
    }

    pub fn curve(&self, index: usize, curve: StyleCurve) -> Option<&CurveRecord> {
        let pass = self.value.passes.get(index)?;
        match curve {
            StyleCurve::ShapeFeather => Some(&pass.area.as_ref()?.feather_curve),
            StyleCurve::ShadowFeather => Some(&pass.area.as_ref()?.secondary.as_ref()?.curve),
            StyleCurve::GradientMask => Some(&pass.gradient_mask.as_ref()?.curve),
            StyleCurve::GradientField => match &pass.paint {
                Paint::Gradient(paint) => Some(&paint.field.as_ref()?.curve),
                _ => None,
            },
            StyleCurve::Bevel => match &pass.paint {
                Paint::Environment(paint) => Some(&paint.bevel.curve),
                Paint::Refract(paint) => Some(&paint.bevel.curve),
                _ => None,
            },
        }
    }

    pub fn set_curve<E: std::fmt::Debug>(
        &mut self,
        index: usize,
        curve: StyleCurve,
        edited: &SharedCurve<E>,
    ) -> Result<bool, String> {
        let pass = self
            .value
            .passes
            .get_mut(index)
            .ok_or("The selected Style pass is unavailable.")?;
        let current = match curve {
            StyleCurve::ShapeFeather => pass.area.as_mut().map(|area| &mut area.feather_curve),
            StyleCurve::ShadowFeather => pass
                .area
                .as_mut()
                .and_then(|area| area.secondary.as_mut())
                .map(|shadow| &mut shadow.curve),
            StyleCurve::GradientMask => pass.gradient_mask.as_mut().map(|field| &mut field.curve),
            StyleCurve::GradientField => match &mut pass.paint {
                Paint::Gradient(paint) => paint.field.as_mut().map(|field| &mut field.curve),
                _ => None,
            },
            StyleCurve::Bevel => match &mut pass.paint {
                Paint::Environment(paint) => Some(&mut paint.bevel.curve),
                Paint::Refract(paint) => Some(&mut paint.bevel.curve),
                _ => None,
            },
        }
        .ok_or("The selected Style curve is unavailable.")?;
        let mut before = BinaryWriter::default();
        current
            .save(&mut before)
            .map_err(|error| error.to_string())?;
        let mut after = BinaryWriter::default();
        edited.save(&mut after).map_err(|error| error.to_string())?;
        if before.bytes() == after.bytes() {
            return Ok(false);
        }
        let mut record = CurveRecord::new(0., 1., false);
        let mut reader = BinaryCursor::new(after.bytes()).map_err(|error| error.to_string())?;
        record
            .load(&mut reader, 3.75)
            .map_err(|error| error.to_string())?;
        *current = record;
        self.value.selected = index as i32;
        self.changed = true;
        Ok(true)
    }

    pub fn gradient(&self, index: usize) -> Option<&GradientRecord> {
        match &self.value.passes.get(index)?.paint {
            Paint::Gradient(paint) => Some(&paint.gradient),
            Paint::Noise(paint) => Some(&paint.gradient),
            _ => None,
        }
    }

    pub fn set_gradient(
        &mut self,
        index: usize,
        gradient: &SharedGradient,
    ) -> Result<bool, String> {
        let record = gradient.wire_record().map_err(|error| error.to_string())?;
        let current = match self.value.passes.get_mut(index).map(|pass| &mut pass.paint) {
            Some(Paint::Gradient(paint)) => &mut paint.gradient,
            Some(Paint::Noise(paint)) => &mut paint.gradient,
            _ => return Err("The selected pass no longer has a gradient.".into()),
        };
        if same_gradient(current, &record) {
            return Ok(false);
        }
        *current = record;
        self.value.selected = index as i32;
        self.changed = true;
        Ok(true)
    }

    pub(in crate::pointwise::canvas) fn new(before: Arc<CanvasStyle>) -> Self {
        Self {
            value: (*before).clone(),
            before,
            changed: false,
        }
    }

    pub(in crate::pointwise::canvas) fn into_replacement(
        self,
    ) -> Option<(Arc<CanvasStyle>, Arc<CanvasStyle>)> {
        self.changed.then(|| (self.before, Arc::new(self.value)))
    }

    pub fn environment_map(&self, index: usize) -> Option<&Arc<EnvironmentRecord>> {
        let Paint::Environment(paint) = &self.value.passes.get(index)?.paint else {
            return None;
        };
        Some(&paint.map)
    }

    pub(crate) fn environment_adjuster_key(&self, index: usize) -> Option<([u8; 16], &[u16])> {
        let Paint::Environment(paint) = &self.value.passes.get(index)?.paint else {
            return None;
        };
        paint.adjuster_key()
    }

    pub fn has_environment_adjuster(&self, index: usize) -> bool {
        self.environment_adjuster_key(index).is_some()
    }

    pub(crate) fn replace_environment_link(
        &mut self,
        index: usize,
        key: Option<([u8; 16], &[u16])>,
        map: Arc<EnvironmentRecord>,
    ) -> Option<()> {
        let Paint::Environment(paint) = &mut self.value.passes.get_mut(index)?.paint else {
            return None;
        };
        paint.adjustment = key.map(|(group, name)| {
            let mut adjustment = crate::AdjustmentRecord::new(&[[0., 0.]]);
            adjustment.property_type = 9;
            adjustment.guid = group;
            let component = &mut adjustment.components_mut()[0];
            component.name = name.to_vec();
            component.active = true;
            Arc::new(adjustment)
        });
        paint.map = map;
        self.changed = true;
        Some(())
    }

    #[cfg(test)]
    pub(crate) fn install_test_environment_adjustment(
        &mut self,
        index: usize,
        record: crate::AdjustmentRecord,
    ) {
        let Paint::Environment(paint) = &mut self.value.passes[index].paint else {
            panic!("an Environment paint");
        };
        paint.adjustment = Some(Arc::new(record));
        self.changed = true;
    }

    pub fn replace_environment_map(
        &mut self,
        index: usize,
        expected: &Arc<EnvironmentRecord>,
        map: EnvironmentRecord,
    ) -> Result<(), String> {
        let Some(current) = self.value.environment_map_mut(index) else {
            return Err("The selected Style pass no longer has an environment map.".into());
        };
        if !Arc::ptr_eq(current, expected) {
            return Err("The environment map changed. Revert fields before continuing.".into());
        }
        *current = Arc::new(map);
        self.value.selected = index as i32;
        self.changed = true;
        Ok(())
    }

    pub fn synthesis_edit(&self, index: usize) -> Option<SynthesisEdit> {
        let texture = self.synthesis_paint(index)?;
        Some(SynthesisEdit::new(texture.texture().clone(), true))
    }

    pub fn is_synthesis_adjusted(&self, index: usize) -> bool {
        self.synthesis_paint(index)
            .is_some_and(|paint| paint.key().is_some())
    }

    pub(crate) fn synthesis_paint(&self, index: usize) -> Option<&SynthesisPaint> {
        let Paint::Synthesis(paint) = &self.value.passes.get(index)?.paint else {
            return None;
        };
        Some(paint)
    }

    pub(crate) fn replace_synthesis_paint(&mut self, index: usize, next: SynthesisPaint) -> bool {
        let Some(pass) = self.value.passes.get_mut(index) else {
            return false;
        };
        if !matches!(pass.paint, Paint::Synthesis(_)) {
            return false;
        }
        pass.paint = Paint::Synthesis(next);
        self.changed = true;
        true
    }

    pub(crate) fn prepare_synthesis_material(
        &self,
        index: usize,
        edit: SynthesisEdit,
    ) -> Result<Option<(SynthesisCatalogValue, SynthesisRecord, SynthesisRecord)>, &'static str>
    {
        let paint = self
            .synthesis_paint(index)
            .ok_or("The selected pass no longer uses Texture Synthesis.")?;
        let (catalog, before) = paint
            .resolved
            .as_ref()
            .ok_or("The selected texture no longer uses this shared Material.")?;
        let after = edit.into_local_replacement(before.texture())?;
        Ok(after.map(|after| {
            (
                catalog.clone(),
                before.clone(),
                SynthesisRecord::from_texture(after),
            )
        }))
    }

    pub fn set_synthesis(
        &mut self,
        index: usize,
        edit: SynthesisEdit,
    ) -> Result<bool, &'static str> {
        let Some(StylePass {
            paint: Paint::Synthesis(texture),
            ..
        }) = self.value.passes.get_mut(index)
        else {
            return Err("The selected pass no longer uses Texture Synthesis.");
        };
        if texture.key().is_some() {
            return Err("The selected texture uses a shared Material.");
        }
        let Some(replacement) = edit.into_local_replacement(&texture.local)? else {
            return Ok(false);
        };
        texture.local = replacement;
        self.value.selected = index as i32;
        self.changed = true;
        Ok(true)
    }

    pub fn same_revision(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.before, &other.before)
    }

    /// Finds the same pass in a different live revision, including after reordering.
    pub fn corresponding_pass(&self, previous: &Self, index: usize) -> Option<usize> {
        let identity = &previous.value.passes.get(index)?.identity;
        self.value
            .passes
            .iter()
            .position(|pass| Arc::ptr_eq(&pass.identity, identity))
    }

    pub fn count(&self) -> usize {
        self.value.passes.len()
    }

    pub fn selected(&self) -> Option<usize> {
        usize::try_from(self.value.selected)
            .ok()
            .filter(|index| *index < self.count())
            .or_else(|| (self.count() > 0).then_some(0))
    }

    pub fn paint(&self, index: usize) -> Option<StylePaintKind> {
        Some(self.value.passes.get(index)?.paint.editor_kind())
    }

    pub fn add(&mut self, kind: StylePaintKind) -> Result<usize, String> {
        let mut pass = CanvasStyle::default().passes.remove(0);
        pass.paint = kind.default_paint()?;
        self.value.passes.insert(0, pass);
        self.value.selected = 0;
        self.changed = true;
        Ok(0)
    }

    pub fn duplicate(&mut self, index: usize) -> Option<usize> {
        let mut pass = self.value.passes.get(index)?.clone();
        pass.identity = Arc::new(());
        self.value.passes.insert(index, pass);
        self.value.selected = index as i32;
        self.changed = true;
        Some(index)
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.count() {
            return false;
        }
        self.value.passes.remove(index);
        self.value.selected = self
            .count()
            .checked_sub(1)
            .map_or(-1, |_| index.saturating_sub(1) as i32);
        self.changed = true;
        true
    }

    pub fn move_pass(&mut self, index: usize, to: usize) -> bool {
        if index == to || index >= self.count() || to >= self.count() {
            return false;
        }
        let pass = self.value.passes.remove(index);
        self.value.passes.insert(to, pass);
        self.value.selected = to as i32;
        self.changed = true;
        true
    }

    pub fn set_paint(&mut self, index: usize, kind: StylePaintKind) -> Result<bool, String> {
        let Some(pass) = self.value.passes.get_mut(index) else {
            return Ok(false);
        };
        if pass.paint.editor_kind() == kind {
            return Ok(false);
        }
        let next = match pass
            .inactive
            .paints
            .iter()
            .position(|paint| paint.editor_kind() == kind)
        {
            Some(index) => pass.inactive.paints.swap_remove(index),
            None => kind.default_paint()?,
        };
        let mut parked = std::mem::replace(&mut pass.paint, next);
        if let Paint::Synthesis(paint) = &mut parked {
            paint.disconnect_for_effect_change();
        }
        pass.inactive.paints.push(parked);
        self.value.selected = index as i32;
        self.changed = true;
        Ok(true)
    }

    pub fn controls(&self, index: usize, section: StyleSection) -> Vec<StyleControl> {
        let Some(mut pass) = self.value.passes.get(index).cloned() else {
            return Vec::new();
        };
        let mut controls = Vec::new();
        pass.visit_controls(section, |control| {
            controls.push(control);
            control.value
        });
        controls
    }

    pub fn set_controls(
        &mut self,
        index: usize,
        section: StyleSection,
        values: &[f64],
    ) -> Result<(), &'static str> {
        let controls = self.controls(index, section);
        if index >= self.count() || values.len() != controls.len() {
            return Err("The selected Style pass changed.");
        }
        let mut changed = false;
        for (control, &value) in controls.iter().zip(values) {
            if !control.accepts(value) {
                return Err(control.label);
            }
            changed |= value != control.value;
        }
        if changed {
            let mut values = values.iter();
            self.value.passes[index]
                .visit_controls(section, |_| *values.next().expect("validated controls"));
            self.value.selected = index as i32;
            self.changed = true;
        }
        Ok(())
    }

    pub fn load_preset(&mut self, bytes: &[u8]) -> Result<(), String> {
        let read = || -> Result<CanvasStyle, NodeWireError<()>> {
            let mut reader = BinaryCursor::new(bytes)?;
            if !reader
                .read_string()?
                .into_iter()
                .eq("GeneticaStyle+;&".encode_utf16())
            {
                return Err(BinaryError::LengthEncoding.into());
            }
            let version = f64::from(reader.read_i32()?) / 100.;
            let thumbnail = reader.read_i32()?;
            let start = reader.position()?;
            if thumbnail < 0 || thumbnail > reader.length()? - start {
                return Err(BinaryError::Count.into());
            }
            reader.seek(i64::from(start + thumbnail))?;
            let style = CanvasStyle::read(&mut reader, version, None)?;
            if reader.position()? != reader.length()? {
                return Err(BinaryError::Count.into());
            }
            Ok(style)
        };
        self.value =
            read().map_err(|error| format!("Couldn’t open the Style preset. {error:?}"))?;
        self.changed = true;
        Ok(())
    }
}

fn identity_curve() -> CurveRecord {
    CurveRecord::new(0., 1., false)
}

fn default_gradient() -> GradientRecord {
    let mut gradient = GradientRecord::default();
    gradient.colors = vec![
        GradientColorPoint {
            coordinate: 0,
            argb: [1., 0., 0., 0.],
        },
        GradientColorPoint {
            coordinate: gradient.width - 1,
            argb: [1.; 4],
        },
    ];
    gradient
}

fn same_gradient(a: &GradientRecord, b: &GradientRecord) -> bool {
    a.width == b.width
        && a.colors.len() == b.colors.len()
        && a.transparency.len() == b.transparency.len()
        && a.colors.iter().zip(&b.colors).all(|(a, b)| {
            a.coordinate == b.coordinate && a.argb.map(f64::to_bits) == b.argb.map(f64::to_bits)
        })
        && a.transparency
            .iter()
            .zip(&b.transparency)
            .all(|(a, b)| a.coordinate == b.coordinate && a.value.to_bits() == b.value.to_bits())
}

fn default_gradient_field() -> GradientField {
    GradientField {
        wrap: true,
        curve: identity_curve(),
        blur: 0.,
        angle: 0.,
        offset: [0.; 2],
        kind: 0,
    }
}

impl<E: 'static> PointwiseFilter<E> {
    pub fn style_edit(&self) -> Option<StyleEdit> {
        Some(StyleEdit::new(self.style()?))
    }

    pub fn edit_style(&self, edit: StyleEdit, label: &str) -> Result<bool, PointwiseError<E>> {
        let Some((before, after)) = edit.into_replacement() else {
            return Ok(false);
        };
        let node = self.node();
        let slot = node.texture_node_logic().and_then(|logic| logic.slot());
        if !self.exchange_style(&before, after.clone()) {
            return Err(PointwiseError::InvalidOperation);
        }
        if let Some((slot, history)) = slot.and_then(|slot| Some((slot.clone(), slot.history()?))) {
            let sequence = history.edit_sequence();
            let result = history.record_graph_edit(
                slot.history_target(),
                label.encode_utf16().collect(),
                Box::new(StyleHistory {
                    owner: Arc::downgrade(&node.core()),
                    before: before.clone(),
                    after: after.clone(),
                }),
            );
            if let Err(error) = result {
                if history.edit_sequence() == sequence {
                    self.exchange_style(&after, before);
                }
                return Err(PointwiseError::History(Box::new(error)));
            }
        }
        node.notify_property_change_with(ContextChangeArgument::shared_empty())?;
        Ok(true)
    }
}

struct StyleHistory<E> {
    owner: Weak<crate::pointwise::graph::NodeCore<E>>,
    before: Arc<CanvasStyle>,
    after: Arc<CanvasStyle>,
}

impl<E: 'static> crate::history::HistoryGraphEdit<E> for StyleHistory<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let owner = self.owner.upgrade().ok_or(HistoryError::BranchChanged)?;
        let sponsor = crate::pointwise::region::Sponsor::for_node(&owner)
            .ok_or(HistoryError::BranchChanged)?;
        let node = crate::pointwise::PointwiseNode {
            id: owner.id,
            sponsor,
        };
        let filter = node.filter().ok_or(HistoryError::BranchChanged)?;
        let (expected, restored) = match direction {
            HistoryDirection::Undo => (&self.after, &self.before),
            HistoryDirection::Redo => (&self.before, &self.after),
        };
        if !filter.exchange_style(expected, restored.clone()) {
            return Err(HistoryError::BranchChanged);
        }
        node.notify_property_change_with(ContextChangeArgument::shared_empty())
            .map_err(|error| HistoryError::Property(Box::new(error)))
    }
}

const BLEND_MODES: &[&str] = &[
    "Replace transparency",
    "Normal",
    "Add",
    "Subtractive",
    "Multiply",
    "Darken",
    "Color Burn",
    "Lighten",
    "Screen",
    "Color Dodge",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];
const ROTATIONS: &[&str] = &[
    "None",
    "Clockwise 45°",
    "Clockwise 90°",
    "Clockwise 135°",
    "180°",
    "Counterclockwise 135°",
    "Counterclockwise 90°",
    "Counterclockwise 45°",
];

struct Controls<F>(F);

impl<F: FnMut(StyleControl) -> f64> Controls<F> {
    fn number(&mut self, label: &'static str, value: &mut f64, range: [f64; 2]) {
        *value = (self.0)(StyleControl {
            label,
            value: *value,
            kind: StyleControlKind::Number(range),
        });
    }

    fn single(&mut self, label: &'static str, value: &mut f32, range: [f64; 2]) {
        let mut double = f64::from(*value);
        self.number(label, &mut double, range);
        *value = double as f32;
    }

    fn toggle(&mut self, label: &'static str, value: &mut bool) {
        *value = (self.0)(StyleControl {
            label,
            value: f64::from(u8::from(*value)),
            kind: StyleControlKind::Toggle,
        }) != 0.;
    }

    fn choice(&mut self, label: &'static str, value: &mut i32, options: &'static [&'static str]) {
        *value = (self.0)(StyleControl {
            label,
            value: f64::from(*value),
            kind: StyleControlKind::Choice(options),
        }) as i32;
    }

    fn rgb(&mut self, argb: &mut [f64; 4]) {
        for (label, value) in ["Red", "Green", "Blue"].into_iter().zip(&mut argb[1..]) {
            self.number(label, value, [0., 1.]);
        }
    }

    fn offset(&mut self, labels: [&'static str; 2], offset: &mut [f64; 2]) {
        self.number(labels[0], &mut offset[0], [-100., 100.]);
        self.number(labels[1], &mut offset[1], [-100., 100.]);
    }

    fn feather(&mut self, hard: &mut f64, soft: &mut f64) {
        self.number("Hard feather", hard, [0., 100.]);
        self.number("Soft feather", soft, [0., 100.]);
    }

    fn bevel(&mut self, bevel: &mut Bevel) {
        self.number("Hard bevel", &mut bevel.feather[0], [0., 100.]);
        self.number("Soft bevel", &mut bevel.feather[1], [0., 100.]);
        if let Some(noise) = &mut bevel.noise {
            self.number("Bevel noise strength", &mut noise.strength, [0., 100.]);
        }
    }

    fn field(&mut self, field: &mut GradientField) {
        self.choice(
            "Gradient type",
            &mut field.kind,
            &["Contour", "Linear", "Radial", "Square", "Conical"],
        );
        self.toggle("Wrap gradient", &mut field.wrap);
        self.number("Gradient blur", &mut field.blur, [0., 100.]);
        self.number("Gradient angle", &mut field.angle, [-360., 360.]);
        self.offset(["Gradient X", "Gradient Y"], &mut field.offset);
    }

    fn optional_field(
        &mut self,
        label: &'static str,
        field: &mut Option<GradientField>,
        disabled: &mut Option<GradientField>,
    ) {
        let mut enabled = field.is_some();
        self.toggle(label, &mut enabled);
        if let Some(field) = field {
            self.field(field);
        }
        match (enabled, field.is_some()) {
            (true, false) => *field = Some(disabled.take().unwrap_or_else(default_gradient_field)),
            (false, true) => *disabled = field.take(),
            _ => {}
        }
    }

    fn area(&mut self, pass: &mut StylePass) {
        let mut enabled = pass.area.is_some();
        self.toggle("Use shape", &mut enabled);
        if let Some(area) = &mut pass.area {
            self.offset(["Shape X", "Shape Y"], &mut area.offset);
            self.number("Shape thickness", &mut area.thickness, [-100., 100.]);
            self.feather(&mut area.hard_feather, &mut area.soft_feather);
            self.toggle("Make outline", &mut area.outline);
            self.number(
                "Outline thickness",
                &mut area.outline_thickness,
                [-100., 100.],
            );
            let mut shadow = area.secondary.as_ref().map_or(0, |setting| setting.mode);
            self.choice(
                "Shadow",
                &mut shadow,
                &["Off", "Outer shadow", "Inner shadow"],
            );
            if let Some(setting) = &mut area.secondary {
                self.offset(["Shadow X", "Shadow Y"], &mut setting.offset);
                self.number("Shadow hard feather", &mut setting.hard_feather, [0., 100.]);
                self.number("Shadow soft feather", &mut setting.soft_feather, [0., 100.]);
            }
            if shadow == 0 {
                if area.secondary.is_some() {
                    pass.inactive.shadow = area.secondary.take();
                }
            } else {
                area.secondary
                    .get_or_insert_with(|| {
                        pass.inactive
                            .shadow
                            .take()
                            .unwrap_or_else(|| SecondaryArea {
                                mode: shadow,
                                offset: [3., -3.],
                                hard_feather: 0.,
                                soft_feather: 3.,
                                curve: identity_curve(),
                            })
                    })
                    .mode = shadow;
            }
        }
        if enabled && pass.area.is_none() {
            pass.area = pass
                .inactive
                .area
                .take()
                .or_else(|| CanvasStyle::default().passes.remove(0).area);
        } else if !enabled && pass.area.is_some() {
            pass.inactive.area = pass.area.take();
        }
        self.optional_field(
            "Gradient mask",
            &mut pass.gradient_mask,
            &mut pass.inactive.gradient_mask,
        );
        if let Some(noise) = &mut pass.noise_mask {
            self.number("Noise mask strength", &mut noise.strength, [0., 100.]);
            self.toggle("Noise affects opacity", &mut noise.affects_opacity);
        }
    }

    fn effect(&mut self, paint: &mut Paint, disabled_field: &mut Option<GradientField>) {
        match paint {
            Paint::Solid { argb, transparency } => {
                self.rgb(argb);
                self.number("Transparency", transparency, [0., 100.]);
            }
            Paint::Input { offset, rotation } | Paint::Copy { offset, rotation } => {
                self.offset(["Offset X", "Offset Y"], offset);
                self.choice("Rotate", rotation, ROTATIONS);
            }
            Paint::Adjust(paint) => {
                self.rgb(&mut paint.argb);
                self.number("Brightness", &mut paint.brightness, [-100., 100.]);
                self.number("Contrast", &mut paint.contrast, [-100., 100.]);
                self.number("Hue", &mut paint.hue, [-360., 360.]);
                self.number("Saturation", &mut paint.saturation, [-100., 100.]);
            }
            Paint::Blur { radius } => self.number("Radius", radius, [0., 100.]),
            Paint::Refract(paint) => {
                self.bevel(&mut paint.bevel);
                self.number("Refraction index", &mut paint.index, [-10., 10.]);
                self.number("Depth", &mut paint.depth, [-100., 100.]);
                self.toggle("Affect Height Map", &mut paint.affects_height);
            }
            Paint::Environment(paint) => {
                self.bevel(&mut paint.bevel);
                self.number("Depth", &mut paint.depth, [-100., 100.]);
                self.toggle("Affect Height Map", &mut paint.affects_height);
                self.toggle("Flatten for color only", &mut paint.flatten_color);
                let sizes = [-1, 100, 200, 400, 800, 1600, 3200];
                let mut index = sizes
                    .iter()
                    .position(|&size| size == paint.resolution)
                    .map_or(-1, |n| n as i32);
                self.choice(
                    "Resolution",
                    &mut index,
                    &["Automatic", "100", "200", "400", "800", "1600", "3200"],
                );
                if let Some(&size) = sizes.get(index as usize) {
                    paint.resolution = size;
                }
            }
            Paint::Gradient(paint) => {
                self.optional_field("Use gradient field", &mut paint.field, disabled_field);
                if let Some(noise) = &mut paint.noise {
                    self.number("Gradient noise strength", &mut noise.strength, [0., 100.]);
                }
            }
            Paint::Noise(_) | Paint::Synthesis(_) => {}
        }
    }

    fn blend(&mut self, blend: &mut Blend) {
        self.choice("Blend mode", &mut blend.mode, BLEND_MODES);
        self.number("Opacity", &mut blend.opacity, [0., 100.]);
        self.toggle("Skip for color only", &mut blend.skip_for_color);
        self.choice(
            "Erode mode",
            &mut blend.erosion_mode,
            &[
                "Upper highlights",
                "Upper shadows",
                "Lower highlights",
                "Lower shadows",
                "Upper minus lower",
                "Lower minus upper",
            ],
        );
        self.single("Erode amount", &mut blend.erosion, [0., 100.]);
        self.single("Erode sharpness", &mut blend.erosion_sharpness, [0., 100.]);
        self.toggle("Erode edges only", &mut blend.erosion_edges_only);
        for (relief, labels) in blend.relief.iter_mut().zip([
            [
                "Upper relief",
                "Upper normal blur",
                "Upper depth",
                "Upper offset blur",
                "Upper direction X",
                "Upper direction Y",
                "Upper distance",
            ],
            [
                "Lower relief",
                "Lower normal blur",
                "Lower depth",
                "Lower offset blur",
                "Lower direction X",
                "Lower direction Y",
                "Lower distance",
            ],
        ]) {
            self.choice(labels[0], &mut relief.mode, &["Off", "Normal", "Offset"]);
            self.number(labels[1], &mut relief.normal_blur, [0., 100.]);
            self.number(labels[2], &mut relief.depth, [-100., 100.]);
            self.number(labels[3], &mut relief.offset_blur, [0., 100.]);
            self.offset([labels[4], labels[5]], &mut relief.direction);
            self.number(labels[6], &mut relief.distance, [0., 100.]);
        }
    }
}

impl StylePass {
    fn visit_controls(&mut self, section: StyleSection, update: impl FnMut(StyleControl) -> f64) {
        let mut controls = Controls(update);
        match section {
            StyleSection::Area => controls.area(self),
            StyleSection::Effect => {
                controls.effect(&mut self.paint, &mut self.inactive.effect_field)
            }
            StyleSection::Blend => controls.blend(&mut self.blend),
        }
    }
}
