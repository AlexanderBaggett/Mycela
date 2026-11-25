//! Actual wu mutable convolution data, distinct from the affine ad9 matrix.
//! TexTree 06002b65/6a/6b/6c/6d: public fields, real array roots and partial I/O.
use crate::{BinaryCursor, BinaryError, BinaryWriter};
use std::{
    any::Any,
    fmt,
    sync::{Arc, Mutex, MutexGuard, Weak},
};
fn guard<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatrixKernelError {
    NullReference,
    Index,
    Allocation,
    Binary(BinaryError),
}
impl MatrixKernelError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::NullReference => "NullReferenceException",
            Self::Index => "IndexOutOfRangeException",
            Self::Allocation => "unproved-allocation-error",
            Self::Binary(e) => e.source_class(),
        }
    }
}
impl From<BinaryError> for MatrixKernelError {
    fn from(value: BinaryError) -> Self {
        Self::Binary(value)
    }
}
impl fmt::Display for MatrixKernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Matrix kernel {}", self.source_class())
    }
}
impl std::error::Error for MatrixKernelError {}
/// The actual replaceable Double[] root. Handle Clone retains storage identity;
/// source wu copy clones it instead, including a nonstandard caller length.
#[derive(Clone)]
pub struct MatrixCoefficients(Arc<Mutex<Vec<f64>>>);
impl fmt::Debug for MatrixCoefficients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MatrixCoefficients({:p})", Arc::as_ptr(&self.0))
    }
}
impl MatrixCoefficients {
    pub fn new(values: Vec<f64>) -> Self {
        Self(Arc::new(Mutex::new(values)))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn len(&self) -> usize {
        guard(&self.0).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn values(&self) -> Vec<f64> {
        guard(&self.0).clone()
    }
    pub fn read(&self, index: usize) -> Result<f64, MatrixKernelError> {
        guard(&self.0)
            .get(index)
            .copied()
            .ok_or(MatrixKernelError::Index)
    }
    pub fn write(&self, index: usize, value: f64) -> Result<(), MatrixKernelError> {
        *guard(&self.0)
            .get_mut(index)
            .ok_or(MatrixKernelError::Index)? = value;
        Ok(())
    }
    fn copy_values(&self) -> Result<Vec<f64>, MatrixKernelError> {
        let values = guard(&self.0);
        let mut copy = Vec::new();
        copy.try_reserve_exact(values.len())
            .map_err(|_| MatrixKernelError::Allocation)?;
        copy.extend_from_slice(&values);
        Ok(copy)
    }
}
type RetentionFrame = Arc<dyn Any + Send + Sync>;
type RetentionRoute = Arc<dyn Fn() -> Option<RetentionFrame> + Send + Sync>;
struct ExternalAnchor {
    owners: Mutex<Vec<(u64, RetentionFrame)>>,
}
struct State {
    anchor: Weak<ExternalAnchor>,
    routes: Vec<(u64, RetentionRoute)>,
    angle: f64,
    radius: f64,
    coefficients: Option<MatrixCoefficients>,
}
/// Retained actual wu authority. Public field edits are silent; this type does
/// not pretend to implement the surrounding aoi/os event/undo property manager.
#[derive(Clone)]
pub struct MatrixKernel(Arc<Mutex<State>>, Option<Arc<ExternalAnchor>>);
impl fmt::Debug for MatrixKernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MatrixKernel({:p})", Arc::as_ptr(&self.0))
    }
}
impl MatrixKernel {
    pub fn new() -> Result<Self, MatrixKernelError> {
        let mut values = Vec::new();
        values
            .try_reserve_exact(9)
            .map_err(|_| MatrixKernelError::Allocation)?;
        values.resize(9, 0.0);
        values[0] = 1.0;
        let anchor = Arc::new(ExternalAnchor {
            owners: Mutex::new(Vec::new()),
        });
        Ok(Self(
            Arc::new(Mutex::new(State {
                anchor: Arc::downgrade(&anchor),
                routes: Vec::new(),
                angle: 0.0,
                radius: 0.003,
                coefficients: Some(MatrixCoefficients::new(values)),
            })),
            Some(anchor),
        ))
    }
    pub(crate) fn captured_fields(
        angle: f64,
        radius: f64,
        coefficients: Option<MatrixCoefficients>,
    ) -> Self {
        let anchor = Arc::new(ExternalAnchor {
            owners: Mutex::new(Vec::new()),
        });
        Self(
            Arc::new(Mutex::new(State {
                angle,
                radius,
                coefficients,
                anchor: Arc::downgrade(&anchor),
                routes: Vec::new(),
            })),
            Some(anchor),
        )
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Internal owning records keep the body but never its outward owner anchor.
    pub(crate) fn internal_handle(&self) -> Self {
        Self(Arc::clone(&self.0), None)
    }
    pub(crate) fn external_handle(&self) -> Self {
        let _held = &self.1;
        let (anchor, routes) = {
            let mut s = guard(&self.0);
            let a = match s.anchor.upgrade() {
                Some(a) => a,
                None => {
                    let a = Arc::new(ExternalAnchor {
                        owners: Mutex::new(Vec::new()),
                    });
                    s.anchor = Arc::downgrade(&a);
                    a
                }
            };
            (a, s.routes.clone())
        };
        let owners = routes
            .into_iter()
            .filter_map(|(id, r)| r().map(|v| (id, v)))
            .collect();
        let old = std::mem::replace(&mut *guard(&anchor.owners), owners);
        drop(old);
        Self(Arc::clone(&self.0), Some(anchor))
    }
    pub(crate) fn add_owner_route(&self, id: u64, route: RetentionRoute) {
        let anchor = {
            let mut s = guard(&self.0);
            if !s.routes.iter().any(|(i, _)| *i == id) {
                s.routes.push((id, route.clone()));
            }
            s.anchor.upgrade()
        };
        if let Some(anchor) = anchor
            && let Some(frame) = route()
        {
            let old = {
                let mut owners = guard(&anchor.owners);
                if let Some(i) = owners.iter().position(|(i, _)| *i == id) {
                    Some(std::mem::replace(&mut owners[i].1, frame))
                } else {
                    owners.push((id, frame));
                    None
                }
            };
            drop(old);
        }
    }
    pub(crate) fn remove_owner_route(&self, id: u64) {
        let (removed, anchor) = {
            let mut s = guard(&self.0);
            let removed = s
                .routes
                .iter()
                .position(|(i, _)| *i == id)
                .map(|i| s.routes.remove(i));
            (removed, s.anchor.upgrade())
        };
        drop(removed);
        if let Some(anchor) = anchor {
            let old = {
                let mut owners = guard(&anchor.owners);
                owners
                    .iter()
                    .position(|(i, _)| *i == id)
                    .map(|i| owners.remove(i))
            };
            drop(old);
        }
    }
    pub fn angle(&self) -> f64 {
        guard(&self.0).angle
    }
    pub fn radius(&self) -> f64 {
        guard(&self.0).radius
    }
    pub fn coefficients(&self) -> Option<MatrixCoefficients> {
        guard(&self.0).coefficients.clone()
    }
    pub fn set_angle(&self, value: f64) {
        guard(&self.0).angle = value;
    }
    pub fn set_radius(&self, value: f64) {
        guard(&self.0).radius = value;
    }
    pub fn set_coefficients(&self, value: Option<MatrixCoefficients>) {
        let old = std::mem::replace(&mut guard(&self.0).coefficients, value);
        drop(old);
    }
    /// wu.cj: angle and radius commit before the fallible array clone. Null
    /// source fails at its first field read; wrong-kind CLR casts are not this
    /// typed API. Self-copy still replaces the actual array with a fresh root.
    pub fn copy_from(&self, source: Option<&Self>) -> Result<(), MatrixKernelError> {
        let source = source.ok_or(MatrixKernelError::NullReference)?;
        self.set_angle(source.angle());
        self.set_radius(source.radius());
        let copy = source
            .coefficients()
            .ok_or(MatrixKernelError::NullReference)?
            .copy_values()?;
        self.set_coefficients(Some(MatrixCoefficients::new(copy)));
        Ok(())
    }
    pub fn source_clone(&self) -> Result<Self, MatrixKernelError> {
        let copy = Self::new()?;
        copy.copy_from(Some(self))?;
        Ok(copy)
    }
    /// Save iterates the current public array length, not a fixed nine values.
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), MatrixKernelError> {
        writer.write_f64(self.angle())?;
        writer.write_f64(self.radius())?;
        let mut index = 0;
        while index
            < self
                .coefficients()
                .ok_or(MatrixKernelError::NullReference)?
                .len()
        {
            writer.write_f64(
                self.coefficients()
                    .ok_or(MatrixKernelError::NullReference)?
                    .read(index)?,
            )?;
            index += 1;
        }
        Ok(())
    }
    /// Load changes the existing array in place; later read errors retain every
    /// prior field/cell. Encoding is unused by the original payload method.
    pub fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        _encoding: f64,
    ) -> Result<(), MatrixKernelError> {
        self.set_angle(reader.read_f64()?);
        self.set_radius(reader.read_f64()?);
        let mut index = 0;
        while index
            < self
                .coefficients()
                .ok_or(MatrixKernelError::NullReference)?
                .len()
        {
            let array = self.coefficients();
            let value = reader.read_f64()?;
            array
                .ok_or(MatrixKernelError::NullReference)?
                .write(index, value)?;
            index += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "matrix_kernel_test.rs"]
mod tests;
