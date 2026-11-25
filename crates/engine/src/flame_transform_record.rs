use crate::{
    BinaryCursor, BinaryError, BinaryWriter, DecimalBits, LimitsRecord, MatrixWireRecord,
    ModernAdjustmentConfiguration, ModernPropertyConfiguration, ModernPropertyRecord,
    ModernPropertyValue, ParameterPropertyId, ParameterRecordError, ParameterRecordStore,
    ParameterSetId, ParameterStoreError, RangeOptionsRecord, SharedReference,
};
use std::{error::Error, fmt};

/// The source collection's wire boundary. Implementations must preserve its
/// clear/count/name/resolve/child-load/add order. No default or opaque payload
/// skipper is provided; successful variation dispatch remains separate work.
pub trait FlameVariationWire {
    type Error;
    fn load(&mut self, reader: &mut BinaryCursor<'_>, version: f64) -> Result<(), Self::Error>;
    fn save(&self, writer: &mut BinaryWriter) -> Result<(), Self::Error>;
}

pub type SharedMatrixRecord = SharedReference<MatrixWireRecord>;

/// Fixed ad9 definition and wire state with an explicit variation backend.
/// Matrix order: m_e, m_c, m_d, m_b; scalar order: m_l, m_k, m_g, n, m_f, m_m.
/// Public source setters, arithmetic, events and clone are not implemented here.
#[derive(Debug)]
pub struct FlameTransformRecord<V> {
    set: ParameterSetId,
    properties: [ParameterPropertyId; 2],
    limits: [LimitsRecord; 2],
    pub scalars: [f64; 6],
    pub text: Option<Vec<u16>>,
    pub matrices: [Option<SharedMatrixRecord>; 4],
    pub mode: i32,
    pub variations: V,
}

const fn decimal(magnitude: u32, scale: u32) -> DecimalBits {
    match DecimalBits::new([magnitude, 0, 0, scale << 16]) {
        Ok(value) => value,
        Err(_) => panic!("invalid fixed decimal"),
    }
}
fn declared_limits(index: usize) -> LimitsRecord {
    let minimum = decimal(0, 0);
    let maximum = decimal(if index == 0 { 100 } else { 1 }, 0);
    LimitsRecord {
        decimal_places: 3,
        increment: decimal(1, 1),
        big_increment: decimal(10, 1),
        minimum,
        maximum,
        used: true,
        use_range: Some(RangeOptionsRecord::new(minimum, maximum)),
        curve_range: None,
    }
}
impl<V> FlameTransformRecord<V> {
    /// The caller supplies the variation context; this constructor does not
    /// claim that a populated backend is the source's empty default collection.
    pub fn new_in(
        store: &mut ParameterRecordStore,
        variations: V,
    ) -> Result<Self, ParameterStoreError<std::convert::Infallible>> {
        let names = ["Probability", "ColorIndex"];
        let defaults = [1.0, 0.0];
        let records = std::array::from_fn(|i| {
            let mut record = ModernPropertyRecord::new(
                ModernPropertyValue::Double {
                    current: defaults[i],
                    previous: defaults[i],
                },
                ModernPropertyConfiguration {
                    can_create_animation: true,
                    adjustment: Some(ModernAdjustmentConfiguration {
                        property_type: 0,
                        bounds: vec![[0.0, if i == 0 { 100.0 } else { 1.0 }]],
                    }),
                },
            )
            .expect("fixed one-component definition");
            record.name = Some(names[i].encode_utf16().collect());
            record.adjustable = true;
            record.animatable = true;
            record
        });
        Self::from_backing_records(store, records, variations)
    }
    /// Explicit fixed-schema backing input, not the source copy constructor.
    pub fn from_backing_records(
        store: &mut ParameterRecordStore,
        records: [ModernPropertyRecord; 2],
        variations: V,
    ) -> Result<Self, ParameterStoreError<std::convert::Infallible>> {
        for (i, record) in records.iter().enumerate() {
            let names = ["Probability", "ColorIndex"];
            let config = record.configuration();
            if !matches!(record.value(), ModernPropertyValue::Double { .. })
                || record.name.as_deref()
                    != Some(names[i].encode_utf16().collect::<Vec<_>>().as_slice())
                || !record.adjustable
                || !record.animatable
                || !config.can_create_animation
                || !config.adjustment.as_ref().is_some_and(|a| {
                    a.property_type == 0
                        && a.bounds == vec![[0.0, if i == 0 { 100.0 } else { 1.0 }]]
                })
            {
                return Err(ParameterStoreError::Record(
                    ParameterRecordError::DefinitionLayout,
                ));
            }
        }
        let set = store.new_set();
        let properties = records.map(|record| store.new_property(record));
        for property in &properties {
            store.add_property(&set, Some(property))?;
        }
        Ok(Self {
            set,
            properties,
            limits: std::array::from_fn(declared_limits),
            scalars: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            text: Some(Vec::new()),
            matrices: std::array::from_fn(|_| {
                Some(SharedReference::new(MatrixWireRecord::default()))
            }),
            mode: 0,
            variations,
        })
    }
    pub fn set_id(&self) -> &ParameterSetId {
        &self.set
    }
    pub fn property_ids(&self) -> &[ParameterPropertyId; 2] {
        &self.properties
    }
    pub fn declared_limits(&self) -> &[LimitsRecord; 2] {
        &self.limits
    }
}
impl<V: FlameVariationWire> FlameTransformRecord<V> {
    pub fn load(
        &mut self,
        store: &mut ParameterRecordStore,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), FlameTransformError<V::Error>> {
        store
            .load(&self.set, reader, version)
            .map_err(FlameTransformError::Property)?;
        self.scalars[0] = reader.read_f64()?;
        self.scalars[1] = reader.read_f64()?;
        self.text = Some(reader.read_string()?);
        self.scalars[2] = reader.read_f64()?;
        for index in 0..4 {
            self.matrices[index] = Some(SharedReference::new(MatrixWireRecord::read(
                reader, version,
            )?));
            if index < 3 {
                self.scalars[index + 3] = reader.read_f64()?;
            }
        }
        self.mode = reader.read_i32()?;
        self.variations
            .load(reader, version)
            .map_err(FlameTransformError::Variation)
    }
    pub fn save(
        &self,
        store: &mut ParameterRecordStore,
        writer: &mut BinaryWriter,
    ) -> Result<(), FlameTransformError<V::Error>> {
        store
            .save(&self.set, writer)
            .map_err(FlameTransformError::Property)?;
        writer.write_f64(self.scalars[0])?;
        writer.write_f64(self.scalars[1])?;
        writer.write_optional_utf16(self.text.as_deref())?;
        writer.write_f64(self.scalars[2])?;
        for index in 0..4 {
            self.matrices[index]
                .as_ref()
                .ok_or(FlameTransformError::NullMatrix)?
                .save_matrix(writer)?;
            if index < 3 {
                writer.write_f64(self.scalars[index + 3])?;
            }
        }
        writer.write_i32(self.mode)?;
        self.variations
            .save(writer)
            .map_err(FlameTransformError::Variation)
    }
}
#[derive(Debug)]
pub enum FlameTransformError<E> {
    Property(ParameterStoreError<std::convert::Infallible>),
    Binary(BinaryError),
    NullMatrix,
    Variation(E),
}
impl<E> From<BinaryError> for FlameTransformError<E> {
    fn from(value: BinaryError) -> Self {
        Self::Binary(value)
    }
}
impl<E: fmt::Display> fmt::Display for FlameTransformError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Property(e) => e.fmt(f),
            Self::Binary(e) => e.fmt(f),
            Self::NullMatrix => f.write_str("matrix: NullReferenceException"),
            Self::Variation(e) => e.fmt(f),
        }
    }
}
impl<E: Error + 'static> Error for FlameTransformError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Property(e) => Some(e),
            Self::Binary(e) => Some(e),
            Self::Variation(e) => Some(e),
            Self::NullMatrix => None,
        }
    }
}
