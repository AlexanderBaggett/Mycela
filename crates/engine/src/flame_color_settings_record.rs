use crate::{
    BinaryCursor, BinaryWriter, CurveRangeRecord, DecimalBits, LimitsRecord,
    ModernAdjustmentConfiguration, ModernPropertyConfiguration, ModernPropertyRecord,
    ModernPropertyValue, ParameterPropertyId, ParameterRecordError, ParameterRecordStore,
    ParameterSetId, ParameterStoreError, RangeOptionsRecord,
};

const fn decimal(magnitude: u32, scale: u32, negative: bool) -> DecimalBits {
    match DecimalBits::new([
        magnitude,
        0,
        0,
        (scale << 16) | if negative { 0x8000_0000 } else { 0 },
    ]) {
        Ok(value) => value,
        Err(_) => panic!("invalid built-in decimal"),
    }
}
fn limits(index: usize) -> LimitsRecord {
    let (places, minimum, maximum, curve_range) = match index {
        0 => (
            2,
            decimal(20, 0, true),
            decimal(20, 0, false),
            Some(CurveRangeRecord {
                minimum: decimal(2, 0, true),
                maximum: decimal(2, 0, false),
                wrap_y: false,
            }),
        ),
        1 => (
            3,
            decimal(1, 2, false),
            decimal(100, 0, false),
            Some(CurveRangeRecord {
                minimum: decimal(2, 1, false),
                maximum: decimal(5, 0, false),
                wrap_y: false,
            }),
        ),
        2 => (3, decimal(0, 0, false), decimal(1, 0, false), None),
        _ => unreachable!("fixed three-property definition"),
    };
    LimitsRecord {
        decimal_places: places,
        increment: decimal(1, 1, false),
        big_increment: decimal(10, 1, false),
        minimum,
        maximum,
        used: true,
        use_range: Some(RangeOptionsRecord::new(minimum, maximum)),
        curve_range,
    }
}

/// Fixed v5 constructor and wire definition for Exposure/Gamma/Vibrancy.
///
/// Limits here are immutable declared metadata, not source shared mutable Limits.
/// Public source setters, reset/clone, events and rendering remain separate.
#[derive(Debug)]
pub struct FlameColorSettingsRecord {
    set: ParameterSetId,
    properties: [ParameterPropertyId; 3],
    limits: [LimitsRecord; 3],
    /// Retained nonwire option. Its rendering semantics are not supplied here.
    pub option: bool,
}
impl FlameColorSettingsRecord {
    pub fn new_in(
        store: &mut ParameterRecordStore,
    ) -> Result<Self, ParameterStoreError<std::convert::Infallible>> {
        let names = ["Exposure", "Gamma", "Vibrancy"];
        let defaults = [0.0, 1.0, 0.0];
        let bounds = [[-2.0, 2.0], [f32::from_bits(0x3e4c_cccd), 5.0], [0.0, 1.0]];
        let properties = std::array::from_fn(|i| {
            let mut property = ModernPropertyRecord::new(
                ModernPropertyValue::Double {
                    current: defaults[i],
                    previous: defaults[i],
                },
                ModernPropertyConfiguration {
                    can_create_animation: true,
                    adjustment: Some(ModernAdjustmentConfiguration {
                        property_type: 0,
                        bounds: vec![bounds[i]],
                    }),
                },
            )
            .expect("fixed one-component definition");
            property.name = Some(names[i].encode_utf16().collect());
            property.adjustable = true;
            property.animatable = true;
            property
        });
        Self::from_backing_records(store, properties, false)
    }
    /// Build the fixed wire definition from explicit backing records. This is
    /// not the source clone/reset/setter API. Names, kinds and getter creation
    /// configuration must match the characterized v5 definition.
    pub fn from_backing_records(
        store: &mut ParameterRecordStore,
        records: [ModernPropertyRecord; 3],
        option: bool,
    ) -> Result<Self, ParameterStoreError<std::convert::Infallible>> {
        let names = ["Exposure", "Gamma", "Vibrancy"];
        let bounds = [[-2.0, 2.0], [f32::from_bits(0x3e4c_cccd), 5.0], [0.0, 1.0]];
        for (i, record) in records.iter().enumerate() {
            let config = record.configuration();
            if !matches!(record.value(), ModernPropertyValue::Double { .. })
                || record.name.as_deref()
                    != Some(names[i].encode_utf16().collect::<Vec<_>>().as_slice())
                || !record.adjustable
                || !record.animatable
                || !config.can_create_animation
                || !config
                    .adjustment
                    .as_ref()
                    .is_some_and(|a| a.property_type == 0 && a.bounds == vec![bounds[i]])
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
            limits: std::array::from_fn(limits),
            option,
        })
    }
    pub fn set_id(&self) -> &ParameterSetId {
        &self.set
    }
    pub fn property_ids(&self) -> &[ParameterPropertyId; 3] {
        &self.properties
    }
    pub fn declared_limits(&self) -> &[LimitsRecord; 3] {
        &self.limits
    }
    pub fn load(
        &mut self,
        store: &mut ParameterRecordStore,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ParameterStoreError<std::convert::Infallible>> {
        store.load(&self.set, reader, version)
    }
    pub fn save(
        &self,
        store: &mut ParameterRecordStore,
        writer: &mut BinaryWriter,
    ) -> Result<(), ParameterStoreError<std::convert::Infallible>> {
        store.save(&self.set, writer)
    }
}
