use super::*;
use crate::{AdjustmentRecord, GradientError, RgbColor};

#[derive(Clone)]
pub(crate) enum EnvironmentLabReference {
    Color([f64; 3]),
    Gradient(SharedGradientRecord),
}

/// A layer property that can receive an existing Color or Gradient Lab control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentColorField {
    LayerColor(usize),
    Colorize(usize),
    Recolor(usize),
}

impl EnvironmentColorField {
    pub(crate) fn type_tag(self) -> i32 {
        match self {
            Self::LayerColor(_) | Self::Colorize(_) => 6,
            Self::Recolor(_) => 7,
        }
    }

    fn layer(self) -> usize {
        match self {
            Self::LayerColor(layer) | Self::Colorize(layer) | Self::Recolor(layer) => layer,
        }
    }

    fn property(self, layer: &EnvironmentLayer) -> Option<&ModernPropertyRecord> {
        match self {
            Self::LayerColor(_) => match &layer.source {
                LayerSource::Color(property) => Some(property),
                _ => None,
            },
            Self::Colorize(_) => Some(&layer.color[4]),
            Self::Recolor(_) => layer.recolor.as_ref(),
        }
    }

    fn property_mut(self, layer: &mut EnvironmentLayer) -> Option<&mut ModernPropertyRecord> {
        match self {
            Self::LayerColor(_) => match &mut layer.source {
                LayerSource::Color(property) => Some(property),
                _ => None,
            },
            Self::Colorize(_) => Some(&mut layer.color[4]),
            Self::Recolor(_) => layer.recolor.as_mut(),
        }
    }
}

impl EnvironmentRecord {
    pub fn color_adjuster_key(&self, field: EnvironmentColorField) -> Option<([u8; 16], &[u16])> {
        let prop = field.property(self.layers.get(field.layer())?)?;
        let adjustment = prop.adjustment.as_ref()?;
        let component = adjustment.components().first()?;
        (adjustment.property_type == field.type_tag()
            && component.active
            && !component.name.is_empty())
        .then_some((adjustment.guid, component.name.as_slice()))
    }

    pub(crate) fn color_receivers(&self) -> impl Iterator<Item = EnvironmentColorField> + use<> {
        (0..self.layers.len()).flat_map(|layer| {
            [
                EnvironmentColorField::LayerColor(layer),
                EnvironmentColorField::Colorize(layer),
                EnvironmentColorField::Recolor(layer),
            ]
        })
    }

    pub(crate) fn count_color_adjuster(&self, key: ([u8; 16], i32, &[u16])) -> usize {
        self.color_receivers()
            .filter(|&field| {
                field.type_tag() == key.1 && self.color_adjuster_key(field) == Some((key.0, key.2))
            })
            .count()
    }

    pub(crate) fn color_adjuster_value(
        &self,
        field: EnvironmentColorField,
    ) -> Option<EnvironmentLabReference> {
        let layer = self.layers.get(field.layer())?;
        if matches!(field, EnvironmentColorField::Recolor(_)) && !layer.recolor_enabled() {
            return None;
        }
        match field.property(layer)?.value() {
            ModernPropertyValue::Color {
                argb: [_, r, g, b], ..
            } => Some(EnvironmentLabReference::Color([*r, *g, *b])),
            ModernPropertyValue::Gradient(Some(value)) => {
                Some(EnvironmentLabReference::Gradient(value.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn rename_color_adjuster(&mut self, field: EnvironmentColorField, name: &[u16]) {
        let layer = Arc::make_mut(&mut self.layers[field.layer()]);
        let property = field
            .property_mut(layer)
            .expect("matched Color or Gradient field");
        let adjustment = property.adjustment.as_mut().expect("matched adjuster");
        adjustment.components_mut()[0].name = name.to_vec();
    }

    pub(crate) fn set_color_adjuster(
        &mut self,
        field: EnvironmentColorField,
        link: Option<([u8; 16], &[u16], EnvironmentLabReference)>,
    ) -> Result<(), GradientError> {
        let layer = Arc::make_mut(&mut self.layers[field.layer()]);
        let property = field
            .property_mut(layer)
            .expect("validated Environment field");
        if let Some((group, name, value)) = link {
            if let Some(value) = map_value(property, value)? {
                *property.value_mut() = value;
            }
            let mut adjustment = property
                .adjustment
                .take()
                .filter(|adjustment| adjustment.property_type == field.type_tag())
                .unwrap_or_else(|| {
                    let definition = property
                        .configuration()
                        .adjustment
                        .as_ref()
                        .expect("Color or Gradient wire definition");
                    AdjustmentRecord::new(&definition.bounds)
                });
            adjustment.property_type = field.type_tag();
            adjustment.guid = group;
            let component = &mut adjustment.components_mut()[0];
            component.name = name.to_vec();
            component.active = true;
            property.adjustment = Some(adjustment);
        } else {
            property.adjustment = None;
        }
        Ok(())
    }

    /// Resolve saved reference links without replacing the layer or adjustment metadata.
    pub(crate) fn map_reference_adjustments(
        &self,
        mut resolve: impl FnMut(
            &AdjustmentRecord,
        ) -> Result<Option<EnvironmentLabReference>, GradientError>,
    ) -> Result<Option<Self>, GradientError> {
        let mut replacement = None;
        for (index, layer) in self.layers.iter().enumerate() {
            let tint = map_property(&layer.color[4], &mut resolve)?;
            let gradient = match &layer.recolor {
                Some(property) => map_property(property, &mut resolve)?,
                None => None,
            };
            let source_color = match &layer.source {
                LayerSource::Color(property) => map_property(property, &mut resolve)?,
                _ => None,
            };
            if tint.is_none() && gradient.is_none() && source_color.is_none() {
                continue;
            }
            let map = replacement.get_or_insert_with(|| self.clone());
            let changed = Arc::make_mut(&mut map.layers[index]);
            if let Some(tint) = tint {
                *changed.color[4].value_mut() = tint;
            }
            if let Some(property) = &mut changed.recolor
                && let Some(gradient) = gradient
            {
                *property.value_mut() = gradient;
            }
            if let LayerSource::Color(property) = &mut changed.source
                && let Some(color) = source_color
            {
                *property.value_mut() = color;
            }
        }
        Ok(replacement)
    }

    #[cfg(test)]
    pub(crate) fn install_test_recolor_adjustment(
        &mut self,
        layer: usize,
        adjustment: AdjustmentRecord,
    ) {
        Arc::make_mut(&mut self.layers[layer])
            .recolor
            .as_mut()
            .unwrap()
            .adjustment = Some(adjustment);
    }
}

fn map_property(
    property: &ModernPropertyRecord,
    resolve: &mut impl FnMut(
        &AdjustmentRecord,
    ) -> Result<Option<EnvironmentLabReference>, GradientError>,
) -> Result<Option<ModernPropertyValue>, GradientError> {
    let Some(adjustment) = &property.adjustment else {
        return Ok(None);
    };
    resolve(adjustment)?.map_or(Ok(None), |value| map_value(property, value))
}

fn map_value(
    property: &ModernPropertyRecord,
    value: EnvironmentLabReference,
) -> Result<Option<ModernPropertyValue>, GradientError> {
    let mapped = match (property.value(), value) {
        (
            ModernPropertyValue::Color { argb, .. },
            EnvironmentLabReference::Color([red, green, blue]),
        ) => {
            let color = RgbColor::new(red, green, blue, 1.);
            (!RgbColor::from_argb(*argb).equals(color)).then_some(ModernPropertyValue::Color {
                argb: color.argb(),
                grayscale: false,
            })
        }
        (ModernPropertyValue::Gradient(current), EnvironmentLabReference::Gradient(gradient)) => {
            if let Some(current) = current
                && (current.same_reference(&gradient)
                    || current.live().source_equals(Some(&gradient.live()))?)
            {
                return Ok(None);
            }
            Some(ModernPropertyValue::Gradient(Some(gradient)))
        }
        _ => None,
    };
    Ok(mapped)
}
