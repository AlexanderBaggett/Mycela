//! Live os metadata and wire operations. Typed values remain in their real owners.
use super::*;
use crate::{
    BinaryCursor, BinaryError, BinaryWriter, GradientAnimation, ImportedImageError,
    MatrixKernelError, PropertyRecordError,
};
#[path = "property_wire/adjustment.rs"]
mod adjustment;
pub(crate) use adjustment::AdjustmentCaptureParts;
pub use adjustment::SharedPropertyAdjustment;
#[path = "property_wire/number.rs"]
mod number;
pub(crate) use number::{converted as framework_decimal_from_f64, validate_wire_number};
#[path = "property_wire/filter.rs"]
mod filter;
pub(crate) use filter::{load_custom, save_custom};
#[cfg(test)]
#[path = "property_wire/laws.rs"]
mod laws;

#[derive(Debug)]
pub enum LivePropertyWireError<E = ()> {
    Binary(BinaryError),
    Property(PointwiseError<E>),
    Matrix(MatrixKernelError),
    Imported(ImportedImageError),
    Environment(crate::EnvironmentWireError),
    Payload(PropertyRecordError),
    CodeMismatch,
    UnknownChoice {
        node: &'static str,
        property: String,
        choice: String,
    },
    LimitViolation,
    DecimalOverflow,
    UnsupportedCapability(&'static str),
}
impl<E> LivePropertyWireError<E> {
    pub fn source_class(&self) -> &'static str {
        match self {
            Self::Binary(e) => e.source_class(),
            Self::Property(e) => e.source_class(),
            Self::Matrix(e) => e.source_class(),
            Self::Imported(e) => e.source_class(),
            Self::Environment(e) => match e {
                crate::EnvironmentWireError::Binary(e) => e.source_class(),
                _ => "Exception",
            },
            Self::Payload(e) => e.source_class(),
            Self::CodeMismatch | Self::UnknownChoice { .. } | Self::LimitViolation => "Exception",
            Self::DecimalOverflow => "OverflowException",
            Self::UnsupportedCapability(_) => "UnsupportedCapability",
        }
    }
}
impl<E: fmt::Debug> fmt::Display for LivePropertyWireError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownChoice {
                node,
                property,
                choice,
            } => {
                write!(f, "{node} has no “{choice}” option for “{property}”.")
            }
            _ => write!(f, "{self:?}"),
        }
    }
}
impl<E: fmt::Debug + 'static> std::error::Error for LivePropertyWireError<E> {}
impl<E> From<BinaryError> for LivePropertyWireError<E> {
    fn from(e: BinaryError) -> Self {
        Self::Binary(e)
    }
}
impl<E> From<PointwiseError<E>> for LivePropertyWireError<E> {
    fn from(e: PointwiseError<E>) -> Self {
        Self::Property(e)
    }
}
impl<E> From<crate::AnimationError<E>> for LivePropertyWireError<E> {
    fn from(e: crate::AnimationError<E>) -> Self {
        Self::Property(e.into())
    }
}
impl<E> From<crate::CurveError<E>> for LivePropertyWireError<E> {
    fn from(e: crate::CurveError<E>) -> Self {
        Self::Property(e.into())
    }
}
impl<E> From<crate::GradientError> for LivePropertyWireError<E> {
    fn from(e: crate::GradientError) -> Self {
        Self::Property(e.into())
    }
}

pub(crate) struct AdjustmentSpec {
    pub(crate) property_type: i32,
    pub(crate) count: i32,
    pub(crate) limits: Option<SharedLimits>,
}
struct MetadataState<E> {
    binding: Option<(i32, Option<Vec<u16>>)>,
    animatable: bool,
    adjustable: bool,
    adjusted: Option<SharedPropertyAdjustment<E>>,
    // The Color catalog supplies this swatch. The Color property owns and saves its local RGB.
    resolved_lab_color: Option<[f64; 3]>,
}
pub(crate) struct LegacyWireMetadata<E>(Arc<Mutex<MetadataState<E>>>);
impl<E> Clone for LegacyWireMetadata<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> fmt::Debug for LegacyWireMetadata<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LegacyWireMetadata({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E> LegacyWireMetadata<E> {
    pub(crate) fn new(animatable: bool, adjustable: bool) -> Self {
        Self(Arc::new(Mutex::new(MetadataState {
            binding: Some((-1, None)),
            animatable,
            adjustable,
            adjusted: None,
            resolved_lab_color: None,
        })))
    }
    pub(crate) fn without_binding(animatable: bool, adjustable: bool) -> Self {
        Self(Arc::new(Mutex::new(MetadataState {
            binding: None,
            animatable,
            adjustable,
            adjusted: None,
            resolved_lab_color: None,
        })))
    }
    pub(crate) fn binding_index(&self) -> i32 {
        guard(&self.0).binding.as_ref().map_or(-1, |b| b.0)
    }
    pub(crate) fn binding_name(&self) -> Option<Vec<u16>> {
        guard(&self.0).binding.as_ref().and_then(|b| b.1.clone())
    }
    pub(crate) fn set_binding_index(&self, value: i32) {
        if let Some(b) = &mut guard(&self.0).binding {
            b.0 = value;
        }
    }
    pub(crate) fn set_binding_name(&self, value: Option<&[u16]>) {
        let mut s = guard(&self.0);
        if let Some(b) = &mut s.binding
            && b.1.as_deref().unwrap_or_default() != value.unwrap_or_default()
        {
            b.1 = value.map(<[u16]>::to_vec);
        }
    }
    pub(crate) fn animatable(&self) -> bool {
        guard(&self.0).animatable
    }
    pub(crate) fn adjustable(&self) -> bool {
        guard(&self.0).adjustable
    }
    pub(crate) fn retained_adjustment(&self) -> Option<SharedPropertyAdjustment<E>> {
        guard(&self.0).adjusted.clone()
    }
    pub(crate) fn resolved_lab_color(&self) -> Option<[f64; 3]> {
        guard(&self.0).resolved_lab_color
    }
    pub(crate) fn replace_resolved_lab_color(
        &self,
        expected: Option<[f64; 3]>,
        value: Option<[f64; 3]>,
    ) -> bool {
        let mut state = guard(&self.0);
        if state.resolved_lab_color.map(|rgb| rgb.map(f64::to_bits))
            != expected.map(|rgb| rgb.map(f64::to_bits))
        {
            return false;
        }
        state.resolved_lab_color = value;
        true
    }
    pub(crate) fn new_reference_adjustment(
        &self,
        type_tag: i32,
        name: &[u16],
        group_id: [u8; 16],
    ) -> Result<SharedPropertyAdjustment<E>, LivePropertyWireError<E>>
    where
        E: 'static,
    {
        let adjustment = match self
            .retained_adjustment()
            .filter(|value| value.property_type() == type_tag)
        {
            Some(existing) => existing.source_clone()?,
            None => SharedPropertyAdjustment::new(type_tag, 1, None)?,
        };
        adjustment.set_guid(group_id);
        adjustment
            .names()
            .set_entry(0, Some(name.to_vec()))
            .map_err(|_| PointwiseError::Index)?;
        adjustment
            .flags()
            .set(0, true)
            .map_err(|_| PointwiseError::Index)?;
        Ok(adjustment)
    }
    pub(crate) fn compare_and_replace_adjustment(
        &self,
        expected: Option<&SharedPropertyAdjustment<E>>,
        replacement: Option<SharedPropertyAdjustment<E>>,
    ) -> bool {
        let mut state = guard(&self.0);
        let same = match (&state.adjusted, expected) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual.same_reference(expected),
            _ => false,
        };
        if !state.adjustable || !same {
            return false;
        }
        state.adjusted = replacement;
        true
    }
    pub(crate) fn set_capabilities(&self, animatable: bool, adjustable: bool) {
        let mut s = guard(&self.0);
        s.animatable = animatable;
        s.adjustable = adjustable;
    }
    pub(crate) fn capture_values(&self) -> (i32, Option<Vec<u16>>, bool, bool) {
        let s = guard(&self.0);
        (
            s.binding.as_ref().map_or(-1, |b| b.0),
            s.binding.as_ref().and_then(|b| b.1.clone()),
            s.animatable,
            s.adjustable,
        )
    }
    pub(crate) fn capture_with(
        &self,
        mut copy: impl FnMut(
            &SharedPropertyAdjustment<E>,
        ) -> Result<SharedPropertyAdjustment<E>, PointwiseError<E>>,
    ) -> Result<Self, PointwiseError<E>> {
        let (binding, animatable, adjustable, adjusted, resolved_lab_color) = {
            let s = guard(&self.0);
            (
                s.binding.clone(),
                s.animatable,
                s.adjustable,
                s.adjusted.clone(),
                s.resolved_lab_color,
            )
        };
        let adjusted = adjusted.as_ref().map(&mut copy).transpose()?;
        Ok(Self(Arc::new(Mutex::new(MetadataState {
            binding,
            animatable,
            adjustable,
            adjusted,
            resolved_lab_color,
        }))))
    }
}
impl<E: 'static> LegacyWireMetadata<E> {
    pub(crate) fn source_clone(&self) -> Result<Self, PointwiseError<E>> {
        self.capture_with(SharedPropertyAdjustment::source_clone)
    }
    pub(crate) fn adjustment(
        &self,
        property_type: i32,
        count: i32,
        limits: Option<&SharedLimits>,
    ) -> Result<Option<SharedPropertyAdjustment<E>>, LivePropertyWireError<E>> {
        if !self.adjustable() {
            return Ok(self.retained_adjustment());
        }
        if let Some(a) = self.retained_adjustment() {
            return Ok(Some(a));
        }
        let a = SharedPropertyAdjustment::new(property_type, count, limits)?;
        guard(&self.0).adjusted = Some(a.clone());
        Ok(Some(a))
    }
    pub(crate) fn is_adjusted(&self) -> Result<bool, PointwiseError<E>> {
        self.retained_adjustment()
            .as_ref()
            .map_or(Ok(false), SharedPropertyAdjustment::is_active)
    }
    pub(crate) fn ensure_evaluation_supported(&self) -> Result<(), PointwiseError<E>> {
        if self.binding_index() >= 0 {
            return Err(PointwiseError::UnsupportedCapability(
                "positive live property binding context",
            ));
        }
        // Unbound os effective getters do not apply m_as as a standalone
        // transform. Preserve the child for IO; attached context evaluation is
        // reached through positive binding, which remains explicitly unsupported.
        Ok(())
    }
    pub(crate) fn save_tail(
        &self,
        writer: &mut BinaryWriter,
        animation: impl FnMut() -> Result<Option<GradientAnimation<E>>, LivePropertyWireError<E>>,
    ) -> Result<(), LivePropertyWireError<E>> {
        if self.binding_index() == -1 {
            writer.write_bool(false)?;
        } else {
            writer.write_bool(true)?;
            writer.write_i16(self.binding_index() as i16)?;
            writer.write_utf16(self.binding_name().as_deref().unwrap_or_default())?;
        }
        self.save_animation_adjustment_tail(writer, animation)
    }
    pub(crate) fn save_animation_adjustment_tail(
        &self,
        writer: &mut BinaryWriter,
        mut animation: impl FnMut() -> Result<Option<GradientAnimation<E>>, LivePropertyWireError<E>>,
    ) -> Result<(), LivePropertyWireError<E>> {
        // Deliberately after binding IO: a failed writer must not force bh.
        if self.animatable() {
            animation()?
                .ok_or(PointwiseError::NullReference)?
                .save(writer)?;
        }
        writer.write_bool(self.adjustable() && self.is_adjusted()?)?;
        if self.adjustable() && self.is_adjusted()? {
            self.retained_adjustment()
                .ok_or(PointwiseError::NullReference)?
                .save(writer)?;
        }
        Ok(())
    }
    pub(crate) fn load_tail(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        animation: impl FnMut() -> Result<Option<GradientAnimation<E>>, LivePropertyWireError<E>>,
        spec: impl FnMut() -> Result<AdjustmentSpec, LivePropertyWireError<E>>,
    ) -> Result<(), LivePropertyWireError<E>> {
        if reader.read_bool()? {
            self.set_binding_index(i32::from(reader.read_i16()?));
            self.set_binding_name(Some(&reader.read_string()?));
        }
        self.load_animation_adjustment_tail(reader, version, animation, spec)
    }
    pub(crate) fn load_animation_adjustment_tail(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        mut animation: impl FnMut() -> Result<Option<GradientAnimation<E>>, LivePropertyWireError<E>>,
        mut spec: impl FnMut() -> Result<AdjustmentSpec, LivePropertyWireError<E>>,
    ) -> Result<(), LivePropertyWireError<E>> {
        if self.animatable() {
            animation()?
                .ok_or(PointwiseError::NullReference)?
                .load(reader, version)?;
        }
        if version >= 3.72 && reader.read_bool()? {
            let s = spec()?;
            self.adjustment(s.property_type, s.count, s.limits.as_ref())?
                .ok_or(PointwiseError::NullReference)?
                .load(reader, version)?;
        }
        if (3.54..=3.71).contains(&version) && reader.read_bool()? {
            reader.read_f64()?;
            reader.read_f64()?;
            reader.read_i32()?;
            reader.read_string()?;
            reader.read_string()?;
            if version >= 3.55 {
                reader.read_f64()?;
            }
        }
        Ok(())
    }
}
pub(crate) fn save_code<E>(
    writer: &mut BinaryWriter,
    name: &[u16],
) -> Result<(), LivePropertyWireError<E>> {
    writer.write_utf16(&name[..name.len().min(3)])?;
    Ok(())
}
pub(crate) fn check_code<E>(
    reader: &mut BinaryCursor<'_>,
    name: &[u16],
    legacy_keep: bool,
) -> Result<(), LivePropertyWireError<E>> {
    let code = reader.read_string()?;
    if !crate::property_prefix_en_us(name, &code)
        && !(legacy_keep && crate::property_prefix_en_us(&code, &text("Kee")))
    {
        return Err(LivePropertyWireError::CodeMismatch);
    }
    Ok(())
}
pub(crate) fn choice_limits<E>(count: usize) -> Result<SharedLimits, PointwiseError<E>> {
    let count = u32::try_from(count).map_err(|_| ImageError::Overflow)?;
    let maximum = count
        .checked_mul(10)
        .and_then(|n| n.checked_add(5))
        .ok_or(ImageError::Overflow)?;
    SharedLimits::from_definition(LimitsDefinition {
        decimal_places: 1,
        step: LimitsStep::Derived(
            DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
        ),
        minimum: DecimalBits::new([5, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
        maximum: DecimalBits::new([maximum, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
        used: true,
        use_range: None,
        curve_range: None,
    })
    .map_err(|_| ImageError::Overflow.into())
}
