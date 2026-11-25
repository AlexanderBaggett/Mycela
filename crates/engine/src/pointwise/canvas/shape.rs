use super::*;
mod authoring_path;
mod compound;
mod freehand;
mod optimize;
mod point_edit;
mod primitive;
mod segment;
pub use authoring_path::{CanvasPathEndpoint, CanvasPathHandle, CanvasPointKind};
pub use segment::CanvasPathSegment;
pub(super) use segment::CubicCoefficients;

pub(super) const MAX_ITEM_DEPTH: usize = 128;

#[derive(Clone, Debug)]
pub(super) enum CanvasShape {
    Group(ShapeGroup),
    Layer(Box<CanvasLayer>),
    Shape(Box<Shape>),
}

#[derive(Clone, Debug)]
pub(super) struct ShapeGroup {
    pub(super) identity: Arc<()>,
    pub(super) header: ItemHeader,
    pub(super) children: Vec<CanvasShape>,
}

#[derive(Clone, Debug)]
pub(super) struct Shape {
    pub(super) identity: Arc<()>,
    pub(super) header: ItemHeader,
    pub(super) kind: i32,
    pub(super) base: ShapeBase,
    details: ShapeDetails,
}

#[derive(Clone, Debug)]
pub(super) struct ShapeBase {
    segments: Vec<CurveSegment>,
    combine: i32,
    pub(super) transform: [f32; 7],
    pub(super) controls: Vec<(Vec<u16>, f32)>,
}

#[derive(Clone, Debug)]
struct CurveSegment {
    points: [f32; 8],
    modes: [i32; 2],
}

#[derive(Clone, Debug)]
enum ShapeDetails {
    Primitive,
    Path(CanvasPath),
    Text(Box<Text>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanvasPath {
    width: f32,
    contours: Vec<Contour>,
}

#[derive(Clone, Debug, PartialEq)]
struct Contour {
    closed: bool,
    points: Vec<CanvasPathPoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanvasPathPoint {
    coordinates: [f32; 6],
    mode: i32,
    flags: [bool; 3],
}

#[derive(Clone, Debug)]
struct Text {
    path: Option<TextPath>,
    text: Vec<u16>,
    font: Vec<u16>,
    size: [f32; 2],
    style: i32,
    alignment: i32,
    bounds: [f32; 4],
    glyph_paths: Option<Vec<Vec<i32>>>,
}

type TextPath = (ItemHeader, ShapeBase, CanvasPath);

pub(super) fn read_items<E: 'static>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    depth: usize,
    mut context: Option<&mut NodeWireReadContext<'_, E>>,
) -> Result<Vec<CanvasShape>, NodeWireError<E>> {
    if depth >= MAX_ITEM_DEPTH {
        return Err(NodeWireError::UnsupportedCapability(
            "Canvas shape hierarchy depth",
        ));
    }
    read_list(reader, 9, |reader| {
        let kind = reader.read_i32()?;
        if kind == 1 {
            return Ok(CanvasShape::Layer(Box::new(CanvasLayer::read(
                reader,
                version,
                depth + 1,
                context.as_deref_mut(),
            )?)));
        }
        let header = ItemHeader::read(reader, context.as_deref())?;
        if kind == 0 {
            return Ok(CanvasShape::Group(ShapeGroup {
                identity: Arc::new(()),
                header,
                children: read_items(reader, version, depth + 1, context.as_deref_mut())?,
            }));
        }
        if !(2..=21).contains(&kind) {
            return Err(BinaryError::Index.into());
        }
        let base = ShapeBase::read(reader)?;
        let details = match kind {
            2 => ShapeDetails::Path(CanvasPath::read(reader, version, &base)?),
            21 => ShapeDetails::Text(Box::new(Text::read(reader, version, context.as_deref())?)),
            _ => ShapeDetails::Primitive,
        };
        Ok(CanvasShape::Shape(Box::new(Shape {
            identity: Arc::new(()),
            header,
            kind,
            base,
            details,
        })))
    })
}

impl CanvasShape {
    pub(super) fn assign_new_identity(&mut self) {
        match self {
            Self::Group(group) => {
                group.identity = Arc::new(());
                for child in &mut group.children {
                    child.assign_new_identity();
                }
            }
            Self::Layer(layer) => layer.assign_new_identity(),
            Self::Shape(shape) => shape.identity = Arc::new(()),
        }
    }

    pub(super) fn primitive(primitive: CanvasPrimitive, bounds: CanvasShapeBounds) -> Self {
        let [x, y] = bounds.center;
        let [width, height] = bounds.size;
        Self::Shape(Box::new(Shape {
            identity: Arc::new(()),
            header: ItemHeader::new(primitive.name()),
            kind: primitive as i32,
            base: ShapeBase {
                segments: Vec::new(),
                combine: 6,
                transform: [x, y, width, height, 0., 0., 0.],
                controls: primitive
                    .default_controls()
                    .iter()
                    .map(|(name, value)| (name.encode_utf16().collect(), *value))
                    .collect(),
            },
            details: ShapeDetails::Primitive,
        }))
    }

    pub(super) fn primitive_outline(&self) -> Result<Vec<[f64; 2]>, PointwiseError> {
        let Self::Shape(shape) = self else {
            return Err(PointwiseError::InvalidCast);
        };
        let kind = CanvasPrimitive::from_tag(shape.kind).ok_or(PointwiseError::InvalidCast)?;
        primitive::outline(kind, &shape.base, OutlineSampling::Scanline)
    }

    pub(super) fn write<E>(
        &self,
        writer: &mut BinaryWriter,
        gtx: bool,
    ) -> Result<(), NodeWireError<E>> {
        match self {
            Self::Group(group) => {
                writer.write_i32(0)?;
                group.header.write(writer)?;
                write_list(writer, &group.children, |shape, writer| {
                    shape.write(writer, gtx)
                })?;
            }
            Self::Layer(layer) => {
                writer.write_i32(1)?;
                layer.write(writer, gtx)?;
            }
            Self::Shape(shape) => {
                writer.write_i32(shape.kind)?;
                shape.header.write(writer)?;
                shape.base.write(writer)?;
                match &shape.details {
                    ShapeDetails::Primitive => {}
                    ShapeDetails::Path(path) => path.write(writer)?,
                    ShapeDetails::Text(text) => text.write(writer, gtx)?,
                }
            }
        }
        Ok(())
    }
}

impl ShapeBase {
    fn read<E>(reader: &mut BinaryCursor<'_>) -> Result<Self, NodeWireError<E>> {
        let segments = read_list(reader, 40, |reader| {
            Ok(CurveSegment {
                points: read_floats(reader)?,
                modes: [reader.read_i32()?, reader.read_i32()?],
            })
        })?;
        let _: [f32; 16] = read_floats(reader)?;
        let combine = reader.read_i32()?;
        let mut transform: [f32; 7] = read_floats(reader)?;
        for dimension in &mut transform[2..4] {
            if dimension.abs() < 0.01 {
                *dimension = 0.01;
            }
        }
        let controls = read_list(reader, 5, |reader| {
            Ok((reader.read_string()?, reader.read_f32()?))
        })?;
        Ok(Self {
            segments,
            combine,
            transform,
            controls,
        })
    }

    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        write_list(writer, &self.segments, |segment, writer| {
            write_floats(writer, &segment.points)?;
            writer.write_i32(segment.modes[0])?;
            writer.write_i32(segment.modes[1])?;
            Ok(())
        })?;
        write_floats(writer, &[0.; 16])?;
        writer.write_i32(self.combine)?;
        write_floats(writer, &self.transform)?;
        write_list(writer, &self.controls, |(name, value), writer| {
            writer.write_utf16(name)?;
            Ok(writer.write_f32(*value)?)
        })
    }
}

impl CanvasPath {
    fn read<E>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
        base: &ShapeBase,
    ) -> Result<Self, NodeWireError<E>> {
        let width = reader.read_f32()?;
        let contours = if version <= 3.17 {
            read_list(reader, 5, |reader| {
                let segments = read_list(reader, 4, |reader| {
                    let index = reader.read_i32()?;
                    base.segments
                        .get(index as usize)
                        .ok_or(BinaryError::Index.into())
                })?;
                let closed = reader.read_bool()?;
                Contour::from_segments(&segments, closed)
            })?
        } else {
            read_list(reader, 5, |reader| {
                let closed = reader.read_bool()?;
                let points = read_list(reader, 31, |reader| {
                    Ok(CanvasPathPoint {
                        coordinates: read_floats(reader)?,
                        mode: reader.read_i32()?,
                        flags: [
                            reader.read_bool()?,
                            reader.read_bool()?,
                            reader.read_bool()?,
                        ],
                    })
                })?;
                Ok(Contour { closed, points })
            })?
        };
        Ok(Self { width, contours })
    }

    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        writer.write_f32(self.width)?;
        write_list(writer, &self.contours, |contour, writer| {
            writer.write_bool(contour.closed)?;
            write_list(writer, &contour.points, |point, writer| {
                write_floats(writer, &point.coordinates)?;
                writer.write_i32(point.mode)?;
                for &flag in &point.flags {
                    writer.write_bool(flag)?;
                }
                Ok(())
            })
        })
    }
}

impl Contour {
    fn from_segments<E>(
        segments: &[&CurveSegment],
        closed: bool,
    ) -> Result<Self, NodeWireError<E>> {
        let first = segments.first().ok_or(BinaryError::Index)?;
        let last = segments.last().ok_or(BinaryError::Index)?;
        let incoming = if closed {
            [last.points[4], last.points[5]]
        } else {
            [first.points[0], first.points[1]]
        };
        let mut points = vec![CanvasPathPoint::new(
            [first.points[0], first.points[1]],
            incoming,
            [first.points[2], first.points[3]],
            first.modes[0],
        )];
        for pair in segments.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            points.push(CanvasPathPoint::new(
                [a.points[6], a.points[7]],
                [a.points[4], a.points[5]],
                [b.points[2], b.points[3]],
                a.modes[1],
            ));
        }
        if !closed {
            let end = [last.points[6], last.points[7]];
            points.push(CanvasPathPoint::new(
                end,
                [last.points[4], last.points[5]],
                end,
                last.modes[1],
            ));
        }
        Ok(Self { closed, points })
    }
}

impl CanvasPathPoint {
    fn new(position: [f32; 2], incoming: [f32; 2], outgoing: [f32; 2], mode: i32) -> Self {
        Self {
            coordinates: [
                position[0],
                position[1],
                incoming[0] - position[0],
                incoming[1] - position[1],
                outgoing[0] - position[0],
                outgoing[1] - position[1],
            ],
            mode,
            flags: [false; 3],
        }
    }
}

pub(super) struct Outline {
    pub(super) contours: Vec<Vec<[f64; 2]>>,
    pub(super) combine: i32,
    pub(super) stroke: Option<Stroke>,
    pub(super) curve: Option<Vec<DistanceSegment>>,
}

#[derive(Clone, Copy)]
pub(super) struct DistanceSegment {
    pub(super) controls: [[f64; 2]; 4],
    pub(super) corners: [bool; 2],
    pub(super) rounded: [bool; 2],
}

#[derive(Clone, Copy)]
pub(super) enum OutlineSampling {
    Scanline,
    Distance,
    Stroke,
    DocumentBounds,
}

pub(super) struct Stroke {
    pub(super) lines: Vec<Vec<[f64; 2]>>,
    pub(super) radius: f64,
}

impl CanvasShape {
    pub(super) fn append_outlines<E>(
        &self,
        outlines: &mut Vec<Outline>,
        sampling: OutlineSampling,
    ) -> Result<(), super::super::PointwiseError<E>> {
        use super::super::PointwiseError;
        match self {
            Self::Group(group) if group.header.container_visible() => {
                for child in group.children.iter().rev() {
                    child.append_outlines(outlines, sampling)?;
                }
            }
            Self::Shape(shape) if shape.header.visible() => {
                let (contours, stroke, curve) = match (&shape.details, sampling) {
                    (ShapeDetails::Path(path), _) if path.is_open() => {
                        let curve = matches!(sampling, OutlineSampling::Distance)
                            .then(|| path.distance_segments(&shape.base));
                        (Vec::new(), Some(path.stroke(&shape.base)?), curve)
                    }
                    (ShapeDetails::Path(path), OutlineSampling::Distance)
                        if path.contours.len() == 1 =>
                    {
                        (
                            path.outline(&shape.base, sampling)?,
                            None,
                            Some(path.distance_segments(&shape.base)),
                        )
                    }
                    (ShapeDetails::Primitive, OutlineSampling::Distance) => {
                        let kind = CanvasPrimitive::from_tag(shape.kind)
                            .ok_or(PointwiseError::InvalidCast)?;
                        let geometry = primitive::distance_outline(kind, &shape.base)?;
                        (vec![geometry.vertices], None, Some(geometry.segments))
                    }
                    _ => (shape.sample_outline(sampling)?, None, None),
                };
                if !contours.is_empty() || stroke.is_some() {
                    outlines.push(Outline {
                        contours,
                        combine: shape.base.combine,
                        stroke,
                        curve,
                    });
                }
            }
            Self::Layer(layer) if layer.header.container_visible() => {
                return Err(PointwiseError::UnsupportedCapability(
                    "nested Canvas layer rendering",
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

impl Shape {
    pub(super) fn authored_corners(&self) -> [[f32; 2]; 4] {
        primitive::PrimitiveMatrix::authored_corners(&self.base)
    }

    pub(super) fn editable(&self) -> bool {
        match &self.details {
            ShapeDetails::Primitive => true,
            ShapeDetails::Path(_) => true,
            ShapeDetails::Text(_) => false,
        }
    }

    pub(super) fn document_bounds<E>(&self) -> Result<[[f32; 2]; 2], PointwiseError<E>> {
        let mut low = [f32::INFINITY; 2];
        let mut high = [f32::NEG_INFINITY; 2];
        for point in self
            .sample_outline::<E>(OutlineSampling::DocumentBounds)?
            .into_iter()
            .flatten()
        {
            for axis in 0..2 {
                let value = point[axis] as f32;
                low[axis] = low[axis].min(value);
                high[axis] = high[axis].max(value);
            }
        }
        if !low[0].is_finite() {
            let [x, y, w, h, ..] = self.base.transform;
            low = [x - 0.5 * w, y - 0.5 * h];
            high = [x + 0.5 * w, y + 0.5 * h];
        }
        if low.into_iter().chain(high).any(|value| !value.is_finite()) {
            return Err(PointwiseError::Argument);
        }
        Ok([low, high])
    }

    pub(super) fn outline<E>(&self) -> Result<Vec<Vec<[f64; 2]>>, super::super::PointwiseError<E>> {
        self.sample_outline(OutlineSampling::Scanline)
    }

    pub(super) fn guide_outline<E>(&self) -> Result<Vec<Vec<[f64; 2]>>, PointwiseError<E>> {
        let mut outlines = self.outline()?;
        if self.open_stroke_width().is_none() {
            for outline in &mut outlines {
                if let Some(first) = outline.first().copied() {
                    outline.push(first);
                }
            }
        }
        Ok(outlines)
    }

    fn sample_outline<E>(
        &self,
        sampling: OutlineSampling,
    ) -> Result<Vec<Vec<[f64; 2]>>, super::super::PointwiseError<E>> {
        use super::super::PointwiseError;
        match &self.details {
            ShapeDetails::Path(path) => path.outline(&self.base, sampling),
            ShapeDetails::Text(text) => {
                if let Some((_, base, path)) = &text.path {
                    return path.outline(base, sampling);
                }
                if text.text.is_empty() {
                    return Ok(Vec::new());
                }
                Err(PointwiseError::UnsupportedCapability(
                    "Canvas text without embedded glyphs",
                ))
            }
            ShapeDetails::Primitive => {
                let kind =
                    CanvasPrimitive::from_tag(self.kind).ok_or(PointwiseError::InvalidCast)?;
                primitive::outline(kind, &self.base, sampling).map(|points| vec![points])
            }
        }
    }

    pub(super) fn apply_path_transform(&mut self, previous: [f32; 7]) -> bool {
        let ShapeDetails::Path(path) = &mut self.details else {
            return true;
        };
        path.reproject(previous, self.base.transform)
    }
}

impl Contour {
    fn segments(&self, shift: [f64; 2]) -> impl Iterator<Item = ([[f64; 2]; 4], bool)> + '_ {
        self.segment_controls(shift, true)
    }

    fn segment_controls(
        &self,
        shift: [f64; 2],
        round: bool,
    ) -> impl Iterator<Item = ([[f64; 2]; 4], bool)> + '_ {
        // Saved vertices are in Canvas units and the handles are vertex-relative.
        self.points
            .iter()
            .zip(self.points.iter().cycle().skip(1))
            .take(self.points.len().saturating_sub(usize::from(!self.closed)))
            .map(move |(start, end)| {
                let mut points = segment_controls(start, end);
                if round {
                    points[0] = points[0].map(round_endpoint);
                    points[3] = points[3].map(round_endpoint);
                }
                for point in &mut points {
                    for axis in 0..2 {
                        point[axis] -= shift[axis];
                    }
                }
                (points, start.mode == 0 && end.mode == 0)
            })
    }
}

fn segment_controls(start: &CanvasPathPoint, end: &CanvasPathPoint) -> [[f64; 2]; 4] {
    [
        start.position(),
        start.handle(CanvasPathHandle::Outgoing),
        end.handle(CanvasPathHandle::Incoming),
        end.position(),
    ]
    .map(|point| point.map(|axis| quantize_point(f64::from(axis))))
}

impl CanvasPath {
    fn is_open(&self) -> bool {
        self.contours
            .iter()
            .any(|contour| !contour.closed && contour.points.len() > 1)
    }

    fn tile_shift(base: &ShapeBase) -> [f64; 2] {
        [base.transform[0], base.transform[1]]
            .map(|center| f64::from((center / 100.).floor() * 100.))
    }

    fn distance_segments(&self, base: &ShapeBase) -> Vec<DistanceSegment> {
        let shift = Self::tile_shift(base);
        self.contours
            .iter()
            .filter(|contour| contour.points.len() >= 2)
            .flat_map(|contour| {
                let ends = contour.points.iter().cycle().skip(1);
                contour
                    .segments(shift)
                    .zip(contour.points.iter().zip(ends))
                    .map(|((controls, _), (start, end))| DistanceSegment {
                        controls,
                        corners: [start.mode == 0, end.mode == 0],
                        rounded: [true; 2],
                    })
            })
            .collect()
    }

    fn outline<E>(
        &self,
        base: &ShapeBase,
        sampling: OutlineSampling,
    ) -> Result<Vec<Vec<[f64; 2]>>, super::super::PointwiseError<E>> {
        let document = matches!(sampling, OutlineSampling::DocumentBounds);
        let shift = if document {
            [0.; 2]
        } else {
            Self::tile_shift(base)
        };
        Ok(self
            .contours
            .iter()
            .filter(|contour| contour.points.len() >= 2)
            .map(|contour| {
                let mut outline = Vec::new();
                let mut last = None;
                for (points, straight) in contour.segment_controls(shift, !document) {
                    last = Some(points[3]);
                    if straight {
                        outline.push(if document {
                            points[0].map(round_endpoint)
                        } else {
                            points[0]
                        });
                    } else if document {
                        cubic(&mut outline, points, sampling);
                    } else {
                        subdivide(&mut outline, points, 0, sampling);
                    }
                }
                if let Some(last) = last
                    && (!contour.closed || self.is_open())
                {
                    outline.push(if document {
                        last.map(round_endpoint)
                    } else {
                        last
                    });
                }
                outline
            })
            .collect())
    }

    fn stroke<E>(&self, base: &ShapeBase) -> Result<Stroke, super::super::PointwiseError<E>> {
        if !self.width.is_finite() || self.width < 0. {
            return Err(super::super::PointwiseError::Argument);
        }
        let shift = Self::tile_shift(base);
        let mut lines = Vec::new();
        for contour in self
            .contours
            .iter()
            .filter(|contour| contour.points.len() >= 2)
        {
            let mut vertices = Vec::new();
            let mut end = [0.; 2];
            for (points, straight) in contour.segments(shift) {
                if !points.iter().flatten().all(|value| value.is_finite()) {
                    return Err(super::super::PointwiseError::Argument);
                }
                end = points[3];
                if straight || controls_near_line(points, 0.001) {
                    vertices.push(points[0]);
                } else {
                    subdivide(&mut vertices, points, 0, OutlineSampling::Stroke);
                }
            }
            vertices.push(end);
            lines.push(vertices);
        }
        Ok(Stroke {
            lines,
            radius: f64::from(self.width) * 0.5,
        })
    }

    fn reproject(&mut self, previous: [f32; 7], current: [f32; 7]) -> bool {
        if previous == current {
            return true;
        }
        let basis = |transform: [f32; 7]| {
            let [_, _, width, height, skew_x, skew_y, angle] = transform.map(f64::from);
            let (sin, cos) = (-angle.to_radians()).sin_cos();
            let rotate = |x, y| [x * cos - y * sin, x * sin + y * cos];
            [rotate(width, -0.5 * skew_y), rotate(-0.5 * skew_x, height)]
        };
        let [origin_x, origin_y] = [previous[0], previous[1]];
        let [center_x, center_y] = [current[0], current[1]];
        if previous[2..] == current[2..] {
            for point in self
                .contours
                .iter_mut()
                .flat_map(|contour| &mut contour.points)
            {
                point.coordinates[0] += center_x - origin_x;
                point.coordinates[1] += center_y - origin_y;
            }
            return true;
        }
        let [[a, c], [b, d]] = basis(previous);
        let determinant = a * d - b * c;
        if !determinant.is_finite() || determinant == 0. {
            return false;
        }
        let [u, v] = basis(current);
        let convert = |x: f64, y: f64| {
            let w = (d * x - b * y) / determinant;
            let h = (a * y - c * x) / determinant;
            [w * u[0] + h * v[0], w * u[1] + h * v[1]]
        };
        for point in self
            .contours
            .iter_mut()
            .flat_map(|contour| &mut contour.points)
        {
            let [x, y, ix, iy, ox, oy] = point.coordinates.map(f64::from);
            let position = convert(x - f64::from(origin_x), y - f64::from(origin_y));
            let incoming = convert(ix, iy);
            let outgoing = convert(ox, oy);
            point.coordinates = [
                position[0] + f64::from(center_x),
                position[1] + f64::from(center_y),
                incoming[0],
                incoming[1],
                outgoing[0],
                outgoing[1],
            ]
            .map(|value| value as f32);
        }
        true
    }
}

fn round_endpoint(value: f64) -> f64 {
    (value * 100.).round_ties_even() / 100.
}

pub(super) fn quantize_point(value: f64) -> f64 {
    (value * 100_000.).round() / 100_000.
}

fn cubic(outline: &mut Vec<[f64; 2]>, mut points: [[f64; 2]; 4], sampling: OutlineSampling) {
    if matches!(sampling, OutlineSampling::DocumentBounds) {
        append_sampled_bounds(outline, points);
        return;
    }
    points[0] = points[0].map(round_endpoint);
    points[3] = points[3].map(round_endpoint);
    // atu uses fd.k for scanline crossings. The two rounded endpoints
    // give the original segment a wider tolerance than its descendants.
    subdivide(outline, points, 0, sampling);
}

fn append_sampled_bounds(outline: &mut Vec<[f64; 2]>, [a, b, c, d]: [[f64; 2]; 4]) {
    fn distance([a, b]: [[f64; 2]; 2]) -> f64 {
        let [dx, dy] = [b[0] - a[0], b[1] - a[1]];
        (dx * dx + dy * dy).sqrt()
    }
    let count = ((distance([a, b]) + distance([b, c]) + distance([c, d])) / (100. / 1024.)).ceil();
    let rounded = [a.map(round_endpoint), d.map(round_endpoint)];
    if !(3. ..=1_000_000.).contains(&count) {
        outline.extend(rounded);
        if count > 1_000_000. {
            subdivide(outline, [a, b, c, d], 0, OutlineSampling::Distance);
        }
        return;
    }
    let count = count as usize;
    let step = f64::from(1.0f32 / count as f32);
    let square = step * step;
    let cube = square * step;
    let mut extrema = [[0.; 2]; 2];
    for axis in 0..2 {
        let w = 3. * (b[axis] - a[axis]);
        let v = 3. * (c[axis] - b[axis]) - w;
        let u = d[axis] - a[axis] - w - v;
        let mut value = a[axis];
        let mut first = u * cube + v * square + w * step;
        let mut second = 6. * u * cube + 2. * v * square;
        let third = 6. * u * cube;
        let mut low = rounded[0][axis].min(rounded[1][axis]);
        let mut high = rounded[0][axis].max(rounded[1][axis]);
        for _ in 1..count - 1 {
            value += first;
            first += second;
            second += third;
            low = low.min(value);
            high = high.max(value);
        }
        extrema[0][axis] = low;
        extrema[1][axis] = high;
    }
    outline.extend(extrema);
}

fn controls_near_line([a, b, c, d]: [[f64; 2]; 4], tolerance: f64) -> bool {
    let segment = [d[0] - a[0], d[1] - a[1]];
    let length_squared = segment[0] * segment[0] + segment[1] * segment[1];
    let near_line = |point: [f64; 2]| {
        let offset = [point[0] - a[0], point[1] - a[1]];
        let dot = offset[0] * segment[0] + offset[1] * segment[1];
        let t = if dot <= 0. {
            0.
        } else if length_squared <= dot {
            1.
        } else {
            dot / length_squared
        };
        (0..2)
            .map(|axis| point[axis] - (a[axis] + t * segment[axis]))
            .map(|difference| difference * difference)
            .sum::<f64>()
            < tolerance
    };
    near_line(b) && near_line(c)
}

fn subdivide(
    outline: &mut Vec<[f64; 2]>,
    points: [[f64; 2]; 4],
    depth: u8,
    sampling: OutlineSampling,
) {
    let tolerance = match sampling {
        OutlineSampling::Stroke => 1.0e-8,
        OutlineSampling::Distance | OutlineSampling::Scanline if depth == 0 => 0.001,
        // Genetica measures distance to the cubic rather than the scanline chord.
        OutlineSampling::Distance => 1.0e-6,
        OutlineSampling::Scanline => 0.0001,
        OutlineSampling::DocumentBounds => unreachable!(),
    };
    if depth == 16
        || !points.iter().flatten().all(|value| value.is_finite())
        || controls_near_line(points, tolerance)
    {
        outline.push(points[0]);
        return;
    }

    let [left, right] = split_cubic(points, 0.5);
    let quantize = |p: [f64; 2]| match sampling {
        OutlineSampling::Scanline => p.map(quantize_point),
        _ => p,
    };
    subdivide(outline, left.map(quantize), depth + 1, sampling);
    subdivide(outline, right.map(quantize), depth + 1, sampling);
}

pub(super) fn split_cubic([a, b, c, d]: [[f64; 2]; 4], t: f64) -> [[[f64; 2]; 4]; 2] {
    let interpolate = |first: [f64; 2], second: [f64; 2]| {
        std::array::from_fn(|axis| first[axis] + t * (second[axis] - first[axis]))
    };
    let ab = interpolate(a, b);
    let bc = interpolate(b, c);
    let cd = interpolate(c, d);
    let abc = interpolate(ab, bc);
    let bcd = interpolate(bc, cd);
    let middle = interpolate(abc, bcd);
    [[a, ab, abc, middle], [middle, bcd, cd, d]]
}

impl Text {
    fn read_path<E>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
        context: Option<&NodeWireReadContext<'_, E>>,
    ) -> Result<Option<TextPath>, NodeWireError<E>> {
        if reader.read_bool()? {
            let header = ItemHeader::read(reader, context)?;
            let base = ShapeBase::read(reader)?;
            let path = CanvasPath::read(reader, version, &base)?;
            Ok(Some((header, base, path)))
        } else {
            Ok(None)
        }
    }

    fn read<E>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
        context: Option<&NodeWireReadContext<'_, E>>,
    ) -> Result<Self, NodeWireError<E>> {
        // Genetica reads the path in both the base and text records.
        // The text record supplies the effective path, including clearing it.
        if !context.is_some_and(NodeWireReadContext::is_mycela_project) {
            Self::read_path(reader, version, context)?;
        }
        let path = Self::read_path(reader, version, context)?;
        let text = reader.read_string()?;
        let font = reader.read_string()?;
        let size = read_floats(reader)?;
        let style = reader.read_i32()?;
        let alignment = reader.read_i32()?;
        let bounds = read_floats(reader)?;
        let glyph_paths = if reader.read_bool()? {
            Some(read_list(reader, 4, |reader| {
                read_list(reader, 4, |reader| Ok(reader.read_i32()?))
            })?)
        } else {
            None
        };
        Ok(Self {
            path,
            text,
            font,
            size,
            style,
            alignment,
            bounds,
            glyph_paths,
        })
    }

    fn write_path<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        writer.write_bool(self.path.is_some())?;
        if let Some((header, base, path)) = &self.path {
            header.write(writer)?;
            base.write(writer)?;
            path.write(writer)?;
        }
        Ok(())
    }

    fn write<E>(&self, writer: &mut BinaryWriter, gtx: bool) -> Result<(), NodeWireError<E>> {
        if gtx {
            self.write_path(writer)?;
        }
        self.write_path(writer)?;
        writer.write_utf16(&self.text)?;
        writer.write_utf16(&self.font)?;
        write_floats(writer, &self.size)?;
        writer.write_i32(self.style)?;
        writer.write_i32(self.alignment)?;
        write_floats(writer, &self.bounds)?;
        writer.write_bool(self.glyph_paths.is_some())?;
        if let Some(paths) = &self.glyph_paths {
            write_list(writer, paths, |path, writer| {
                write_list(writer, path, |value, writer| Ok(writer.write_i32(*value)?))
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_canvas_text_without_an_outline_contributes_no_geometry() {
        let text = Text {
            path: None,
            text: Vec::new(),
            font: "Microsoft Sans Serif".encode_utf16().collect(),
            size: [20., 20.],
            style: 0,
            alignment: 0,
            bounds: [0.; 4],
            glyph_paths: None,
        };
        let shape = Shape {
            identity: Arc::new(()),
            header: ItemHeader::new("Text"),
            kind: 21,
            base: ShapeBase {
                segments: Vec::new(),
                combine: 6,
                transform: [50., 50., 1., 1., 0., 0., 0.],
                controls: Vec::new(),
            },
            details: ShapeDetails::Text(Box::new(text)),
        };
        let mut outlines = Vec::new();
        CanvasShape::Shape(Box::new(shape))
            .append_outlines::<()>(&mut outlines, OutlineSampling::Scanline)
            .unwrap();
        assert!(outlines.is_empty());
    }

    #[test]
    fn canvas_text_uses_the_second_gtx_path_and_one_native_path() {
        let path = (
            ItemHeader::new("Glyph outlines"),
            ShapeBase {
                segments: Vec::new(),
                combine: 6,
                transform: [50., 50., 20., 30., 0., 0., 0.],
                controls: Vec::new(),
            },
            CanvasPath {
                width: 3.,
                contours: Vec::new(),
            },
        );
        for effective_path in [None, Some(path.clone())] {
            let text = Text {
                path: effective_path,
                text: "Title".encode_utf16().collect(),
                font: "Example".encode_utf16().collect(),
                size: [48., 48.],
                style: 0,
                alignment: 1,
                bounds: [0., 0., 20., 30.],
                glyph_paths: None,
            };
            let mut native = BinaryWriter::default();
            text.write::<()>(&mut native, false).unwrap();
            let mut authored = BinaryWriter::default();
            // The first record can contain an outline that the second clears or replaces.
            authored.write_bool(true).unwrap();
            ItemHeader::new("Base outline")
                .write(&mut authored)
                .unwrap();
            path.1.write::<()>(&mut authored).unwrap();
            path.2.write::<()>(&mut authored).unwrap();
            authored.write_bytes(native.bytes()).unwrap();
            for (gtx, bytes) in [(true, authored.bytes()), (false, native.bytes())] {
                let mut reader = BinaryCursor::new(bytes).unwrap();
                let context = (!gtx).then(|| {
                    NodeWireReadContext::for_project(crate::NodeWireReadPolicy::new(1, 1), 2)
                });
                let loaded = Text::read::<()>(&mut reader, 3.71, context.as_ref()).unwrap();
                assert_eq!(reader.position().unwrap() as usize, bytes.len());
                assert_eq!(loaded.text, text.text);
                assert_eq!(
                    loaded.path.as_ref().map(|(header, _, _)| &header.name),
                    text.path.as_ref().map(|(header, _, _)| &header.name)
                );
            }
            let mut expected_gtx = BinaryWriter::default();
            text.write_path::<()>(&mut expected_gtx).unwrap();
            expected_gtx.write_bytes(native.bytes()).unwrap();
            let mut gtx = BinaryWriter::default();
            text.write::<()>(&mut gtx, true).unwrap();
            assert_eq!(gtx.bytes(), expected_gtx.bytes());
        }
    }
}
