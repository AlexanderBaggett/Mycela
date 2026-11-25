//! Retained caller writer and stream views over one actual BinaryWriter authority.
//! Leases move the writer out before child calls and restore partial bytes on unwind.
use crate::{
    AnimationError, BinaryError, BinaryWriter, CurveError, GradientAnimation, RangeRecordError,
    SharedCurve, SharedCurveRangeRecord, SharedLimits,
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
struct Core {
    writer: Mutex<Option<BinaryWriter>>,
}
#[derive(Debug)]
pub enum MemoryWriterError {
    Busy,
    Binary(BinaryError),
}
pub struct RetainedBinaryWriter(Arc<Core>);
pub struct RetainedMemoryStream(Arc<Core>);
impl Clone for RetainedBinaryWriter {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl Clone for RetainedMemoryStream {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl fmt::Debug for RetainedBinaryWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RetainedBinaryWriter({:p})", Arc::as_ptr(&self.0))
    }
}
impl fmt::Debug for RetainedMemoryStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RetainedMemoryStream({:p})", Arc::as_ptr(&self.0))
    }
}
struct Lease {
    owner: Arc<Core>,
    writer: Option<BinaryWriter>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let retired = {
            let mut state = self.owner.writer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *state, self.writer.take())
        };
        drop(retired);
    }
}
impl RetainedBinaryWriter {
    pub fn new() -> Self {
        Self(Arc::new(Core {
            writer: Mutex::new(Some(BinaryWriter::default())),
        }))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn stream(&self) -> RetainedMemoryStream {
        RetainedMemoryStream(Arc::clone(&self.0))
    }
    fn lease(&self) -> Result<Lease, MemoryWriterError> {
        let writer = self
            .0
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or(MemoryWriterError::Busy)?;
        Ok(Lease {
            owner: Arc::clone(&self.0),
            writer: Some(writer),
        })
    }
    pub fn close(&self) -> Result<(), MemoryWriterError> {
        let mut l = self.lease()?;
        l.writer.as_mut().unwrap().close();
        Ok(())
    }
    pub fn bytes(&self) -> Result<Vec<u8>, MemoryWriterError> {
        self.stream().bytes()
    }
    pub fn position(&self) -> Result<usize, MemoryWriterError> {
        self.stream().position()
    }
    pub fn save_animation<E: 'static>(
        &self,
        value: &GradientAnimation<E>,
    ) -> Result<Result<(), AnimationError<E>>, MemoryWriterError> {
        let mut l = self.lease()?;
        Ok(value.save(l.writer.as_mut().unwrap()))
    }
    pub fn save_curve<E>(
        &self,
        value: &SharedCurve<E>,
    ) -> Result<Result<(), CurveError<E>>, MemoryWriterError> {
        let mut l = self.lease()?;
        Ok(value.save(l.writer.as_mut().unwrap()))
    }
    pub fn save_limits(
        &self,
        value: &SharedLimits,
    ) -> Result<Result<(), RangeRecordError>, MemoryWriterError> {
        let mut l = self.lease()?;
        Ok(value.save(l.writer.as_mut().unwrap()))
    }
    pub fn save_curve_range(
        &self,
        value: &SharedCurveRangeRecord,
    ) -> Result<Result<(), RangeRecordError>, MemoryWriterError> {
        let mut l = self.lease()?;
        Ok(value.save(l.writer.as_mut().unwrap()))
    }
    pub fn save_range(
        &self,
        value: &crate::SharedRangeOptionsRecord,
    ) -> Result<Result<(), RangeRecordError>, MemoryWriterError> {
        let mut l = self.lease()?;
        Ok(value.save(l.writer.as_mut().unwrap()))
    }
}
impl Default for RetainedBinaryWriter {
    fn default() -> Self {
        Self::new()
    }
}
impl RetainedMemoryStream {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn bytes(&self) -> Result<Vec<u8>, MemoryWriterError> {
        let state = self.0.writer.lock().unwrap_or_else(|e| e.into_inner());
        Ok(state
            .as_ref()
            .ok_or(MemoryWriterError::Busy)?
            .bytes()
            .to_vec())
    }
    pub fn position(&self) -> Result<usize, MemoryWriterError> {
        let state = self.0.writer.lock().unwrap_or_else(|e| e.into_inner());
        state
            .as_ref()
            .ok_or(MemoryWriterError::Busy)?
            .position()
            .map_err(MemoryWriterError::Binary)
    }
    pub fn length(&self) -> Result<usize, MemoryWriterError> {
        self.position()?;
        Ok(self.bytes()?.len())
    }
    pub fn is_readable(&self) -> Result<bool, MemoryWriterError> {
        self.is_writable()
    }
    pub fn is_writable(&self) -> Result<bool, MemoryWriterError> {
        let state = self.0.writer.lock().unwrap_or_else(|e| e.into_inner());
        Ok(state.as_ref().ok_or(MemoryWriterError::Busy)?.is_writable())
    }
}
