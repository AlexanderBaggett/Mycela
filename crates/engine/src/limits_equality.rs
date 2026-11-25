use crate::{CurveRangeRecord, DecimalBits, LimitsRecord, RangeOptionsRecord};

impl DecimalBits {
    /// Numeric CLR decimal equality, distinct from raw wire-storage equality.
    /// Neither operand's scale or sign bits are rewritten.
    pub fn numeric_equals(self, other: Self) -> bool {
        fn canonical(value: DecimalBits) -> (u128, u32, bool) {
            let [low, middle, high, flags] = value.bits();
            let mut magnitude =
                u128::from(low) | (u128::from(middle) << 32) | (u128::from(high) << 64);
            if magnitude == 0 {
                return (0, 0, false);
            }
            let mut scale = (flags >> 16) & 0xff;
            while scale > 0 && magnitude % 10 == 0 {
                magnitude /= 10;
                scale -= 1;
            }
            (magnitude, scale, flags & 0x8000_0000 != 0)
        }
        canonical(self) == canonical(other)
    }
}

impl RangeOptionsRecord {
    /// Original ad4 value comparison. A NaN multiplier is unequal even to
    /// itself. None represents a null or rejected non-ad4 argument.
    pub fn source_equals(&self, other: Option<&Self>) -> bool {
        let Some(other) = other else {
            return false;
        };
        self.flags[1] == other.flags[1]
            && self.multiplier == other.multiplier
            && self.maximum.numeric_equals(other.maximum)
            && self.minimum.numeric_equals(other.minimum)
            && self.flags[2] == other.flags[2]
            && self.flags[0] == other.flags[0]
            && self.decimal_places == other.decimal_places
    }
}

impl CurveRangeRecord {
    /// Original CurveRange value comparison; raw decimal bits remain intact.
    pub fn source_equals(&self, other: Option<&Self>) -> bool {
        let Some(other) = other else {
            return false;
        };
        self.maximum.numeric_equals(other.maximum)
            && self.minimum.numeric_equals(other.minimum)
            && self.wrap_y == other.wrap_y
    }
}

impl LimitsRecord {
    /// Original ordered Limits value comparison, including its conditional
    /// child check and omission of BigIncrement. This is not an equivalence
    /// relation and must not implement PartialEq or be used for hash keys.
    /// Owned wire records do not establish source child-root sharing.
    pub fn source_equals(&self, other: Option<&Self>) -> bool {
        let Some(other) = other else {
            return false;
        };
        let use_equal = match &self.use_range {
            Some(range) => range.source_equals(other.use_range.as_ref()),
            None => other.use_range.is_none(),
        };
        let child_equal = match (use_equal, &self.curve_range) {
            (true, Some(curve)) => curve.source_equals(other.curve_range.as_ref()),
            _ => other.curve_range.is_none(),
        };
        self.decimal_places == other.decimal_places
            && self.increment.numeric_equals(other.increment)
            && self.minimum.numeric_equals(other.minimum)
            && self.maximum.numeric_equals(other.maximum)
            && self.used == other.used
            && child_equal
    }
}
