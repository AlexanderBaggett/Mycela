//! Plot geometry and painting shared by the Custom curve editors.
use crate::curve_axes::{INPUT_TICKS, output_fraction};
use gpui_kit::{
    Bounds, ContentMask, Hsla, PathBuilder, Pixels, Point, Task, Window, fill, point, px, size,
};
use std::time::Duration;

pub(crate) const SCROLL_INTERVAL: Duration = Duration::from_millis(50);
const VERTICAL_LIMIT: f32 = 1000.0;

#[derive(Clone, Copy)]
pub(crate) struct PlotSpace {
    pub(crate) area: Bounds<Pixels>,
    pub(crate) vertical: [f32; 2],
}

impl PlotSpace {
    pub(crate) fn to_screen(self, [x, y]: [f32; 2]) -> Point<Pixels> {
        point(
            self.area.left() + self.area.size.width * x,
            self.area.bottom() - self.area.size.height * output_fraction(y, self.vertical),
        )
    }

    pub(crate) fn to_curve(self, screen: Point<Pixels>) -> [f32; 2] {
        let [low, high] = self.vertical;
        [
            (screen.x - self.area.left()) / self.area.size.width,
            low + ((self.area.bottom() - screen.y) / self.area.size.height) * (high - low),
        ]
    }
}

pub(crate) struct PointDrag {
    pub(crate) space: PlotSpace,
    start: Point<Pixels>,
    offset: Point<Pixels>,
    pointer: Point<Pixels>,
    pub(crate) moved: bool,
    pub(crate) scroll: Option<Task<()>>,
}

impl PointDrag {
    pub(crate) fn new(space: PlotSpace, pointer: Point<Pixels>, offset: Point<Pixels>) -> Self {
        Self {
            space,
            start: pointer,
            offset,
            pointer,
            moved: false,
            scroll: None,
        }
    }

    pub(crate) fn move_pointer(&mut self, pointer: Point<Pixels>) {
        self.pointer = pointer;
        let delta = pointer - self.start;
        self.moved |= delta.x.abs().max(delta.y.abs()) >= px(3.);
    }

    pub(crate) fn position(&self, looped: bool) -> [f32; 2] {
        let [x, y] = self.space.to_curve(self.pointer + self.offset);
        let [low, high] = self.space.vertical;
        let x = if looped && !(0.0..=1.0).contains(&x) {
            x.rem_euclid(1.0)
        } else {
            x.clamp(0.0, 1.0)
        };
        [x, y.clamp(low, high)]
    }

    pub(crate) fn scroll_edge(&self) -> Option<ScrollEdge> {
        let area = self.space.area;
        if !self.moved || !(area.left()..=area.right()).contains(&self.pointer.x) {
            return None;
        }
        let edge_band = area.size.height / 7.;
        let [low, high] = self.space.vertical;
        if self.pointer.y <= area.top() + edge_band && high < VERTICAL_LIMIT {
            Some(ScrollEdge::Top)
        } else if self.pointer.y >= area.bottom() - edge_band && low > -VERTICAL_LIMIT {
            Some(ScrollEdge::Bottom)
        } else {
            None
        }
    }
}

pub(crate) enum ScrollEdge {
    Top,
    Bottom,
}

impl ScrollEdge {
    pub(crate) fn expand(self, [low, high]: &mut [f32; 2]) {
        let bound = match self {
            Self::Top => high,
            Self::Bottom => low,
        };
        let step = match bound.abs() {
            distance if distance < 250. => 3.,
            distance if distance < 500. => 5.,
            _ => 10.,
        };
        *bound = match self {
            Self::Top => (*bound + step).min(VERTICAL_LIMIT),
            Self::Bottom => (*bound - step).max(-VERTICAL_LIMIT),
        };
    }
}

pub(crate) fn fitted_range(points: &[([f32; 2], bool)]) -> [f32; 2] {
    let [low, high] = points.iter().fold(
        [VERTICAL_LIMIT, -VERTICAL_LIMIT],
        |[low, high], (position, _)| [low.min(position[1]), high.max(position[1])],
    );
    let padding = if high - low < 500. { 20. } else { 50. };
    [
        ((low / 10.).floor() * 10. - padding).clamp(-VERTICAL_LIMIT, -20.),
        ((high / 10.).ceil() * 10. + padding).clamp(120., VERTICAL_LIMIT),
    ]
}

pub(crate) struct CurvePlotColors {
    pub(crate) stroke: Hsla,
    pub(crate) grid: Hsla,
    pub(crate) marker: Hsla,
    pub(crate) outside: Hsla,
}

pub(crate) fn paint_custom_plot(
    space: PlotSpace,
    output_guides: &[f32],
    points: &[([f32; 2], bool)],
    samples: Option<&[[f32; 2]]>,
    colors: &CurvePlotColors,
    window: &mut Window,
) {
    if space.area.size.width <= px(0.) || space.area.size.height <= px(0.) {
        return;
    }
    let [low, high] = space.vertical;
    for [bottom, top] in [[low, 0.], [100., high]] {
        if bottom < top {
            let origin = space.to_screen([0., top]);
            let bottom = space.to_screen([0., bottom]).y;
            window.paint_quad(fill(
                Bounds::new(origin, size(space.area.size.width, bottom - origin.y)),
                colors.outside,
            ));
        }
    }
    let mut guides = PathBuilder::stroke(px(1.));
    for x in INPUT_TICKS {
        guides.move_to(space.to_screen([x, low]));
        guides.line_to(space.to_screen([x, high]));
    }
    for &y in output_guides {
        guides.move_to(space.to_screen([0., y]));
        guides.line_to(space.to_screen([1., y]));
    }
    if let Ok(path) = guides.build() {
        window.paint_path(path, colors.grid);
    }
    if let Some(samples) = samples
        && let Some((first, rest)) = samples.split_first()
    {
        let mut curve = PathBuilder::stroke(px(2.));
        curve.move_to(space.to_screen(*first));
        for &sample in rest {
            curve.line_to(space.to_screen(sample));
        }
        if let Ok(path) = curve.build() {
            window.with_content_mask(Some(ContentMask { bounds: space.area }), |window| {
                window.paint_path(path, colors.stroke);
            });
        }
    }
    for &(position, selected) in points {
        let center = space.to_screen(position);
        let radius = if selected { px(5.) } else { px(3.) };
        window.paint_quad(fill(
            Bounds::new(
                center - point(radius, radius),
                size(radius * 2., radius * 2.),
            ),
            if selected {
                colors.stroke
            } else {
                colors.marker
            },
        ));
    }
}
