//! A typed Lab Color supplies one RGB value to a retained texture property.
use super::{LayerColor, Master, Paint, SolidPaint, SynthesisRecord, SynthesisTexture};
use crate::AdjustmentRecord;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisColorTarget {
    Master,
    Layer(usize),
    Solid(usize),
}

pub(crate) type SynthesisColorKey<'a> = ([u8; 16], i32, &'a [u16]);

#[derive(Clone, Debug)]
pub(super) struct ColorLink {
    record: Arc<AdjustmentRecord>,
    rgb: Option<[f64; 3]>,
}

impl ColorLink {
    pub(super) fn new(record: AdjustmentRecord) -> Self {
        Self {
            record: Arc::new(record),
            rgb: None,
        }
    }

    pub(super) fn record(&self) -> &AdjustmentRecord {
        &self.record
    }
}

pub(crate) fn color_key(record: &AdjustmentRecord) -> Option<SynthesisColorKey<'_>> {
    let [component] = record.components() else {
        return None;
    };
    if record.property_type != 6 || !component.active || component.name.is_empty() {
        return None;
    }
    Some((record.guid, record.property_type, &component.name))
}

fn effective_rgb(raw: [f64; 4], link: Option<&ColorLink>) -> [f64; 3] {
    let [_, r, g, b] = raw;
    link.and_then(|link| link.rgb).unwrap_or([r, g, b])
}

impl Master {
    pub(super) fn effective_tint(&self) -> [f64; 3] {
        effective_rgb(self.color, self.color_adjustment.as_ref())
    }
}

impl LayerColor {
    pub(super) fn effective_tint(&self) -> [f64; 3] {
        effective_rgb(self.tint, self.tint_adjustment.as_ref())
    }
}

impl SolidPaint {
    pub(super) fn effective_color(&self) -> [f64; 3] {
        effective_rgb(self.color, self.color_adjustment.as_ref())
    }
}

impl SynthesisTexture {
    fn color_slot(&self, target: SynthesisColorTarget) -> Option<([f64; 4], Option<&ColorLink>)> {
        Some(match target {
            SynthesisColorTarget::Master => {
                (self.master.color, self.master.color_adjustment.as_ref())
            }
            SynthesisColorTarget::Layer(layer) => {
                let controls = &self.layers.get(layer)?.color_controls;
                (controls.tint, controls.tint_adjustment.as_ref())
            }
            SynthesisColorTarget::Solid(layer) => {
                let Paint::Solid(solid) = &self.layers.get(layer)?.paint else {
                    return None;
                };
                (solid.color, solid.color_adjustment.as_ref())
            }
        })
    }

    pub(super) fn color_slot_mut(
        &mut self,
        target: SynthesisColorTarget,
    ) -> Option<(&mut [f64; 4], &mut Option<ColorLink>)> {
        Some(match target {
            SynthesisColorTarget::Master => {
                (&mut self.master.color, &mut self.master.color_adjustment)
            }
            SynthesisColorTarget::Layer(layer) => {
                let controls = &mut Arc::make_mut(self.layers.get_mut(layer)?).color_controls;
                (&mut controls.tint, &mut controls.tint_adjustment)
            }
            SynthesisColorTarget::Solid(layer) => {
                let Paint::Solid(solid) = &mut Arc::make_mut(self.layers.get_mut(layer)?).paint
                else {
                    return None;
                };
                (&mut solid.color, &mut solid.color_adjustment)
            }
        })
    }

    pub(super) fn color_value(&self, target: SynthesisColorTarget) -> Option<[f64; 3]> {
        let (raw, link) = self.color_slot(target)?;
        Some(effective_rgb(raw, link))
    }

    pub(super) fn color_adjustment(
        &self,
        target: SynthesisColorTarget,
    ) -> Option<&AdjustmentRecord> {
        Some(self.color_slot(target)?.1?.record())
    }

    pub(super) fn set_color_adjustment(
        &mut self,
        target: SynthesisColorTarget,
        destination: Option<(AdjustmentRecord, [f64; 3])>,
    ) -> Option<bool> {
        let (raw, current) = self.color_slot(target)?;
        let (new_raw, next) = if let Some((record, rgb)) = destination {
            let key = color_key(&record)?;
            if current.is_some_and(|current| color_key(current.record()) == Some(key)) {
                return Some(false);
            }
            if !rgb.iter().all(|value| value.is_finite()) {
                return None;
            }
            (
                [1., raw[1], raw[2], raw[3]],
                Some(ColorLink {
                    record: Arc::new(record),
                    rgb: Some(rgb),
                }),
            )
        } else {
            let current = current?;
            (freeze_color(raw, current), None)
        };
        let (raw, link) = self.color_slot_mut(target)?;
        *raw = new_raw;
        *link = next;
        Some(true)
    }

    fn color_targets(&self) -> impl Iterator<Item = SynthesisColorTarget> + '_ {
        std::iter::once(SynthesisColorTarget::Master).chain(
            self.layers.iter().enumerate().flat_map(|(index, layer)| {
                [
                    Some(SynthesisColorTarget::Layer(index)),
                    matches!(layer.paint, Paint::Solid(_))
                        .then_some(SynthesisColorTarget::Solid(index)),
                ]
                .into_iter()
                .flatten()
            }),
        )
    }
}

fn freeze_color(raw: [f64; 4], link: &ColorLink) -> [f64; 4] {
    link.rgb.map_or(raw, |[r, g, b]| [1., r, g, b])
}

impl SynthesisRecord {
    pub(crate) fn visit_color_adjustments(&self, mut visit: impl FnMut(SynthesisColorKey<'_>)) {
        for target in self.0.color_targets() {
            if let Some(key) = self.0.color_adjustment(target).and_then(color_key) {
                visit(key);
            }
        }
    }

    pub(crate) fn map_color_adjustments(
        &self,
        mut resolve: impl FnMut(SynthesisColorKey<'_>) -> Option<[f64; 3]>,
    ) -> Option<Self> {
        self.transform_color_links(|raw, link| {
            let value = resolve(color_key(link.record())?)?;
            (value.iter().all(|channel| channel.is_finite())
                && link.rgb.map(|rgb| rgb.map(f64::to_bits)) != Some(value.map(f64::to_bits)))
            .then(|| {
                (
                    raw,
                    Some(ColorLink {
                        record: link.record.clone(),
                        rgb: Some(value),
                    }),
                )
            })
        })
    }

    pub(crate) fn without_color_adjustments(&self) -> Self {
        self.transform_color_links(|raw, link| Some((freeze_color(raw, link), None)))
            .unwrap_or_else(|| self.clone())
    }

    pub(crate) fn remap_color_adjustments(
        &self,
        mut remap: impl FnMut(SynthesisColorKey<'_>) -> Option<([u8; 16], Vec<u16>, Option<[f64; 3]>)>,
    ) -> Option<Self> {
        self.transform_color_links(|raw, link| {
            let (group, name, rgb) = remap(color_key(link.record())?)?;
            let mut record = link.record().clone();
            record.guid = group;
            record.components_mut()[0].name = name;
            Some((
                raw,
                Some(ColorLink {
                    record: Arc::new(record),
                    rgb: rgb.or(link.rgb),
                }),
            ))
        })
    }

    fn transform_color_links(
        &self,
        mut transform: impl FnMut([f64; 4], &ColorLink) -> Option<([f64; 4], Option<ColorLink>)>,
    ) -> Option<Self> {
        let mut texture = None;
        for target in self.0.color_targets() {
            if let Some((raw, Some(link))) = self.0.color_slot(target)
                && let Some((value, link)) = transform(raw, link)
            {
                let next = texture.get_or_insert_with(|| self.0.as_ref().clone());
                let (raw, binding) = next.color_slot_mut(target).expect("retained color target");
                *raw = value;
                *binding = link;
            }
        }
        texture.map(|texture| Self(Arc::new(texture)))
    }
}
