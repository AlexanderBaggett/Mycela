use crate::MatrixWireRecord;
use std::{error::Error, fmt};

/// Source nullable point-array boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NullMatrixPoints;
impl NullMatrixPoints {
    pub const fn source_class(self) -> &'static str {
        "ArgumentNullException"
    }
}
impl fmt::Display for NullMatrixPoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("matrix points: ArgumentNullException")
    }
}
impl Error for NullMatrixPoints {}

/// The characterized unchecked CLR conversion. Rust's saturating float cast
/// would produce different results for NaN, infinity and overflow.
pub fn matrix_coordinate_i32(value: f64) -> i32 {
    if !(-2147483648.0..2147483648.0).contains(&value) {
        i32::MIN
    } else {
        value as i32
    }
}
impl MatrixWireRecord {
    pub fn copy_value(&self) -> Self {
        Self {
            elements: self.elements,
        }
    }
    pub fn reset(&mut self) {
        self.elements = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    pub fn determinant(&self) -> f64 {
        let [a, b, c, d, _, _] = self.elements;
        sub(mul(a, d), mul(b, c))
    }
    pub fn is_identity(&self) -> bool {
        let [a, b, c, d, x, y] = self.elements;
        a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 && x == 0.0 && y == 0.0
    }
    pub fn is_invertible(&self) -> bool {
        self.determinant() != 0.0
    }
    pub fn translate(&mut self, x: f64, y: f64, prepend: bool) {
        let [a, b, c, d, _, _] = self.elements;
        let (x, y) = if prepend {
            (add(mul(x, a), mul(y, c)), add(mul(x, b), mul(y, d)))
        } else {
            (x, y)
        };
        self.elements[4] = add(self.elements[4], x);
        self.elements[5] = add(self.elements[5], y);
    }
    pub fn scale(&mut self, x: f64, y: f64, prepend: bool) {
        let factors = if prepend {
            [x, x, y, y, 1.0, 1.0]
        } else {
            [x, y, x, y, x, y]
        };
        let count = if prepend { 4 } else { 6 };
        for (field, factor) in self.elements.iter_mut().zip(factors).take(count) {
            *field = mul(*field, factor);
        }
    }
    pub fn rotate_degrees(&mut self, angle: f64, prepend: bool) {
        let radians = mul(angle, std::f64::consts::PI / 180.0);
        let cos = libm::cos(radians);
        let sin = libm::sin(radians);
        let [a, b, c, d, x, y] = self.elements;
        if prepend {
            self.elements[0] = add(mul(cos, a), mul(sin, c));
            self.elements[1] = add(mul(cos, b), mul(sin, d));
            self.elements[2] = sub(mul(cos, c), mul(sin, a));
            self.elements[3] = sub(mul(cos, d), mul(sin, b));
        } else {
            self.elements = [
                sub(mul(a, cos), mul(b, sin)),
                add(mul(a, sin), mul(b, cos)),
                sub(mul(c, cos), mul(d, sin)),
                add(mul(c, sin), mul(d, cos)),
                sub(mul(x, cos), mul(y, sin)),
                add(mul(x, sin), mul(y, cos)),
            ];
        }
    }
    pub fn rotate_about(&mut self, angle: f64, pivot: [f64; 2]) {
        self.translate(-pivot[0], -pivot[1], false);
        self.rotate_degrees(angle, false);
        self.translate(pivot[0], pivot[1], false);
    }
    pub fn invert(&mut self) {
        let det = self.determinant();
        if det != 0.0 {
            let reciprocal = 1.0 / det;
            let [a, b, c, d, x, y] = self.elements;
            self.elements = [
                mul(d, reciprocal),
                mul(-b, reciprocal),
                mul(-c, reciprocal),
                mul(a, reciprocal),
                mul(sub(mul(c, y), mul(d, x)), reciprocal),
                mul(sub(mul(b, x), mul(a, y)), reciprocal),
            ];
        }
    }
    /// The original single-value overload modifies a by-value struct and
    /// returns nothing. This returns its unchanged caller value explicitly.
    pub fn transform_value_i32(&self, point: [i32; 2]) -> [i32; 2] {
        point
    }
    pub fn transform_value_f32(&self, point: [f32; 2]) -> [f32; 2] {
        point
    }
    pub fn transform_value_f64(&self, point: [f64; 2]) -> [f64; 2] {
        point
    }
    /// vl's receiver method has matrix-first multiplication, unlike arrays.
    pub fn transform_f64_receiver(&self, point: &mut [f64; 2]) {
        let [a, b, c, d, x, y] = self.elements;
        let old_x = point[0];
        point[0] = add(add(mul(a, point[0]), mul(c, point[1])), x);
        point[1] = add(add(mul(b, old_x), mul(d, point[1])), y);
    }
    pub fn transform_points_i32(
        &self,
        points: Option<&mut [[i32; 2]]>,
    ) -> Result<(), NullMatrixPoints> {
        let points = points.ok_or(NullMatrixPoints)?;
        let [a, b, c, d, x, y] = self.elements;
        for p in points.iter_mut().rev() {
            let px = f64::from(p[0]);
            let py = f64::from(p[1]);
            p[0] = matrix_coordinate_i32(add(add(mul(px, a), mul(py, c)), x));
            p[1] = matrix_coordinate_i32(add(add(mul(px, b), mul(py, d)), y));
        }
        Ok(())
    }
    pub fn transform_points_f32(
        &self,
        points: Option<&mut [[f32; 2]]>,
    ) -> Result<(), NullMatrixPoints> {
        let points = points.ok_or(NullMatrixPoints)?;
        let [a, b, c, d, x, y] = self.elements;
        for p in points.iter_mut().rev() {
            let px = f64::from(p[0]);
            let py = f64::from(p[1]);
            p[0] = (add(add(mul(px, a), mul(py, c)), x)) as f32;
            p[1] = (add(add(mul(px, b), mul(py, d)), y)) as f32;
        }
        Ok(())
    }
    pub fn transform_points_f64(
        &self,
        points: Option<&mut [[f64; 2]]>,
    ) -> Result<(), NullMatrixPoints> {
        let points = points.ok_or(NullMatrixPoints)?;
        let [a, b, c, d, x, y] = self.elements;
        for p in points.iter_mut().rev() {
            let px = p[0];
            let py = p[1];
            p[0] = add(add(mul(px, a), mul(py, c)), x);
            p[1] = add(add(mul(px, b), mul(py, d)), y);
        }
        Ok(())
    }
    /// The executed Mono Matrix.RotateAt append path, with f32 rounding at
    /// every arithmetic operation. Native handles/statuses are not modeled.
    pub fn float_rotate_at_offsets(&self, angle: f64) -> Self {
        let [a, b, c, d, x, y] = self.elements.map(|v| v as f32);
        let radians = mul32(angle as f32, f32::from_bits(0x3c8e_fa35));
        let cos = libm::cos(f64::from(radians)) as f32;
        let sin = libm::sin(f64::from(radians)) as f32;
        let tx = add32(add32(mul32(-x, cos), mul32(y, sin)), x);
        let ty = add32(sub32(mul32(-x, sin), mul32(y, cos)), y);
        let elements = [
            add32(mul32(a, cos), mul32(b, -sin)),
            add32(mul32(a, sin), mul32(b, cos)),
            add32(mul32(c, cos), mul32(d, -sin)),
            add32(mul32(c, sin), mul32(d, cos)),
            add32(add32(mul32(x, cos), mul32(y, -sin)), tx),
            add32(add32(mul32(x, sin), mul32(y, cos)), ty),
        ];
        Self {
            elements: elements.map(f64::from),
        }
    }
    /// Characterized MatrixD -> GDI float elements -> MatrixD conversion.
    pub fn float_roundtrip(&self) -> Self {
        Self {
            elements: self.elements.map(|v| f64::from(v as f32)),
        }
    }
}

use crate::source_float::{first_nan as nan_pair, ordered_add as add};
fn sub(a: f64, b: f64) -> f64 {
    match nan_pair(a, b) {
        Some(value) => value,
        None => a - b,
    }
}
fn mul(a: f64, b: f64) -> f64 {
    match nan_pair(a, b) {
        Some(value) => value,
        None => a * b,
    }
}

fn nan_pair32(a: f32, b: f32) -> Option<f32> {
    let v = if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else {
        return None;
    };
    Some(f32::from_bits(v.to_bits() | 0x0040_0000))
}
fn add32(a: f32, b: f32) -> f32 {
    match nan_pair32(a, b) {
        Some(value) => value,
        None => a + b,
    }
}
fn sub32(a: f32, b: f32) -> f32 {
    match nan_pair32(a, b) {
        Some(value) => value,
        None => a - b,
    }
}
fn mul32(a: f32, b: f32) -> f32 {
    match nan_pair32(a, b) {
        Some(value) => value,
        None => a * b,
    }
}

/// Closed matrix operations keep host callbacks outside private shared locks.
#[derive(Clone, Copy, Debug)]
pub enum MatrixOperation {
    Reset,
    Invert,
    Translate { x: f64, y: f64, prepend: bool },
    Scale { x: f64, y: f64, prepend: bool },
    Rotate { angle: f64, prepend: bool },
    RotateAbout { angle: f64, pivot: [f64; 2] },
    SetElements([f64; 6]),
}
impl MatrixWireRecord {
    pub fn apply(&mut self, operation: MatrixOperation) {
        match operation {
            MatrixOperation::Reset => self.reset(),
            MatrixOperation::Invert => self.invert(),
            MatrixOperation::Translate { x, y, prepend } => self.translate(x, y, prepend),
            MatrixOperation::Scale { x, y, prepend } => self.scale(x, y, prepend),
            MatrixOperation::Rotate { angle, prepend } => self.rotate_degrees(angle, prepend),
            MatrixOperation::RotateAbout { angle, pivot } => self.rotate_about(angle, pivot),
            MatrixOperation::SetElements(elements) => self.elements = elements,
        }
    }
}
