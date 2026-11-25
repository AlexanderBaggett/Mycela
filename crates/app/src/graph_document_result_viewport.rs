//! Display-only transforms for a retained Results raster.
use gpui_kit::{Bounds, Pixels, Point, RenderImage, Size, Window, fill, point, px, size};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Weak},
};

pub(super) const ZOOM_LEVELS: [(f32, &str); 13] = [
    (1. / 64., "1.56%"),
    (1. / 32., "3.12%"),
    (1. / 16., "6.25%"),
    (1. / 8., "12.5%"),
    (1. / 4., "25%"),
    (1. / 2., "50%"),
    (1., "100%"),
    (2., "200%"),
    (4., "400%"),
    (8., "800%"),
    (16., "1600%"),
    (32., "3200%"),
    (64., "6400%"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Zoom {
    Fit,
    Level(usize),
}

impl Default for Zoom {
    fn default() -> Self {
        Self::Level(6)
    }
}

impl Zoom {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Fit => "Fit",
            Self::Level(index) => ZOOM_LEVELS[index].1,
        }
    }

    fn scale(self, area: Size<Pixels>, native: Size<Pixels>) -> f32 {
        match self {
            Self::Fit => (area.width / native.width).min(area.height / native.height),
            Self::Level(index) => ZOOM_LEVELS[index].0,
        }
    }

    pub(super) fn step(self, area: Size<Pixels>, native: Size<Pixels>, larger: bool) -> Self {
        let scale = self.scale(area, native);
        let index = if larger {
            ZOOM_LEVELS.iter().position(|(level, _)| *level > scale)
        } else {
            ZOOM_LEVELS.iter().rposition(|(level, _)| *level < scale)
        };
        index.map_or(self, Self::Level)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Display {
    pub(super) zoom: Zoom,
    pub(super) tiled: bool,
    offset: Point<Pixels>,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            zoom: Zoom::default(),
            tiled: true,
            offset: Point::default(),
        }
    }
}

impl Display {
    pub(super) fn set_tiled(&mut self, tiled: bool) {
        self.tiled = tiled;
        self.reset_position();
    }

    pub(super) fn reset_position(&mut self) {
        self.offset = Point::default();
    }

    pub(super) fn tile(self, area: Bounds<Pixels>, native: Size<Pixels>) -> Bounds<Pixels> {
        let scale = self.zoom.scale(area.size, native);
        let span = size(native.width * scale, native.height * scale);
        let origin = match self.zoom {
            Zoom::Fit => area.center() - point(span.width / 2., span.height / 2.),
            Zoom::Level(_) => area.origin + self.offset,
        };
        Bounds::new(origin, span)
    }

    pub(super) fn set_zoom(
        &mut self,
        next: Zoom,
        area: Bounds<Pixels>,
        native: Size<Pixels>,
        pivot: Point<Pixels>,
    ) {
        if self.zoom == next || area.size.width <= px(0.) || area.size.height <= px(0.) {
            return;
        }
        let old = self.tile(area, native);
        let ratio = next.scale(area.size, native) / self.zoom.scale(area.size, native);
        let mut anchor = pivot;
        if !self.tiled && old.size.width * ratio.min(1.) < area.size.width {
            anchor.x = area.left();
        }
        if !self.tiled && old.size.height * ratio.min(1.) < area.size.height {
            anchor.y = area.top();
        }
        self.offset = anchor - area.origin - (anchor - old.origin) * ratio;
        if !self.tiled {
            let minimum = point(
                (area.size.width - old.size.width * ratio).min(px(0.)),
                (area.size.height - old.size.height * ratio).min(px(0.)),
            );
            self.offset = self.offset.min(&Point::default()).max(&minimum);
        }
        self.zoom = next;
    }
}

#[derive(Default, Clone)]
pub(super) struct DenseTiles(Rc<RefCell<Option<DenseTileFrame>>>);

struct DenseTileFrame {
    source: Weak<RenderImage>,
    raster: Arc<RenderImage>,
    area: Size<Pixels>,
    tile: Bounds<Pixels>,
}

impl DenseTiles {
    // A viewport full of subpixel tiles should cost one draw, even at 1.56%.
    // Sample the original projected raster and cache the screen-sized repeat.
    fn raster(
        &self,
        area: Size<Pixels>,
        tile: Bounds<Pixels>,
        source: &Arc<RenderImage>,
    ) -> Option<Arc<RenderImage>> {
        let mut cached = self.0.borrow_mut();
        if let Some(frame) = cached.as_ref()
            && frame.area == area
            && frame.tile == tile
            && frame.source.as_ptr() == Arc::as_ptr(source)
        {
            return Some(frame.raster.clone());
        }
        let frame = source.size(0);
        let width = area.width.as_f32().ceil().clamp(1., 2048.) as u32;
        let height = area.height.as_f32().ceil().clamp(1., 2048.) as u32;
        let sample =
            |i: u32, count: u32, span: Pixels, start: Pixels, period: Pixels, length: i32| {
                let position = span * ((i as f32 + 0.5) / count as f32);
                let fraction = ((position - start) / period).rem_euclid(1.);
                ((fraction * length as f32) as usize).min(length as usize - 1)
            };
        let columns: Vec<_> = (0..width)
            .map(|x| {
                4 * sample(
                    x,
                    width,
                    area.width,
                    tile.origin.x,
                    tile.size.width,
                    frame.width.0,
                )
            })
            .collect();
        let bytes = source.as_bytes(0)?;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            let row = sample(
                y,
                height,
                area.height,
                tile.origin.y,
                tile.size.height,
                frame.height.0,
            ) * frame.width.0 as usize
                * 4;
            for &column in &columns {
                pixels.extend_from_slice(&bytes[row + column..row + column + 4]);
            }
        }
        let image = image::RgbaImage::from_raw(width, height, pixels)?;
        let raster = Arc::new(RenderImage::new(vec![image::Frame::new(image)]));
        *cached = Some(DenseTileFrame {
            source: Arc::downgrade(source),
            raster: raster.clone(),
            area,
            tile,
        });
        Some(raster)
    }
}

pub(super) fn paint(
    area: Bounds<Pixels>,
    tile: Bounds<Pixels>,
    tiled: bool,
    raster: &Arc<RenderImage>,
    dense: &DenseTiles,
    window: &mut Window,
) {
    if area.size.width <= px(0.) || area.size.height <= px(0.) {
        return;
    }
    if !tiled {
        let _ = window.paint_image(area, tile, Default::default(), raster.clone(), 0, false);
        return;
    }
    let phase = point(
        px(-(area.left() - tile.origin.x)
            .as_f32()
            .rem_euclid(tile.size.width.as_f32())),
        px(-(area.top() - tile.origin.y)
            .as_f32()
            .rem_euclid(tile.size.height.as_f32())),
    );
    let columns = ((area.size.width - phase.x) / tile.size.width).ceil() as usize;
    let rows = ((area.size.height - phase.y) / tile.size.height).ceil() as usize;
    if columns.saturating_mul(rows) > 512 {
        if let Some(raster) = dense.raster(area.size, Bounds::new(phase, tile.size), raster) {
            let _ = window.paint_image(area, area, Default::default(), raster, 0, false);
        }
        return;
    }
    for row in 0..rows {
        for column in 0..columns {
            let start =
                area.origin + phase + point(tile.size.width * column, tile.size.height * row);
            let cell = Bounds::new(start, tile.size);
            let _ = window.paint_image(area, cell, Default::default(), raster.clone(), 0, false);
        }
    }
}

pub(super) fn paint_checkerboard(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    light: gpui_kit::Hsla,
    dark: gpui_kit::Hsla,
) {
    let cell = window
        .rem_size()
        .max(bounds.size.width.max(bounds.size.height) / 64.);
    let columns = (bounds.size.width / cell).ceil() as usize;
    let rows = (bounds.size.height / cell).ceil() as usize;
    for row in 0..rows {
        for column in 0..columns {
            let start = bounds.origin + point(cell * column, cell * row);
            let tile = Bounds::new(start, size(cell, cell)).intersect(&bounds);
            window.paint_quad(fill(
                tile,
                if (column + row) % 2 == 0 { light } else { dark },
            ));
        }
    }
}
