//! Copy-on-write edits to the Environment map stored in one graph property.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentLight {
    Hard,
    Soft,
    ClearSky,
    CloudySky,
}

impl EnvironmentLight {
    pub const ALL: [Self; 4] = [Self::Hard, Self::Soft, Self::ClearSky, Self::CloudySky];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Hard => "HardLight",
            Self::Soft => "SoftLight",
            Self::ClearSky => "ClearSky",
            Self::CloudySky => "CloudySky",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentLayerKind {
    Image,
    Light(EnvironmentLight),
    Color,
}

impl EnvironmentLayerKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Light(light) => light.label(),
            Self::Color => "Solid color",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentTransformFilter {
    None,
    Diffuse,
    Scatter,
    Anisotropic,
    LegacyAnisotropic,
}

impl EnvironmentTransformFilter {
    pub const ALL: [Self; 4] = [Self::None, Self::Diffuse, Self::Scatter, Self::Anisotropic];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Diffuse => "Diffuse",
            Self::Scatter => "Scatter",
            Self::Anisotropic => "Anisotropic",
            Self::LegacyAnisotropic => "Legacy anisotropic",
        }
    }

    fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::None),
            1 => Some(Self::Diffuse),
            2 => Some(Self::Scatter),
            3 => Some(Self::Anisotropic),
            4 => Some(Self::LegacyAnisotropic),
            _ => None,
        }
    }

    const fn defaults(self) -> [f64; 2] {
        match self {
            Self::None => [-1., -1.],
            Self::Diffuse => [30., 0.],
            Self::Scatter => [30., 50.],
            Self::Anisotropic => [3., 40.],
            Self::LegacyAnisotropic => [5., 5.],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentField {
    Exposure,
    Gamma,
    Rotation(usize),
    Elevation(usize),
    SkyHaze(usize),
    SkyDepth(usize),
    ImageExposure(usize),
    ImageGamma(usize),
    Red(usize),
    Green(usize),
    Blue(usize),
    Contrast(usize),
    Hue(usize),
    Saturation(usize),
    Tint(usize, usize),
    MaskBlur(usize),
    MaskTint(usize, usize),
    MaskColor(usize, usize),
    Strength(usize),
    BlendGamma(usize),
    MixerAmount(usize),
    MixerContrast(usize),
    MixerLevel(usize),
    MixerRotation(usize),
    MixerElevation(usize),
    Heading(Option<usize>),
    Pitch(Option<usize>),
    Roll(Option<usize>),
    FilterRadius(Option<usize>),
    FilterStrength(Option<usize>),
    FilterHorizontal(Option<usize>),
    FilterVertical(Option<usize>),
}

impl EnvironmentField {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Exposure => "Master exposure",
            Self::Gamma => "Master gamma",
            Self::Rotation(_) => "Light rotation",
            Self::Elevation(_) => "Light elevation",
            Self::SkyHaze(_) => "Sky haze",
            Self::SkyDepth(_) => "Sky depth",
            Self::ImageExposure(_) => "Image exposure",
            Self::ImageGamma(_) => "Image gamma",
            Self::Red(_) => "Red",
            Self::Green(_) => "Green",
            Self::Blue(_) => "Blue",
            Self::Contrast(_) => "Contrast",
            Self::Hue(_) => "Hue",
            Self::Saturation(_) => "Saturation",
            Self::Tint(_, 0) => "Colorize red",
            Self::Tint(_, 1) => "Colorize green",
            Self::Tint(_, 2) => "Colorize blue",
            Self::Tint(..) => "Colorize",
            Self::MaskBlur(_) => "Mask blur",
            Self::MaskTint(_, 0) => "Mask tint red",
            Self::MaskTint(_, 1) => "Mask tint green",
            Self::MaskTint(_, 2) => "Mask tint blue",
            Self::MaskTint(..) => "Mask tint",
            Self::MaskColor(_, 0) => "Mask fill red",
            Self::MaskColor(_, 1) => "Mask fill green",
            Self::MaskColor(_, 2) => "Mask fill blue",
            Self::MaskColor(..) => "Mask fill color",
            Self::Strength(_) => "Blend strength",
            Self::BlendGamma(_) => "Blend gamma",
            Self::MixerAmount(_) => "Mixer amount",
            Self::MixerContrast(_) => "Mixer contrast",
            Self::MixerLevel(_) => "Mixer level",
            Self::MixerRotation(_) => "Mixer rotation",
            Self::MixerElevation(_) => "Mixer elevation",
            Self::Heading(None) => "Master heading",
            Self::Heading(Some(_)) => "Layer heading",
            Self::Pitch(None) => "Master pitch",
            Self::Pitch(Some(_)) => "Layer pitch",
            Self::Roll(None) => "Master roll",
            Self::Roll(Some(_)) => "Layer roll",
            Self::FilterRadius(None) => "Master filter radius",
            Self::FilterRadius(Some(_)) => "Layer filter radius",
            Self::FilterStrength(None) => "Master filter strength",
            Self::FilterStrength(Some(_)) => "Layer filter strength",
            Self::FilterHorizontal(None) => "Master filter horizontal",
            Self::FilterHorizontal(Some(_)) => "Layer filter horizontal",
            Self::FilterVertical(None) => "Master filter vertical",
            Self::FilterVertical(Some(_)) => "Layer filter vertical",
        }
    }

    pub const fn bounds(self) -> [f64; 2] {
        match self {
            Self::Exposure | Self::ImageExposure(_) => [-20., 20.],
            Self::Gamma | Self::ImageGamma(_) | Self::BlendGamma(_) => [0.01, 100.],
            Self::Rotation(_) | Self::MixerRotation(_) => [0., 360.],
            Self::Elevation(_) | Self::MixerElevation(_) => [0., 90.],
            Self::Red(_)
            | Self::Green(_)
            | Self::Blue(_)
            | Self::Tint(..)
            | Self::MaskTint(..)
            | Self::MaskColor(..) => [0., 1.],
            Self::Contrast(_)
            | Self::Saturation(_)
            | Self::MixerAmount(_)
            | Self::MixerContrast(_) => [-100., 100.],
            Self::Hue(_) => [-360., 360.],
            Self::Strength(_) => [0., 100_000.],
            Self::Heading(_) | Self::Pitch(_) | Self::Roll(_) => [-360., 360.],
            Self::MaskBlur(_)
            | Self::SkyHaze(_)
            | Self::SkyDepth(_)
            | Self::MixerLevel(_)
            | Self::FilterRadius(_)
            | Self::FilterStrength(_)
            | Self::FilterHorizontal(_)
            | Self::FilterVertical(_) => [0., 100.],
        }
    }

    fn layer(self) -> Option<usize> {
        match self {
            Self::Exposure | Self::Gamma => None,
            Self::Rotation(i)
            | Self::Elevation(i)
            | Self::SkyHaze(i)
            | Self::SkyDepth(i)
            | Self::ImageExposure(i)
            | Self::ImageGamma(i)
            | Self::Red(i)
            | Self::Green(i)
            | Self::Blue(i)
            | Self::Contrast(i)
            | Self::Hue(i)
            | Self::Saturation(i)
            | Self::Tint(i, _)
            | Self::MaskBlur(i)
            | Self::MaskTint(i, _)
            | Self::MaskColor(i, _)
            | Self::Strength(i)
            | Self::BlendGamma(i)
            | Self::MixerAmount(i)
            | Self::MixerContrast(i)
            | Self::MixerLevel(i)
            | Self::MixerRotation(i)
            | Self::MixerElevation(i) => Some(i),
            _ => self.transform_slot().and_then(|(layer, _)| layer),
        }
    }
    fn mask_channel(self) -> Option<(usize, usize)> {
        match self {
            Self::MaskTint(_, channel @ 0..=2) => Some((0, channel + 1)),
            Self::MaskColor(_, channel @ 0..=2) => Some((1, channel + 1)),
            _ => None,
        }
    }

    fn blend_slot(self) -> Option<usize> {
        match self {
            Self::Strength(_) => Some(1),
            Self::BlendGamma(_) => Some(2),
            Self::MixerAmount(_) => Some(4),
            Self::MixerContrast(_) => Some(5),
            Self::MixerLevel(_) => Some(6),
            Self::MixerRotation(_) => Some(7),
            Self::MixerElevation(_) => Some(8),
            _ => None,
        }
    }

    fn transform_slot(self) -> Option<(Option<usize>, usize)> {
        match self {
            Self::Heading(layer) => Some((layer, 0)),
            Self::Pitch(layer) => Some((layer, 1)),
            Self::Roll(layer) => Some((layer, 2)),
            Self::FilterRadius(layer) | Self::FilterHorizontal(layer) => Some((layer, 4)),
            Self::FilterStrength(layer) | Self::FilterVertical(layer) => Some((layer, 5)),
            _ => None,
        }
    }
}

/// A numeric field or choice on an Environment map that can receive a Lab link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentNumericReceiver {
    Field(EnvironmentField),
    Light(usize),
    Blend(usize),
    Filter(Option<usize>),
}

impl From<EnvironmentField> for EnvironmentNumericReceiver {
    fn from(field: EnvironmentField) -> Self {
        Self::Field(field)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentEditError {
    Layer,
    Field,
    Bounds,
    Allocation,
    Gradient(crate::GradientError),
}

impl fmt::Display for EnvironmentEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Layer => "The environment layer is no longer available.",
            Self::Field => "The field is unavailable for this environment layer.",
            Self::Bounds => "The value is outside the field’s range.",
            Self::Allocation => "Couldn’t allocate another environment layer.",
            Self::Gradient(error) => return error.fmt(f),
        })
    }
}

impl Error for EnvironmentEditError {}

impl Clone for EnvironmentRecord {
    fn clone(&self) -> Self {
        Self {
            layers: self.layers.clone(),
            master: self
                .master
                .each_ref()
                .map(ModernPropertyRecord::copy_record_snapshot),
            transform: self.transform.clone(),
        }
    }
}

impl Clone for EnvironmentLayer {
    fn clone(&self) -> Self {
        Self {
            source: match &self.source {
                LayerSource::Image {
                    tone,
                    asset,
                    projection,
                } => LayerSource::Image {
                    tone: Box::new(
                        tone.each_ref()
                            .map(ModernPropertyRecord::copy_record_snapshot),
                    ),
                    asset: asset.clone(),
                    projection: *projection,
                },
                LayerSource::Light(light) => LayerSource::Light(Box::new(
                    light
                        .each_ref()
                        .map(ModernPropertyRecord::copy_record_snapshot),
                )),
                LayerSource::Color(color) => {
                    LayerSource::Color(Box::new(color.copy_record_snapshot()))
                }
            },
            mask: self.mask.clone(),
            transform: self.transform.clone(),
            color: self
                .color
                .each_ref()
                .map(ModernPropertyRecord::copy_record_snapshot),
            recolor: self
                .recolor
                .as_ref()
                .map(ModernPropertyRecord::copy_record_snapshot),
            blend: self
                .blend
                .each_ref()
                .map(ModernPropertyRecord::copy_record_snapshot),
        }
    }
}

impl Clone for Transform {
    fn clone(&self) -> Self {
        Self {
            properties: self
                .properties
                .each_ref()
                .map(ModernPropertyRecord::copy_record_snapshot),
            filter_values: self.filter_values,
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            properties: [
                ANGLE.record(),
                ANGLE.record(),
                ANGLE.record(),
                Property::Selection(4).record(),
                PERCENT.with_current(-1.),
                PERCENT.with_current(-1.),
            ],
            filter_values: [None; 5],
        }
    }
}

impl EnvironmentRecord {
    pub fn image_projection(&self, index: usize) -> Option<EnvironmentImageProjection> {
        let LayerSource::Image { projection, .. } = &self.layers.get(index)?.source else {
            return None;
        };
        EnvironmentImageProjection::from_wire(*projection)
    }

    pub fn set_image_projection(
        &mut self,
        index: usize,
        next: EnvironmentImageProjection,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        let LayerSource::Image { projection, .. } = &mut Arc::make_mut(layer).source else {
            return Err(EnvironmentEditError::Field);
        };
        *projection = next as i32;
        Ok(())
    }

    pub fn set_layer_image(
        &mut self,
        index: usize,
        image: EnvironmentImage,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        let LayerSource::Image { asset, .. } = &mut Arc::make_mut(layer).source else {
            return Err(EnvironmentEditError::Field);
        };
        *asset = Some(Arc::new(ImageAsset::Embedded(image)));
        Ok(())
    }

    pub fn layer_kind(&self, index: usize) -> Option<EnvironmentLayerKind> {
        Some(match &self.layers.get(index)?.source {
            LayerSource::Image { .. } => EnvironmentLayerKind::Image,
            LayerSource::Color(_) => EnvironmentLayerKind::Color,
            LayerSource::Light(properties) => {
                let ModernPropertyValue::Selected { current, .. } = properties[0].value() else {
                    return None;
                };
                EnvironmentLayerKind::Light(*EnvironmentLight::ALL.get(*current as usize)?)
            }
        })
    }

    /// Replaces the generator and its properties without changing the layer's
    /// mask, transform, color adjustments, or blend.
    pub fn set_layer_generator(
        &mut self,
        index: usize,
        kind: EnvironmentLayerKind,
    ) -> Result<bool, EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        let same_kind = matches!(
            (&layer.source, kind),
            (LayerSource::Image { .. }, EnvironmentLayerKind::Image)
                | (LayerSource::Color(_), EnvironmentLayerKind::Color)
                | (LayerSource::Light(_), EnvironmentLayerKind::Light(_))
        );
        if same_kind {
            return Ok(false);
        }
        Arc::make_mut(layer).source = new_layer_source(kind);
        Ok(true)
    }

    /// New layers enter at the top of the stack, which is index zero on disk.
    pub fn add_layer(&mut self, kind: EnvironmentLayerKind) -> Result<(), EnvironmentEditError> {
        self.layers
            .try_reserve(1)
            .map_err(|_| EnvironmentEditError::Allocation)?;
        let mut white = COLOR.record();
        if let ModernPropertyValue::Color { argb, .. } = white.value_mut() {
            *argb = [1.; 4];
        }
        let mut strength = Property::Integer([0., 400.]).record();
        set_integer(&mut strength, 100);
        self.layers.insert(
            0,
            Arc::new(EnvironmentLayer {
                source: new_layer_source(kind),
                mask: Mask {
                    fill: EnvironmentMaskFill::KeepSource,
                    hemisphere: EnvironmentMaskHemisphere::Off,
                    amount: 0.,
                    colors: [[1.; 4], [0.; 4]],
                    gradient: GradientRecord::default(),
                },
                transform: Transform::default(),
                color: [
                    CONTRAST.record(),
                    ANGLE.record(),
                    CONTRAST.record(),
                    FLAG.record(),
                    white,
                ],
                recolor: None,
                blend: [
                    selected(18, 0),
                    strength,
                    GAMMA.with_current(1.),
                    FLAG.record(),
                    CONTRAST.record(),
                    CONTRAST.record(),
                    PERCENT.record(),
                    ANGLE.with_current(45.),
                    ELEVATION.with_current(30.),
                ],
            }),
        );
        Ok(())
    }

    /// Insert a preset's layers above this map's layers, in their original order.
    pub fn insert_preset_layers(
        &mut self,
        preset: EnvironmentRecord,
    ) -> Result<(), EnvironmentEditError> {
        self.layers
            .try_reserve(preset.layers.len())
            .map_err(|_| EnvironmentEditError::Allocation)?;
        self.layers.splice(..0, preset.layers);
        Ok(())
    }

    pub fn remove_layer(&mut self, index: usize) -> Result<(), EnvironmentEditError> {
        if index >= self.layers.len() {
            return Err(EnvironmentEditError::Layer);
        }
        self.layers.remove(index);
        Ok(())
    }

    pub fn clear_layers(&mut self) {
        self.layers.clear();
    }

    pub fn move_layer(&mut self, from: usize, to: usize) -> Result<(), EnvironmentEditError> {
        if from >= self.layers.len() || to >= self.layers.len() {
            return Err(EnvironmentEditError::Layer);
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        Ok(())
    }

    pub fn set_light(
        &mut self,
        index: usize,
        light: EnvironmentLight,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        let LayerSource::Light(properties) = &mut Arc::make_mut(layer).source else {
            return Err(EnvironmentEditError::Field);
        };
        set_integer(&mut properties[0], light as i32);
        Ok(())
    }

    fn transform(&self, layer: Option<usize>) -> Option<&Transform> {
        match layer {
            None => Some(&self.transform),
            Some(index) => Some(&self.layers.get(index)?.transform),
        }
    }

    fn transform_mut(&mut self, layer: Option<usize>) -> Option<&mut Transform> {
        match layer {
            None => Some(&mut self.transform),
            Some(index) => Some(&mut Arc::make_mut(self.layers.get_mut(index)?).transform),
        }
    }

    pub fn transform_filter(&self, layer: Option<usize>) -> Option<EnvironmentTransformFilter> {
        let ModernPropertyValue::Selected { current, .. } =
            self.transform(layer)?.properties[3].value()
        else {
            return None;
        };
        EnvironmentTransformFilter::from_index(*current)
    }

    pub fn set_transform_filter(
        &mut self,
        layer: Option<usize>,
        filter: EnvironmentTransformFilter,
    ) -> Result<(), EnvironmentEditError> {
        if self.transform_filter(layer) == Some(filter) {
            return Ok(());
        }
        let transform = self
            .transform_mut(layer)
            .ok_or(EnvironmentEditError::Layer)?;
        set_integer(&mut transform.properties[3], filter as i32);
        let values = transform.filter_values[filter as usize].unwrap_or_else(|| filter.defaults());
        for (property, value) in transform.properties[4..].iter_mut().zip(values) {
            property.overwrite_double_current(value);
            property.copy_double_current_to_previous();
        }
        Ok(())
    }

    pub fn blend_mode(&self, index: usize) -> Option<EnvironmentBlendMode> {
        EnvironmentBlendMode::read(&self.layers.get(index)?.blend[0])
    }

    pub fn set_blend_mode(
        &mut self,
        index: usize,
        mode: EnvironmentBlendMode,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        set_integer(&mut Arc::make_mut(layer).blend[0], mode as i32);
        Ok(())
    }

    pub fn mixer_enabled(&self, index: usize) -> bool {
        self.layers
            .get(index)
            .is_some_and(|layer| layer.mixer_enabled())
    }

    pub fn set_mixer_enabled(
        &mut self,
        index: usize,
        enabled: bool,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        *Arc::make_mut(layer).blend[3].value_mut() = ModernPropertyValue::Boolean(enabled);
        Ok(())
    }

    pub fn mask_hemisphere(&self, index: usize) -> Option<EnvironmentMaskHemisphere> {
        self.layers.get(index).map(|layer| layer.mask.hemisphere)
    }

    pub fn set_mask_hemisphere(
        &mut self,
        index: usize,
        hemisphere: EnvironmentMaskHemisphere,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        Arc::make_mut(layer).mask.hemisphere = hemisphere;
        Ok(())
    }

    pub fn mask_fill(&self, index: usize) -> Option<EnvironmentMaskFill> {
        self.layers.get(index).map(|layer| layer.mask.fill)
    }

    pub fn set_mask_fill(
        &mut self,
        index: usize,
        fill: EnvironmentMaskFill,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        Arc::make_mut(layer).mask.fill = fill;
        Ok(())
    }

    pub fn copy_mask_gradient(&self, index: usize) -> Option<crate::SharedGradient> {
        let layer = self.layers.get(index)?;
        (layer.mask.hemisphere != EnvironmentMaskHemisphere::Off
            && layer.mask.fill == EnvironmentMaskFill::Gradient)
            .then(|| crate::SharedGradient::from_wire_record(layer.mask.gradient.clone()))
    }

    pub fn set_mask_gradient(
        &mut self,
        index: usize,
        gradient: &crate::SharedGradient,
    ) -> Result<(), EnvironmentEditError> {
        let layer = self
            .layers
            .get_mut(index)
            .ok_or(EnvironmentEditError::Layer)?;
        if layer.mask.fill != EnvironmentMaskFill::Gradient {
            return Err(EnvironmentEditError::Field);
        }
        let record = gradient
            .wire_record()
            .map_err(EnvironmentEditError::Gradient)?;
        Arc::make_mut(layer).mask.gradient = record;
        Ok(())
    }

    pub fn recolor_enabled(&self, index: usize) -> bool {
        self.layers
            .get(index)
            .is_some_and(|layer| layer.recolor_enabled())
    }

    pub fn set_recolor_enabled(
        &mut self,
        index: usize,
        enabled: bool,
    ) -> Result<(), EnvironmentEditError> {
        let layer = Arc::make_mut(
            self.layers
                .get_mut(index)
                .ok_or(EnvironmentEditError::Layer)?,
        );
        if enabled && layer.recolor.is_none() {
            let gradient = crate::SharedGradient::new();
            gradient
                .add_normalized_color(0., 0., 0., 0.)
                .map_err(EnvironmentEditError::Gradient)?;
            gradient
                .add_normalized_color(1., 1., 1., 1.)
                .map_err(EnvironmentEditError::Gradient)?;
            let mut property = Property::Gradient.record();
            *property.value_mut() =
                ModernPropertyValue::Gradient(Some(SharedGradientRecord::from_live(gradient)));
            layer.recolor = Some(property);
        }
        *layer.color[3].value_mut() = ModernPropertyValue::Boolean(enabled);
        Ok(())
    }

    pub fn copy_recolor_gradient(
        &self,
        index: usize,
    ) -> Result<Option<crate::SharedGradient>, crate::GradientError> {
        let Some(layer) = self
            .layers
            .get(index)
            .filter(|layer| layer.recolor_enabled())
        else {
            return Ok(None);
        };
        let value = layer
            .recolor_value()
            .ok_or(crate::GradientError::NullReference)?;
        value.live().source_clone().map(Some)
    }

    pub fn set_recolor_gradient(
        &mut self,
        index: usize,
        gradient: &crate::SharedGradient,
    ) -> Result<(), EnvironmentEditError> {
        let layer = Arc::make_mut(
            self.layers
                .get_mut(index)
                .ok_or(EnvironmentEditError::Layer)?,
        );
        if !layer.recolor_enabled() {
            return Err(EnvironmentEditError::Field);
        }
        let property = layer.recolor.as_mut().ok_or(EnvironmentEditError::Field)?;
        let copy = gradient
            .source_clone()
            .map_err(EnvironmentEditError::Gradient)?;
        *property.value_mut() =
            ModernPropertyValue::Gradient(Some(SharedGradientRecord::from_live(copy)));
        Ok(())
    }

    pub fn field(&self, field: EnvironmentField) -> Option<f64> {
        if let Some((index, slot)) = field.transform_slot() {
            let filter = self.transform_filter(index);
            let available = match field {
                EnvironmentField::FilterRadius(_) => matches!(
                    filter,
                    Some(EnvironmentTransformFilter::Diffuse | EnvironmentTransformFilter::Scatter)
                ),
                EnvironmentField::FilterStrength(_) => {
                    filter == Some(EnvironmentTransformFilter::Scatter)
                }
                EnvironmentField::FilterHorizontal(_) | EnvironmentField::FilterVertical(_) => {
                    matches!(
                        filter,
                        Some(
                            EnvironmentTransformFilter::Anisotropic
                                | EnvironmentTransformFilter::LegacyAnisotropic
                        )
                    )
                }
                _ => true,
            };
            if !available {
                return None;
            }
            return self.transform(index)?.properties[slot]
                .double_pair()
                .map(|(value, _)| value);
        }
        let layer = field.layer().and_then(|i| self.layers.get(i));
        if matches!(field, EnvironmentField::MaskBlur(_)) {
            return Some(layer?.mask.amount);
        }
        if let Some((color, channel)) = field.mask_channel() {
            return Some(layer?.mask.colors[color][channel]);
        }
        let (property, channel) = edit_property(&self.master, layer.map(AsRef::as_ref), field)?;
        match property.value() {
            ModernPropertyValue::Double { current, .. } => Some(*current),
            ModernPropertyValue::Int32 { current, .. } => Some(f64::from(*current)),
            ModernPropertyValue::Color { argb, .. } => Some(argb[channel?]),
            _ => None,
        }
    }

    pub fn set_field(
        &mut self,
        field: EnvironmentField,
        value: f64,
    ) -> Result<bool, EnvironmentEditError> {
        let [min, max] = field.bounds();
        if !(min..=max).contains(&value) {
            return Err(EnvironmentEditError::Bounds);
        }
        let places = match field {
            EnvironmentField::Rotation(_)
            | EnvironmentField::Elevation(_)
            | EnvironmentField::MixerRotation(_)
            | EnvironmentField::MixerElevation(_)
            | EnvironmentField::Strength(_) => 0,
            EnvironmentField::Contrast(_)
            | EnvironmentField::Hue(_)
            | EnvironmentField::Saturation(_)
            | EnvironmentField::MaskBlur(_)
            | EnvironmentField::SkyHaze(_)
            | EnvironmentField::SkyDepth(_)
            | EnvironmentField::MixerAmount(_)
            | EnvironmentField::MixerContrast(_)
            | EnvironmentField::MixerLevel(_)
            | EnvironmentField::Heading(_)
            | EnvironmentField::Pitch(_)
            | EnvironmentField::Roll(_)
            | EnvironmentField::FilterRadius(_)
            | EnvironmentField::FilterStrength(_)
            | EnvironmentField::FilterHorizontal(_)
            | EnvironmentField::FilterVertical(_) => 1,
            EnvironmentField::Red(_)
            | EnvironmentField::Green(_)
            | EnvironmentField::Blue(_)
            | EnvironmentField::Tint(..)
            | EnvironmentField::MaskTint(..)
            | EnvironmentField::MaskColor(..) => 4,
            _ => 2,
        };
        let value = crate::round_decimal_places(value, places).expect("bounded Environment value");
        if self.field(field).ok_or(EnvironmentEditError::Field)? == value {
            return Ok(false);
        }
        if let Some((index, slot)) = field.transform_slot() {
            return self.set_transform_number(index, slot, value);
        }
        let layer = field
            .layer()
            .and_then(|i| self.layers.get_mut(i))
            .map(Arc::make_mut);
        if matches!(field, EnvironmentField::MaskBlur(_)) {
            layer.ok_or(EnvironmentEditError::Layer)?.mask.amount = value;
            return Ok(true);
        }
        if let Some((color, channel)) = field.mask_channel() {
            layer.ok_or(EnvironmentEditError::Layer)?.mask.colors[color][channel] = value;
            return Ok(true);
        }
        let (property, channel) = match field {
            EnvironmentField::Exposure => (&mut self.master[0], None),
            EnvironmentField::Gamma => (&mut self.master[1], None),
            _ => layer
                .and_then(|layer| layer.field_mut(field))
                .ok_or(EnvironmentEditError::Field)?,
        };
        match property.value_mut() {
            ModernPropertyValue::Double { current, previous } => {
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::Int32 { current, previous } => {
                *current = value as i32;
                *previous = *current;
            }
            ModernPropertyValue::Color { argb, .. } => argb[channel.expect("RGB channel")] = value,
            _ => return Err(EnvironmentEditError::Field),
        }
        Ok(true)
    }

    fn set_transform_number(
        &mut self,
        layer: Option<usize>,
        slot: usize,
        value: f64,
    ) -> Result<bool, EnvironmentEditError> {
        let filter = self.transform_filter(layer);
        let current = self.transform(layer).ok_or(EnvironmentEditError::Layer)?;
        if current.properties[slot]
            .double_pair()
            .map(|(current, _)| current)
            == Some(value)
        {
            return Ok(false);
        }
        let transform = self
            .transform_mut(layer)
            .ok_or(EnvironmentEditError::Layer)?;
        let property = &mut transform.properties[slot];
        property.overwrite_double_current(value);
        property.copy_double_current_to_previous();
        if let Some(filter) =
            filter.filter(|filter| *filter != EnvironmentTransformFilter::None && slot >= 4)
        {
            let values =
                transform.filter_values[filter as usize].get_or_insert_with(|| filter.defaults());
            values[slot - 4] = value;
        }
        Ok(true)
    }

    pub fn supports_numeric_adjuster(&self, receiver: EnvironmentNumericReceiver) -> bool {
        receiver.current(self).is_some()
    }

    pub fn numeric_adjuster_key(
        &self,
        receiver: EnvironmentNumericReceiver,
    ) -> Option<([u8; 16], &[u16])> {
        let adjustment = receiver.record(self)?.adjustment.as_ref()?;
        let component = adjustment.components().first()?;
        (matches!(adjustment.property_type, 0 | 4)
            && component.active
            && !component.name.is_empty())
        .then_some((adjustment.guid, component.name.as_slice()))
    }

    pub(crate) fn count_numeric_adjuster(&self, group: [u8; 16], name: &[u16]) -> usize {
        self.numeric_receivers()
            .filter(|&receiver| self.numeric_adjuster_key(receiver) == Some((group, name)))
            .count()
    }

    pub(crate) fn numeric_receivers(
        &self,
    ) -> impl Iterator<Item = EnvironmentNumericReceiver> + use<> {
        use EnvironmentField as Field;
        use EnvironmentNumericReceiver as Receiver;
        let transform = |owner| {
            [
                Receiver::Field(Field::Heading(owner)),
                Receiver::Field(Field::Pitch(owner)),
                Receiver::Field(Field::Roll(owner)),
                Receiver::Filter(owner),
                Receiver::Field(Field::FilterRadius(owner)),
                Receiver::Field(Field::FilterStrength(owner)),
            ]
        };
        [
            Receiver::Field(Field::Exposure),
            Receiver::Field(Field::Gamma),
        ]
        .into_iter()
        .chain(transform(None))
        .chain((0..self.layer_count()).flat_map(move |index| {
            [
                Receiver::Blend(index),
                Receiver::Field(Field::Strength(index)),
                Receiver::Field(Field::BlendGamma(index)),
                Receiver::Field(Field::MixerAmount(index)),
                Receiver::Field(Field::MixerContrast(index)),
                Receiver::Field(Field::MixerLevel(index)),
                Receiver::Field(Field::MixerRotation(index)),
                Receiver::Field(Field::MixerElevation(index)),
                Receiver::Field(Field::Contrast(index)),
                Receiver::Field(Field::Hue(index)),
                Receiver::Field(Field::Saturation(index)),
                Receiver::Field(Field::ImageGamma(index)),
                Receiver::Field(Field::ImageExposure(index)),
                Receiver::Light(index),
                Receiver::Field(Field::Rotation(index)),
                Receiver::Field(Field::Elevation(index)),
                Receiver::Field(Field::SkyHaze(index)),
                Receiver::Field(Field::SkyDepth(index)),
            ]
            .into_iter()
            .chain(transform(Some(index)))
        }))
    }

    /// Stage matches in Genetica's property enumeration order within each
    /// adjuster, then apply adjusters in the caller's order.
    pub(crate) fn map_numeric_adjustments<E>(
        &self,
        mut sample: impl FnMut(
            &crate::AdjustmentRecord,
        ) -> Result<Option<(usize, f64)>, crate::PointwiseError<E>>,
    ) -> Result<Option<Self>, crate::PointwiseError<E>> {
        let mut changes = Vec::new();
        for receiver in self.numeric_receivers() {
            if let Some(adjustment) = receiver
                .record(self)
                .and_then(|record| record.adjustment.as_ref())
                && let Some((order, value)) = sample(adjustment)?
            {
                changes.push((order, receiver, value));
            }
        }
        if changes.is_empty() {
            return Ok(None);
        }
        changes.sort_by_key(|(order, _, _)| *order);
        let mut map = self.clone();
        let mut changed = false;
        for (_, receiver, value) in changes {
            changed |= receiver.apply(&mut map, value)?;
        }
        Ok(changed.then_some(map))
    }

    #[cfg(test)]
    pub(crate) fn install_test_adjustment(
        &mut self,
        receiver: impl Into<EnvironmentNumericReceiver>,
        adjustment: crate::AdjustmentRecord,
    ) {
        receiver.into().record_mut(self).unwrap().adjustment = Some(adjustment);
    }
}

fn new_layer_source(kind: EnvironmentLayerKind) -> LayerSource {
    match kind {
        EnvironmentLayerKind::Image => LayerSource::Image {
            tone: Box::new([GAMMA.with_current(1.), EXPOSURE.record()]),
            asset: None,
            projection: 0,
        },
        EnvironmentLayerKind::Color => LayerSource::Color(Box::new(COLOR.record())),
        EnvironmentLayerKind::Light(light) => LayerSource::Light(Box::new([
            selected(4, light as i32),
            ANGLE.with_current(45.),
            ELEVATION.with_current(35.),
            PERCENT.record(),
            PERCENT.record(),
        ])),
    }
}

impl EnvironmentNumericReceiver {
    pub fn label(self) -> &'static str {
        match self {
            Self::Field(field) => field.label(),
            Self::Light(_) => "Light type",
            Self::Blend(_) => "Blend mode",
            Self::Filter(None) => "Master filter",
            Self::Filter(Some(_)) => "Layer filter",
        }
    }

    pub fn is_choice(self) -> bool {
        !matches!(self, Self::Field(_))
    }

    pub(crate) fn options(self) -> Vec<&'static str> {
        match self {
            Self::Light(_) => EnvironmentLight::ALL.map(EnvironmentLight::label).to_vec(),
            Self::Blend(_) => EnvironmentBlendMode::ALL
                .map(EnvironmentBlendMode::label)
                .to_vec(),
            Self::Filter(_) => EnvironmentTransformFilter::ALL
                .map(EnvironmentTransformFilter::label)
                .to_vec(),
            Self::Field(_) => Vec::new(),
        }
    }

    pub(crate) fn current(self, map: &EnvironmentRecord) -> Option<f64> {
        let property = self.record(map)?;
        let definition = property.configuration().adjustment.as_ref()?;
        match (self, property.value(), definition.property_type) {
            (
                Self::Field(field),
                ModernPropertyValue::Double { .. } | ModernPropertyValue::Int32 { .. },
                0,
            ) => map.field(field),
            (_, ModernPropertyValue::Selected { current, .. }, 4) if self.is_choice() => {
                Some(f64::from(*current))
            }
            _ => None,
        }
    }

    pub(crate) fn mapping_bounds(self, map: &EnvironmentRecord) -> Option<[f32; 2]> {
        self.record(map)?
            .configuration()
            .adjustment
            .as_ref()?
            .bounds
            .first()
            .copied()
    }

    pub(crate) fn new_adjustment(
        self,
        map: &EnvironmentRecord,
        group: [u8; 16],
        name: &[u16],
    ) -> Option<crate::AdjustmentRecord> {
        let property = self.record(map)?;
        let definition = property.configuration().adjustment.as_ref()?;
        let mut adjustment = property
            .adjustment
            .clone()
            .filter(|existing| existing.property_type == definition.property_type)
            .unwrap_or_else(|| crate::AdjustmentRecord::new(&definition.bounds));
        adjustment.property_type = definition.property_type;
        adjustment.guid = group;
        let component = adjustment.components_mut().first_mut()?;
        component.name = name.to_vec();
        component.active = true;
        component.curve_mut();
        Some(adjustment)
    }

    pub(crate) fn scope_adjustment(
        self,
        map: &EnvironmentRecord,
        group: [u8; 16],
    ) -> Option<crate::AdjustmentRecord> {
        let mut adjustment = self.record(map)?.adjustment.clone()?;
        adjustment.guid = group;
        Some(adjustment)
    }

    pub(crate) fn set_adjustment(
        self,
        map: &mut EnvironmentRecord,
        adjustment: Option<crate::AdjustmentRecord>,
    ) -> Option<()> {
        self.record_mut(map)?.adjustment = adjustment;
        Some(())
    }

    fn record_mut(self, map: &mut EnvironmentRecord) -> Option<&mut ModernPropertyRecord> {
        match self {
            Self::Field(EnvironmentField::Exposure) => Some(&mut map.master[0]),
            Self::Field(EnvironmentField::Gamma) => Some(&mut map.master[1]),
            Self::Field(field) => {
                if let Some((layer, slot)) = field.transform_slot() {
                    return Some(&mut map.transform_mut(layer)?.properties[slot]);
                }
                let layer = Arc::make_mut(map.layers.get_mut(field.layer()?)?);
                layer.field_mut(field).map(|(property, _)| property)
            }
            Self::Blend(index) => Some(&mut Arc::make_mut(map.layers.get_mut(index)?).blend[0]),
            Self::Light(index) => match &mut Arc::make_mut(map.layers.get_mut(index)?).source {
                LayerSource::Light(light) => Some(&mut light[0]),
                _ => None,
            },
            Self::Filter(owner) => Some(&mut map.transform_mut(owner)?.properties[3]),
        }
    }

    fn record(self, map: &EnvironmentRecord) -> Option<&ModernPropertyRecord> {
        match self {
            Self::Field(field) => {
                if let Some((layer, slot)) = field.transform_slot() {
                    return map.transform(layer).map(|value| &value.properties[slot]);
                }
                let layer = field.layer().and_then(|index| map.layers.get(index));
                edit_property(&map.master, layer.map(AsRef::as_ref), field)
                    .map(|(record, _)| record)
            }
            Self::Blend(index) => Some(&map.layers.get(index)?.blend[0]),
            Self::Light(index) => match &map.layers.get(index)?.source {
                LayerSource::Light(light) => Some(&light[0]),
                _ => None,
            },
            Self::Filter(layer) => Some(&map.transform(layer)?.properties[3]),
        }
    }

    pub(crate) fn apply<E>(
        self,
        map: &mut EnvironmentRecord,
        value: f64,
    ) -> Result<bool, crate::PointwiseError<E>> {
        if let Self::Field(field) = self {
            let [min, max] = field.bounds();
            let value = if matches!(field, EnvironmentField::Strength(_)) {
                value.round_ties_even()
            } else {
                value
            };
            if !(min..=max).contains(&value) {
                return Ok(false);
            }
            let edited = if let Some((layer, slot)) = field.transform_slot() {
                let value = crate::round_decimal_places(value, 1)
                    .map_err(crate::PointwiseError::DecimalRounding)?;
                map.set_transform_number(layer, slot, value)
            } else {
                map.set_field(field, value)
            };
            return edited.map_err(|_| crate::PointwiseError::InvalidOperation);
        }
        let count = match self {
            Self::Light(_) => EnvironmentLight::ALL.len(),
            Self::Blend(_) => EnvironmentBlendMode::ALL.len(),
            Self::Filter(_) => EnvironmentTransformFilter::ALL.len(),
            Self::Field(_) => unreachable!(),
        };
        let index = crate::pointwise::choice_index_from_numeric(value, count)?;
        if matches!(self.record(map).map(ModernPropertyRecord::value),
            Some(ModernPropertyValue::Selected { current, .. }) if *current == index)
        {
            return Ok(false);
        }
        match self {
            Self::Light(layer) => map.set_light(layer, EnvironmentLight::ALL[index as usize]),
            Self::Blend(layer) => {
                map.set_blend_mode(layer, EnvironmentBlendMode::ALL[index as usize])
            }
            Self::Filter(layer) if index == 0 => {
                // Genetica's callback attempts to assign -1 to the two Double
                // properties. Their lower bound is zero, so the raw values stay.
                let transform = map
                    .transform_mut(layer)
                    .ok_or(crate::PointwiseError::InvalidOperation)?;
                set_integer(&mut transform.properties[3], 0);
                Ok(())
            }
            Self::Filter(layer) => {
                map.set_transform_filter(layer, EnvironmentTransformFilter::ALL[index as usize])
            }
            Self::Field(_) => unreachable!(),
        }
        .map_err(|_| crate::PointwiseError::InvalidOperation)?;
        Ok(true)
    }
}

fn edit_property<'a>(
    master: &'a [ModernPropertyRecord; 2],
    layer: Option<&'a EnvironmentLayer>,
    field: EnvironmentField,
) -> Option<(&'a ModernPropertyRecord, Option<usize>)> {
    use EnvironmentField as Field;
    if let Some(slot) = field.blend_slot() {
        return Some((&layer?.blend[slot], None));
    }
    match field {
        Field::Exposure => Some((&master[0], None)),
        Field::Gamma => Some((&master[1], None)),
        Field::Contrast(_) => Some((&layer?.color[0], None)),
        Field::Hue(_) => Some((&layer?.color[1], None)),
        Field::Saturation(_) => Some((&layer?.color[2], None)),
        Field::Tint(_, component @ 0..=2) => Some((&layer?.color[4], Some(component + 1))),
        _ => match (&layer?.source, field) {
            (LayerSource::Image { tone, .. }, Field::ImageGamma(_)) => Some((&tone[0], None)),
            (LayerSource::Image { tone, .. }, Field::ImageExposure(_)) => Some((&tone[1], None)),
            (LayerSource::Light(light), Field::Rotation(_)) => Some((&light[1], None)),
            (LayerSource::Light(light), Field::Elevation(_)) => Some((&light[2], None)),
            (LayerSource::Light(light), Field::SkyHaze(_)) => Some((&light[3], None)),
            (LayerSource::Light(light), Field::SkyDepth(_)) => Some((&light[4], None)),
            (LayerSource::Color(color), Field::Red(_)) => Some((color, Some(1))),
            (LayerSource::Color(color), Field::Green(_)) => Some((color, Some(2))),
            (LayerSource::Color(color), Field::Blue(_)) => Some((color, Some(3))),
            _ => None,
        },
    }
}

impl EnvironmentLayer {
    fn field_mut(
        &mut self,
        field: EnvironmentField,
    ) -> Option<(&mut ModernPropertyRecord, Option<usize>)> {
        use EnvironmentField as Field;
        if let Some(slot) = field.blend_slot() {
            return Some((&mut self.blend[slot], None));
        }
        match field {
            Field::Contrast(_) => Some((&mut self.color[0], None)),
            Field::Hue(_) => Some((&mut self.color[1], None)),
            Field::Saturation(_) => Some((&mut self.color[2], None)),
            Field::Tint(_, component @ 0..=2) => Some((&mut self.color[4], Some(component + 1))),
            _ => match (&mut self.source, field) {
                (LayerSource::Image { tone, .. }, Field::ImageGamma(_)) => {
                    Some((&mut tone[0], None))
                }
                (LayerSource::Image { tone, .. }, Field::ImageExposure(_)) => {
                    Some((&mut tone[1], None))
                }
                (LayerSource::Light(light), Field::Rotation(_)) => Some((&mut light[1], None)),
                (LayerSource::Light(light), Field::Elevation(_)) => Some((&mut light[2], None)),
                (LayerSource::Light(light), Field::SkyHaze(_)) => Some((&mut light[3], None)),
                (LayerSource::Light(light), Field::SkyDepth(_)) => Some((&mut light[4], None)),
                (LayerSource::Color(color), Field::Red(_)) => Some((color, Some(1))),
                (LayerSource::Color(color), Field::Green(_)) => Some((color, Some(2))),
                (LayerSource::Color(color), Field::Blue(_)) => Some((color, Some(3))),
                _ => None,
            },
        }
    }
}

fn selected(options: i32, index: i32) -> ModernPropertyRecord {
    let mut property = Property::Selection(options).record();
    set_integer(&mut property, index);
    property
}

fn set_integer(property: &mut ModernPropertyRecord, value: i32) {
    match property.value_mut() {
        ModernPropertyValue::Selected { current, previous }
        | ModernPropertyValue::Int32 { current, previous } => {
            *current = value;
            *previous = value;
        }
        _ => unreachable!("Environment integer property"),
    }
}
