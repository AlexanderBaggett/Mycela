//! Edits to the Levels controls. Imported histogram values keep their wire semantics.
use super::{Histogram, HistogramState, PointwiseParameter, WeakPointwiseParameter, guard};
use crate::{
    HistoryDirection, HistoryError, ImageError, PointwiseCustomObject, PointwiseError,
    PointwiseFilter, PointwiseKind, history::HistoryGraphEdit,
};

#[path = "levels_input_histogram.rs"]
mod input_histogram;
pub use input_histogram::LevelsInputHistogram;

const POINT_LIMIT: f32 = 10.0;
const MINIMUM_RANGE: f32 = 0.01;
const MIDPOINT_MARGIN: f32 = 1e-7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelsChannel {
    Rgb,
    Red,
    Green,
    Blue,
}

impl LevelsChannel {
    pub const ALL: [Self; 4] = [Self::Rgb, Self::Red, Self::Green, Self::Blue];

    fn index(self) -> usize {
        match self {
            Self::Rgb => 0,
            Self::Red => 1,
            Self::Green => 2,
            Self::Blue => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LevelsControlEdit {
    BlackPoint(f32),
    Gamma(f32),
    Midpoint(f32),
    WhitePoint(f32),
    Reset,
    Auto(LevelsPoints),
}

/// The source histogram stores a midpoint position and displays it as gamma.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelsPoints([f32; 3]);

impl LevelsPoints {
    const DEFAULT: Self = Self([0.0, 0.5, 1.0]);

    pub fn black_point(self) -> f32 {
        self.0[0]
    }

    pub fn midpoint(self) -> f32 {
        self.0[1]
    }

    pub fn white_point(self) -> f32 {
        self.0[2]
    }

    pub fn gamma(self) -> f32 {
        let [black, midpoint, white] = self.0;
        if midpoint == black {
            9.99
        } else if midpoint == white {
            0.01
        } else {
            let fraction = (midpoint - black) / (white - black);
            (libm::log(f64::from(fraction)) / libm::log(0.5)) as f32
        }
    }

    /// Apply Genetica's control constraints only after an explicit editor action.
    pub fn edited(self, edit: LevelsControlEdit) -> Result<Self, ImageError> {
        if edit == LevelsControlEdit::Reset {
            return Ok(Self::DEFAULT);
        }
        if let LevelsControlEdit::Auto(points) = edit {
            return self.edited_auto(points);
        }
        let [mut black, _, mut white] = self.0;
        if !self.0.into_iter().all(f32::is_finite) {
            return Err(ImageError::Argument);
        }
        let mut midpoint = match edit {
            LevelsControlEdit::BlackPoint(value) if value.is_finite() => {
                let new_black = value
                    .clamp(-POINT_LIMIT, POINT_LIMIT)
                    .min(white - MINIMUM_RANGE);
                if new_black < -POINT_LIMIT {
                    return Err(ImageError::Argument);
                }
                black = new_black;
                self.move_midpoint(black, white)
            }
            LevelsControlEdit::WhitePoint(value) if value.is_finite() => {
                let new_white = value
                    .clamp(-POINT_LIMIT, POINT_LIMIT)
                    .max(black + MINIMUM_RANGE);
                if new_white > POINT_LIMIT {
                    return Err(ImageError::Argument);
                }
                white = new_white;
                self.move_midpoint(black, white)
            }
            LevelsControlEdit::Gamma(value) if value.is_finite() => {
                (libm::pow(0.5, f64::from(value.clamp(0.01, 9.999))) * f64::from(white - black)
                    + f64::from(black)) as f32
            }
            LevelsControlEdit::Midpoint(value) if value.is_finite() => value,
            _ => return Err(ImageError::Argument),
        };
        if black >= white {
            return Err(ImageError::Argument);
        }
        if midpoint <= black {
            midpoint = (black + MIDPOINT_MARGIN).max(black.next_up());
        } else if midpoint >= white {
            midpoint = (white - MIDPOINT_MARGIN).min(white.next_down());
        }
        if midpoint <= black || midpoint >= white {
            return Err(ImageError::Argument);
        }
        Ok(Self([black, midpoint, white]))
    }

    fn move_midpoint(self, black: f32, white: f32) -> f32 {
        let [old_black, old_midpoint, old_white] = self.0.map(f64::from);
        if f64::from(black) == old_black && f64::from(white) == old_white {
            return self.midpoint();
        }
        let fraction_from_white = if old_white == old_black {
            0.5
        } else {
            (1.0 - (old_midpoint - old_black) / (old_white - old_black)) as f32
        };
        let black = f64::from(black);
        let white = f64::from(white);
        (black + (f64::from(fraction_from_white) - 1.0) * (black - white)) as f32
    }

    fn edited_auto(self, points: Self) -> Result<Self, ImageError> {
        let [target_black, midpoint, target_white] = points.0;
        if !points.0.into_iter().all(f32::is_finite) || target_black > target_white {
            return Err(ImageError::Argument);
        }
        let [mut black, _, mut white] = self.0;
        if !black.is_finite() || !white.is_finite() || black >= white {
            [black, _, white] = Self::DEFAULT.0;
        }
        // u7.a(float[]) changes white first when the new range starts past
        // the current white point. The midpoint from Auto is applied last.
        if white <= target_black {
            white = target_white.max(black + MINIMUM_RANGE).min(POINT_LIMIT);
            black = target_black.min(white - MINIMUM_RANGE).max(-POINT_LIMIT);
        } else {
            black = target_black.min(white - MINIMUM_RANGE).max(-POINT_LIMIT);
            white = target_white.max(black + MINIMUM_RANGE).min(POINT_LIMIT);
        }
        Self([black, midpoint, white]).edited(LevelsControlEdit::Midpoint(midpoint))
    }
}

#[derive(Clone, Copy)]
struct LevelsValues([LevelsPoints; 4]);

impl LevelsValues {
    fn read(state: &HistogramState) -> Result<Self, ImageError> {
        let mut channels = [LevelsPoints::DEFAULT; 4];
        for (index, points) in channels.iter_mut().enumerate() {
            points.0 = [
                state.arrays[0].read(index)?,
                state.arrays[1].read(index)?,
                state.arrays[2].read(index)?,
            ];
        }
        Ok(Self(channels))
    }

    fn matches(self, other: Self) -> bool {
        self.0
            .into_iter()
            .zip(other.0)
            .all(|(left, right)| left.0.map(f32::to_bits) == right.0.map(f32::to_bits))
    }

    fn with_points(mut self, channel: LevelsChannel, points: LevelsPoints) -> Self {
        let index = channel.index();
        for (component, value) in points.0.into_iter().enumerate() {
            if self.0[index].0[component] == value {
                continue;
            }
            if channel == LevelsChannel::Rgb {
                for channel in &mut self.0 {
                    channel.0[component] = value;
                }
            } else {
                self.0[index].0[component] = value;
                self.0[0].0[component] =
                    (self.0[1].0[component] + self.0[2].0[component] + self.0[3].0[component])
                        / 3.0;
            }
        }
        self
    }
}

/// A detached Levels edit. A draft can be rendered without writing its source graph.
#[derive(Clone, Copy)]
pub struct LevelsDraft {
    before: LevelsValues,
    after: LevelsValues,
}

impl LevelsDraft {
    pub fn points(&self, channel: LevelsChannel) -> LevelsPoints {
        self.after.0[channel.index()]
    }

    pub fn edit(
        &mut self,
        channel: LevelsChannel,
        edit: LevelsControlEdit,
    ) -> Result<(), ImageError> {
        let points = self.points(channel).edited(edit)?;
        self.after = self.after.with_points(channel, points);
        Ok(())
    }

    pub fn preview_filter<E: 'static>(&self) -> Result<PointwiseFilter<E>, PointwiseError<E>> {
        let filter = PointwiseFilter::new(PointwiseKind::Levels)?;
        let histogram = filter.parameter(0)?.histogram()?;
        histogram.replace_levels(LevelsValues([LevelsPoints::DEFAULT; 4]), self.after)?;
        Ok(filter)
    }
}

impl Histogram {
    pub fn levels_points(&self, channel: LevelsChannel) -> Result<LevelsPoints, ImageError> {
        Ok(LevelsValues::read(&guard(&self.0))?.0[channel.index()])
    }

    fn replace_levels(&self, before: LevelsValues, after: LevelsValues) -> Result<(), ImageError> {
        let state = guard(&self.0);
        if state.disabled || !before.matches(LevelsValues::read(&state)?) {
            return Err(ImageError::Argument);
        }
        for (index, points) in after.0.into_iter().enumerate() {
            for (array, value) in state.arrays.iter().zip(points.0) {
                array.write(index, value)?;
            }
        }
        Ok(())
    }
}

impl<E: 'static> PointwiseParameter<E> {
    pub fn levels_draft(&self) -> Result<LevelsDraft, PointwiseError<E>> {
        if self.owner().kind() != PointwiseKind::Levels {
            return Err(PointwiseError::InvalidCast);
        }
        let histogram = self.histogram()?;
        let before = LevelsValues::read(&guard(&histogram.0))?;
        Ok(LevelsDraft {
            before,
            after: before,
        })
    }

    /// Commit one Levels gesture and record it as one graph history entry.
    pub fn edit_levels(
        &self,
        channel: LevelsChannel,
        edit: LevelsControlEdit,
    ) -> Result<bool, PointwiseError<E>> {
        let mut draft = self.levels_draft()?;
        draft.edit(channel, edit)?;
        self.commit_levels_draft(&draft)
    }

    /// Apply all channels as one history command if the captured values still match.
    pub fn commit_levels_draft(&self, draft: &LevelsDraft) -> Result<bool, PointwiseError<E>> {
        if self.owner().kind() != PointwiseKind::Levels {
            return Err(PointwiseError::InvalidCast);
        }
        let object = self.custom_object()?.ok_or(PointwiseError::NullReference)?;
        let histogram = object.histogram().ok_or(PointwiseError::InvalidCast)?;
        let LevelsDraft { before, after } = *draft;
        if !before.matches(LevelsValues::read(&guard(&histogram.0))?) {
            return Err(PointwiseError::History(Box::new(
                HistoryError::InputChanged,
            )));
        }
        if before.matches(after) {
            return Ok(false);
        }
        apply_levels(&object, &histogram, before, after)?;
        if let Some(slot) = self
            .owner()
            .node()
            .texture_node_logic()
            .and_then(|logic| logic.slot())
            && let Some(history) = slot.history()
        {
            let sequence = history.edit_sequence();
            let edit = LevelsHistoryEdit {
                parameter: self.downgrade(),
                object: object.raw(),
                before,
                after,
            };
            if let Err(error) = history.record_graph_edit(
                slot.history_target(),
                "Levels".encode_utf16().collect(),
                Box::new(edit),
            ) {
                if history.edit_sequence() == sequence {
                    let _ = apply_levels(&object, &histogram, after, before);
                }
                return Err(PointwiseError::History(Box::new(error)));
            }
        }
        Ok(true)
    }
}

fn apply_levels<E: 'static>(
    object: &PointwiseCustomObject<E>,
    histogram: &Histogram,
    before: LevelsValues,
    after: LevelsValues,
) -> Result<(), PointwiseError<E>> {
    histogram.replace_levels(before, after)?;
    if let Err(error) = object.notify() {
        if histogram.replace_levels(after, before).is_ok() {
            let _ = object.notify();
        }
        return Err(error);
    }
    Ok(())
}

struct LevelsHistoryEdit<E> {
    parameter: WeakPointwiseParameter<E>,
    object: PointwiseCustomObject<E>,
    before: LevelsValues,
    after: LevelsValues,
}

impl<E: 'static> HistoryGraphEdit<E> for LevelsHistoryEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let property = self.parameter.upgrade().ok_or(HistoryError::NullTarget)?;
        let property_error = |error| HistoryError::Property(Box::new(error));
        let object = property
            .custom_object()
            .map_err(property_error)?
            .filter(|current| current.same_reference(&self.object))
            .ok_or(HistoryError::NullTarget)?;
        let histogram = object.histogram().ok_or(HistoryError::InputChanged)?;
        let (before, after) = match direction {
            HistoryDirection::Redo => (self.before, self.after),
            HistoryDirection::Undo => (self.after, self.before),
        };
        if !before.matches(
            LevelsValues::read(&guard(&histogram.0))
                .map_err(|error| property_error(error.into()))?,
        ) {
            return Err(HistoryError::InputChanged);
        }
        apply_levels(&object, &histogram, before, after).map_err(property_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContextUndoInfo, Gtx375GraphDocument, History, HistoryCommand, HistoryEffect,
        HistoryServices, HistoryTarget, PointwiseFilter,
    };
    use std::sync::Arc;

    struct TestHistory;

    impl HistoryServices<()> for TestHistory {
        fn route_property_history(
            &self,
            _: &History<()>,
            _: &HistoryTarget<()>,
            _: Option<&ContextUndoInfo<()>>,
        ) -> Result<Option<History<()>>, HistoryError<()>> {
            Ok(None)
        }

        fn resolve_command_context(
            &self,
            _: &History<()>,
            _: &HistoryCommand<()>,
        ) -> Result<Option<HistoryTarget<()>>, HistoryError<()>> {
            Ok(None)
        }

        fn effect(
            &self,
            _: &History<()>,
            _: Option<&HistoryCommand<()>>,
            _: HistoryEffect,
        ) -> Result<(), HistoryError<()>> {
            Ok(())
        }

        fn pending_commands(
            &self,
            _: &History<()>,
            _: &HistoryCommand<()>,
        ) -> Result<Vec<HistoryCommand<()>>, HistoryError<()>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn levels_editor_preserves_gamma_and_limits_points() {
        use LevelsControlEdit::*;
        let points = LevelsPoints::DEFAULT.edited(Gamma(2.0)).unwrap();
        assert_eq!(points.0, [0.0, 0.25, 1.0]);
        assert_eq!(points.gamma(), 2.0);
        let points = points.edited(BlackPoint(-0.25)).unwrap();
        assert_eq!(points.0, [-0.25, 0.0625, 1.0]);
        let points = points.edited(WhitePoint(20.)).unwrap();
        assert_eq!((points.white_point(), points.gamma()), (10.0, 2.0));
        let points = points.edited(WhitePoint(-500.)).unwrap();
        assert_eq!(points.white_point(), points.black_point() + MINIMUM_RANGE);
        assert!(points.black_point() < points.midpoint());
        let points = points.edited(Midpoint(100.)).unwrap();
        assert!(points.midpoint() < points.white_point());
        assert!(points.edited(Gamma(f32::NAN)).is_err());
        assert_eq!(points.edited(Reset).unwrap(), LevelsPoints::DEFAULT);
    }

    #[test]
    fn levels_editor_undo_keeps_imported_values_and_channel_selection() {
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Levels).unwrap();
        let parameter = filter.parameter(0).unwrap();
        let histogram = parameter.histogram().unwrap();
        let invalid_nan = f32::from_bits(0xffc0_0000);
        histogram.set_channel(1);
        histogram.set_component(1, invalid_nan).unwrap();
        histogram.set_channel(3);
        let mut doc = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
        let history = doc.bind_property_history(Arc::new(TestHistory)).unwrap();
        let red = LevelsChannel::Red;
        let blue = LevelsChannel::Blue;
        let sequence = history.edit_sequence();
        assert!(
            parameter
                .edit_levels(red, LevelsControlEdit::Reset)
                .unwrap()
        );
        assert_eq!(history.edit_sequence(), sequence + 1);
        assert_eq!(histogram.levels_points(red).unwrap(), LevelsPoints::DEFAULT);
        assert_eq!(
            histogram.levels_points(blue).unwrap(),
            LevelsPoints::DEFAULT
        );
        histogram.set_channel(2);
        history.undo().unwrap();
        assert_eq!(
            histogram.levels_points(red).unwrap().midpoint().to_bits(),
            invalid_nan.to_bits()
        );
        assert_eq!(histogram.selected_channel(), 2);
        history.redo().unwrap();
        assert_eq!(histogram.levels_points(red).unwrap(), LevelsPoints::DEFAULT);
        assert!(
            !parameter
                .edit_levels(red, LevelsControlEdit::Reset)
                .unwrap()
        );
    }

    #[test]
    fn levels_batch_applies_channels_as_one_history_command() {
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Levels).unwrap();
        let parameter = filter.parameter(0).unwrap();
        let histogram = parameter.histogram().unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
        let history = doc.bind_property_history(Arc::new(TestHistory)).unwrap();
        let red = LevelsChannel::Red;
        let blue = LevelsChannel::Blue;
        let mut draft = parameter.levels_draft().unwrap();
        draft.edit(red, LevelsControlEdit::Gamma(2.)).unwrap();
        draft
            .edit(blue, LevelsControlEdit::WhitePoint(0.75))
            .unwrap();
        let preview = draft
            .preview_filter::<()>()
            .unwrap()
            .parameter(0)
            .unwrap()
            .histogram()
            .unwrap();
        assert_eq!(preview.levels_points(red).unwrap(), draft.points(red));
        assert_eq!(preview.levels_points(blue).unwrap(), draft.points(blue));
        assert_eq!(histogram.levels_points(red).unwrap(), LevelsPoints::DEFAULT);
        assert_eq!(
            histogram.levels_points(blue).unwrap(),
            LevelsPoints::DEFAULT
        );
        assert_eq!(history.commands().len(), 0);
        assert!(parameter.commit_levels_draft(&draft).unwrap());
        assert_eq!(history.commands().len(), 1);
        assert_eq!(histogram.levels_points(red).unwrap(), draft.points(red));
        assert_eq!(histogram.levels_points(blue).unwrap(), draft.points(blue));
        assert!(
            matches!(parameter.commit_levels_draft(&draft), Err(PointwiseError::History(error))
            if matches!(*error, HistoryError::InputChanged))
        );
        history.undo().unwrap();
        assert_eq!(histogram.levels_points(red).unwrap(), LevelsPoints::DEFAULT);
        assert_eq!(
            histogram.levels_points(blue).unwrap(),
            LevelsPoints::DEFAULT
        );
        history.redo().unwrap();
        assert_eq!(histogram.levels_points(red).unwrap(), draft.points(red));
        assert_eq!(histogram.levels_points(blue).unwrap(), draft.points(blue));
    }
}
