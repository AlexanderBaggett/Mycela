use std::{error::Error, fmt};

/// Exact CLR decimal storage, retaining scale and the sign of zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecimalBits([u32; 4]);
impl DecimalBits {
    pub const fn new(bits: [u32; 4]) -> Result<Self, BinaryError> {
        if bits[3] & 0x7f00_ffff != 0 || (bits[3] >> 16) & 0xff > 28 {
            return Err(BinaryError::DecimalBits);
        }
        Ok(Self(bits))
    }
    pub const fn bits(self) -> [u32; 4] {
        self.0
    }

    // Any three coefficient limbs are valid; preserve the validated sign/scale.
    pub(crate) const fn with_coefficient(mut self, coefficient: [u32; 3]) -> Self {
        self.0[0] = coefficient[0];
        self.0[1] = coefficient[1];
        self.0[2] = coefficient[2];
        self
    }

    // Removing one scale digit cannot introduce reserved flags or lose sign.
    pub(crate) const fn reduce_scale_once(mut self) -> Option<Self> {
        if (self.0[3] >> 16) & 0xff == 0 {
            None
        } else {
            self.0[3] -= 1 << 16;
            Some(self)
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Utf8State {
    bytes: [u8; 4],
    length: usize,
    expected: usize,
}
struct Decoded {
    units: [u16; 129],
    length: usize,
    capacity: usize,
}
impl Decoded {
    fn new(capacity: usize) -> Self {
        Self {
            units: [0; 129],
            length: 0,
            capacity: capacity.min(129),
        }
    }
    fn push(&mut self, units: &[u16]) -> Result<(), BinaryError> {
        let end = self.length + units.len();
        if end > self.capacity {
            return Err(BinaryError::CharacterCapacity);
        }
        self.units[self.length..end].copy_from_slice(units);
        self.length = end;
        Ok(())
    }
    fn values(&self) -> &[u16] {
        &self.units[..self.length]
    }
}
impl Utf8State {
    fn decode(&mut self, bytes: &[u8], capacity: usize) -> Result<Decoded, BinaryError> {
        // GetChars commits decoder state only after a successful call. A
        // character-buffer failure preserves the state from its previous call.
        let mut state = *self;
        let mut result = Decoded::new(capacity);
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if state.length == 0 {
                index += 1;
                match byte {
                    0..=0x7f => result.push(&[u16::from(byte)])?,
                    0xc2..=0xdf => {
                        state.bytes[0] = byte;
                        state.length = 1;
                        state.expected = 2;
                    }
                    0xe0..=0xef => {
                        state.bytes[0] = byte;
                        state.length = 1;
                        state.expected = 3;
                    }
                    0xf0..=0xf4 => {
                        state.bytes[0] = byte;
                        state.length = 1;
                        state.expected = 4;
                    }
                    _ => result.push(&[0xfffd])?,
                }
                continue;
            }
            if byte & 0xc0 != 0x80 {
                result.push(&[0xfffd])?;
                state.length = 0;
                // The non-continuation byte starts a new sequence.
                continue;
            }
            state.bytes[state.length] = byte;
            state.length += 1;
            index += 1;
            if state.length == 2
                && matches!(
                    (state.bytes[0], byte),
                    (0xe0, 0x80..=0x9f)
                        | (0xed, 0xa0..=0xbf)
                        | (0xf0, 0x80..=0x8f)
                        | (0xf4, 0x90..=0xbf)
                )
            {
                // This runtime replaces the rejected two-byte prefix as a
                // group, rather than replacing each byte independently.
                result.push(&[0xfffd])?;
                state.length = 0;
                continue;
            }
            if state.length == state.expected {
                let mut scalar = u32::from(state.bytes[0] & (0x7f >> state.expected));
                for byte in &state.bytes[1..state.expected] {
                    scalar = (scalar << 6) | u32::from(byte & 0x3f);
                }
                if scalar <= 0xffff {
                    result.push(&[scalar as u16])?;
                } else {
                    let scalar = scalar - 0x1_0000;
                    result.push(&[
                        0xd800 | (scalar >> 10) as u16,
                        0xdc00 | (scalar & 0x3ff) as u16,
                    ])?;
                }
                state.length = 0;
            }
        }
        // String boundaries and EOF do not flush an incomplete sequence.
        *self = state;
        Ok(result)
    }
}

/// Default compatibility-CLR binary decoding over immutable memory.
///
/// Text is returned as UTF-16 code units. The source decoder is shared by
/// string/character operations. Signed cursor overflow and partial failures
/// remain observable; every access still uses checked Rust slice bounds.
/// Custom/mutating streams and general I/O callbacks are separate work.
#[derive(Debug)]
pub struct BinaryCursor<'a> {
    input: &'a [u8],
    position: i32,
    closed: bool,
    null_receiver: bool,
    decoder: Utf8State,
}
impl<'a> BinaryCursor<'a> {
    pub(crate) fn fork(&self) -> Self {
        Self {
            input: self.input,
            position: self.position,
            closed: self.closed,
            null_receiver: self.null_receiver,
            decoder: self.decoder,
        }
    }
    pub(crate) fn null_receiver() -> Self {
        Self {
            input: &[],
            position: 0,
            closed: false,
            null_receiver: true,
            decoder: Utf8State::default(),
        }
    }
    pub fn new(input: &'a [u8]) -> Result<Self, BinaryError> {
        if input.len() > i32::MAX as usize {
            return Err(BinaryError::InputLength);
        }
        Ok(Self {
            input,
            position: 0,
            closed: false,
            null_receiver: false,
            decoder: Utf8State::default(),
        })
    }
    fn open(&self) -> Result<(), BinaryError> {
        if self.null_receiver {
            return Err(BinaryError::NullReceiver);
        }
        if self.closed {
            Err(BinaryError::Closed)
        } else {
            Ok(())
        }
    }
    pub const fn is_readable(&self) -> bool {
        !self.closed
    }
    pub fn position(&self) -> Result<i32, BinaryError> {
        self.open()?;
        Ok(self.position)
    }
    pub fn seek(&mut self, position: i64) -> Result<(), BinaryError> {
        if position < 0 {
            return Err(BinaryError::Position);
        }
        self.open()?;
        self.position = i32::try_from(position).map_err(|_| BinaryError::Position)?;
        Ok(())
    }
    /// Length of the actual caller stream; does not move its cursor.
    pub fn length(&self) -> Result<i32, BinaryError> {
        self.open()?;
        Ok(self.input.len() as i32)
    }
    /// BinaryReader.Read(Byte[], offset, count): validate before consuming, keep
    /// unread destination bytes intact, and return the actual partial count.
    pub(crate) fn read_into(
        &mut self,
        destination: &mut [u8],
        offset: usize,
        count: usize,
    ) -> Result<usize, BinaryError> {
        if offset > destination.len() || count > destination.len() - offset {
            return Err(BinaryError::ByteOffset);
        }
        let bytes = self.bulk(count)?;
        destination[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    pub fn close(&mut self) {
        self.closed = true;
    }
    pub fn read_u8(&mut self) -> Result<u8, BinaryError> {
        self.open()?;
        if self.position >= self.input.len() as i32 {
            return Err(BinaryError::End);
        }
        let index = self.position;
        self.position = self.position.wrapping_add(1);
        self.input
            .get(usize::try_from(index).map_err(|_| BinaryError::Index)?)
            .copied()
            .ok_or(BinaryError::Index)
    }
    pub fn read_bool(&mut self) -> Result<bool, BinaryError> {
        Ok(self.read_u8()? != 0)
    }
    fn bulk(&mut self, count: usize) -> Result<&'a [u8], BinaryError> {
        self.open()?;
        let available = (self.input.len() as i32).wrapping_sub(self.position);
        if available <= 0 || count == 0 {
            return Ok(&[]);
        }
        let length = count.min(available as usize);
        let bounds_error = if length <= 8 {
            BinaryError::Index
        } else {
            BinaryError::ByteOffset
        };
        let start = usize::try_from(self.position).map_err(|_| bounds_error)?;
        let end = start.checked_add(length).ok_or(bounds_error)?;
        let bytes = self.input.get(start..end).ok_or(bounds_error)?;
        self.position = self.position.wrapping_add(length as i32);
        Ok(bytes)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], BinaryError> {
        let bytes = self.bulk(N)?;
        if bytes.len() < N {
            return Err(BinaryError::End);
        }
        let mut value = [0; N];
        value.copy_from_slice(bytes);
        Ok(value)
    }
    pub fn read_i16(&mut self) -> Result<i16, BinaryError> {
        Ok(i16::from_le_bytes(self.fixed()?))
    }
    pub fn read_i32(&mut self) -> Result<i32, BinaryError> {
        self.open()?;
        // Exact MemoryStream's fast path mutates position before checking.
        self.position = self.position.wrapping_add(4);
        if self.position > self.input.len() as i32 {
            self.position = self.input.len() as i32;
            return Err(BinaryError::End);
        }
        let start =
            usize::try_from(self.position.wrapping_sub(4)).map_err(|_| BinaryError::Index)?;
        let end = start.checked_add(4).ok_or(BinaryError::Index)?;
        let bytes = self.input.get(start..end).ok_or(BinaryError::Index)?;
        let mut value = [0; 4];
        value.copy_from_slice(bytes);
        Ok(i32::from_le_bytes(value))
    }
    pub fn read_i64(&mut self) -> Result<i64, BinaryError> {
        Ok(i64::from_le_bytes(self.fixed()?))
    }
    pub fn read_f32(&mut self) -> Result<f32, BinaryError> {
        Ok(f32::from_le_bytes(self.fixed()?))
    }
    pub fn read_f64(&mut self) -> Result<f64, BinaryError> {
        Ok(f64::from_le_bytes(self.fixed()?))
    }
    pub fn read_decimal(&mut self) -> Result<DecimalBits, BinaryError> {
        let bytes = self.fixed::<16>()?;
        let bits = std::array::from_fn(|index| {
            u32::from_le_bytes(std::array::from_fn(|byte| bytes[index * 4 + byte]))
        });
        DecimalBits::new(bits).map_err(|_| BinaryError::DecimalEncoding)
    }
    pub fn read_seven_bit_int(&mut self) -> Result<i32, BinaryError> {
        let mut value = 0_i32;
        for shift in [0, 7, 14, 21, 28] {
            let byte = self.read_u8()?;
            value |= i32::from(byte & 0x7f).wrapping_shl(shift);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(BinaryError::LengthEncoding)
    }
    pub fn read_bytes(&mut self, count: i32) -> Result<Vec<u8>, BinaryError> {
        let count = usize::try_from(count).map_err(|_| BinaryError::Count)?;
        self.open()?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(count)
            .map_err(|_| BinaryError::Allocation)?;
        result.extend_from_slice(self.bulk(count)?);
        Ok(result)
    }
    pub fn read_string(&mut self) -> Result<Vec<u16>, BinaryError> {
        self.open()?;
        let length =
            usize::try_from(self.read_seven_bit_int()?).map_err(|_| BinaryError::NegativeLength)?;
        let mut result = Vec::new();
        let mut read = 0;
        while read < length {
            let bytes = self.bulk((length - read).min(128))?;
            if bytes.is_empty() {
                return Err(BinaryError::End);
            }
            let decoded = self.decoder.decode(bytes, 129)?;
            if read == 0 && bytes.len() != length {
                result
                    .try_reserve_exact(length)
                    .map_err(|_| BinaryError::Allocation)?;
            }
            result
                .try_reserve(decoded.length)
                .map_err(|_| BinaryError::Allocation)?;
            result.extend_from_slice(decoded.values());
            read += bytes.len();
        }
        Ok(result)
    }
    pub fn read_char(&mut self) -> Result<u16, BinaryError> {
        self.open()?;
        let start = self.position;
        loop {
            let byte = self.read_u8()?;
            match self.decoder.decode(&[byte], 1) {
                Ok(decoded) if decoded.length > 0 => return Ok(decoded.units[0]),
                Ok(_) => {}
                Err(error) => {
                    self.position = start;
                    return Err(error);
                }
            }
        }
    }
    pub fn read_chars(&mut self, count: i32) -> Result<Vec<u16>, BinaryError> {
        let count = usize::try_from(count).map_err(|_| BinaryError::Count)?;
        self.open()?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(count)
            .map_err(|_| BinaryError::Allocation)?;
        while result.len() < count {
            let remaining = count - result.len();
            let requested =
                (remaining - usize::from(self.decoder.length > 0 && remaining > 1)).min(128);
            let available = (self.input.len() as i32).wrapping_sub(self.position).max(0) as usize;
            let length = requested.min(available);
            let start = self.position;
            self.position = self.position.wrapping_add(length as i32);
            if length == 0 {
                break;
            }
            // InternalEmulateRead advances before GetChars validates offset.
            let start = usize::try_from(start).map_err(|_| BinaryError::ByteOffset)?;
            let end = start.checked_add(length).ok_or(BinaryError::ByteOffset)?;
            let bytes = self.input.get(start..end).ok_or(BinaryError::ByteOffset)?;
            result.extend_from_slice(self.decoder.decode(bytes, remaining)?.values());
        }
        Ok(result)
    }
}

/// Default strict-UTF8 binary writing to owned memory. Closing leaves already
/// written bytes accessible, as MemoryStream.ToArray does in the source.
#[derive(Debug, Default)]
pub struct BinaryWriter {
    bytes: Vec<u8>,
    fail_after: Option<usize>,
    writes: Vec<BinaryWriteObservation>,
    closed: bool,
    null_receiver: bool,
}
/// One actual caller transport invocation, including partial/zero commits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BinaryWriteObservation {
    pub offset: usize,
    pub requested: usize,
    pub committed: usize,
}
impl BinaryWriter {
    pub(crate) fn null_receiver() -> Self {
        Self {
            null_receiver: true,
            ..Self::default()
        }
    }
    /// Caller-supplied failure position; None retains the unlimited memory writer.
    pub fn set_write_failure_at(&mut self, position: Option<usize>) {
        self.fail_after = position;
    }
    pub fn writes(&self) -> &[BinaryWriteObservation] {
        &self.writes
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub const fn is_writable(&self) -> bool {
        !self.closed
    }
    pub fn position(&self) -> Result<usize, BinaryError> {
        if self.null_receiver {
            return Err(BinaryError::NullReceiver);
        }
        if self.closed {
            Err(BinaryError::Closed)
        } else {
            Ok(self.bytes.len())
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
    }
    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), BinaryError> {
        if self.null_receiver {
            return Err(BinaryError::NullReceiver);
        }
        if self.closed {
            return Err(BinaryError::Closed);
        }
        let offset = self.bytes.len();
        let accepted = self.fail_after.map_or(bytes.len(), |limit| {
            bytes.len().min(limit.saturating_sub(offset))
        });
        // CallerStream.Put checks the fault before each next byte. Bytes
        // accepted before that failure remain owned and observable.
        let result = self
            .bytes
            .try_reserve(accepted)
            .map_err(|_| BinaryError::Allocation);
        if result.is_ok() {
            self.bytes.extend_from_slice(&bytes[..accepted]);
        }
        self.writes.push(BinaryWriteObservation {
            offset,
            requested: bytes.len(),
            committed: self.bytes.len() - offset,
        });
        result?;
        if accepted < bytes.len() {
            Err(BinaryError::CallerWrite)
        } else {
            Ok(())
        }
    }
    pub fn write_u8(&mut self, value: u8) -> Result<(), BinaryError> {
        self.write_bytes(&[value])
    }
    pub fn write_bool(&mut self, value: bool) -> Result<(), BinaryError> {
        self.write_u8(u8::from(value))
    }
    pub fn write_i16(&mut self, value: i16) -> Result<(), BinaryError> {
        self.write_bytes(&value.to_le_bytes())
    }
    pub fn write_i32(&mut self, value: i32) -> Result<(), BinaryError> {
        self.write_bytes(&value.to_le_bytes())
    }
    pub fn write_i64(&mut self, value: i64) -> Result<(), BinaryError> {
        self.write_bytes(&value.to_le_bytes())
    }
    pub fn write_f32(&mut self, value: f32) -> Result<(), BinaryError> {
        self.write_bytes(&value.to_le_bytes())
    }
    pub fn write_f64(&mut self, value: f64) -> Result<(), BinaryError> {
        self.write_bytes(&value.to_le_bytes())
    }
    pub fn write_decimal(&mut self, value: DecimalBits) -> Result<(), BinaryError> {
        let bits = value.bits();
        let bytes: [u8; 16] = std::array::from_fn(|index| bits[index / 4].to_le_bytes()[index % 4]);
        self.write_bytes(&bytes)
    }
    pub fn write_seven_bit_int(&mut self, value: i32) -> Result<(), BinaryError> {
        let mut value = value as u32;
        while value >= 128 {
            self.write_u8((value | 128) as u8)?;
            value >>= 7;
        }
        self.write_u8(value as u8)
    }
    pub fn write_optional_utf16(&mut self, value: Option<&[u16]>) -> Result<(), BinaryError> {
        if self.null_receiver {
            return Err(BinaryError::NullReceiver);
        }
        self.write_utf16(value.ok_or(BinaryError::NullText)?)
    }
    pub fn write_utf16(&mut self, units: &[u16]) -> Result<(), BinaryError> {
        if self.null_receiver {
            return Err(BinaryError::NullReceiver);
        }
        // GetByteCount validates the complete input before any prefix is
        // written or the disposed stream is consulted.
        let mut length = 0_usize;
        for scalar in char::decode_utf16(units.iter().copied()) {
            length = length
                .checked_add(scalar.map_err(|_| BinaryError::Surrogate)?.len_utf8())
                .ok_or(BinaryError::Allocation)?;
        }
        let length = i32::try_from(length).map_err(|_| BinaryError::Allocation)?;
        self.write_seven_bit_int(length)?;
        // BCL Write(String) dispatches one <=256-byte payload, including an
        // empty payload. Larger payloads use 256/GetMaxByteCount(1)=42 UTF16
        // units per stateful encoder call; a boundary high surrogate is held.
        let mut pending = None;
        let chunk_units = if length <= 256 {
            units.len().max(1)
        } else {
            42
        };
        if units.is_empty() {
            self.write_bytes(&[])?;
        }
        for chunk in units.chunks(chunk_units) {
            let mut input = Vec::new();
            input
                .try_reserve(chunk.len() + usize::from(pending.is_some()))
                .map_err(|_| BinaryError::Allocation)?;
            if let Some(high) = pending.take() {
                input.push(high);
            }
            input.extend_from_slice(chunk);
            if input
                .last()
                .is_some_and(|unit| (0xd800..=0xdbff).contains(unit))
            {
                pending = input.pop();
            }
            let mut encoded = Vec::new();
            encoded
                .try_reserve(256)
                .map_err(|_| BinaryError::Allocation)?;
            for scalar in char::decode_utf16(input) {
                let scalar = scalar.map_err(|_| BinaryError::Surrogate)?;
                let mut bytes = [0; 4];
                encoded.extend_from_slice(scalar.encode_utf8(&mut bytes).as_bytes());
            }
            self.write_bytes(&encoded)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryError {
    End,
    LengthEncoding,
    NegativeLength,
    DecimalBits,
    DecimalEncoding,
    Count,
    Position,
    InputLength,
    Index,
    ByteOffset,
    CharacterCapacity,
    Surrogate,
    NullText,
    Closed,
    Allocation,
    NullReceiver,
    CallerWrite,
}
impl BinaryError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::End => "EndOfStreamException",
            Self::LengthEncoding => "FormatException",
            Self::NegativeLength | Self::DecimalEncoding | Self::CallerWrite => "IOException",
            Self::DecimalBits | Self::CharacterCapacity => "ArgumentException",
            Self::Count | Self::Position | Self::InputLength | Self::ByteOffset => {
                "ArgumentOutOfRangeException"
            }
            Self::Index => "IndexOutOfRangeException",
            Self::Surrogate => "EncoderFallbackException",
            Self::NullText => "ArgumentNullException",
            Self::Closed => "ObjectDisposedException",
            Self::Allocation => "OutOfMemoryException",
            Self::NullReceiver => "NullReferenceException",
        }
    }
}
impl fmt::Display for BinaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "binary {:?}: {}", self, self.source_class())
    }
}
impl Error for BinaryError {}

#[cfg(test)]
mod caller_write_laws {
    use super::*;
    #[test]
    fn caller_failure_preserves_own_prefix_and_can_resume_without_resetting_history() {
        let mut reference = BinaryWriter::default();
        reference.write_f64(-17.25).unwrap();
        let mut writer = BinaryWriter::default();
        writer.set_write_failure_at(Some(3));
        assert_eq!(writer.write_f64(-17.25), Err(BinaryError::CallerWrite));
        assert_eq!(writer.bytes(), &reference.bytes()[..3]);
        assert_eq!(
            writer.writes(),
            &[BinaryWriteObservation {
                offset: 0,
                requested: 8,
                committed: 3
            }]
        );
        assert_eq!(writer.write_u8(91), Err(BinaryError::CallerWrite));
        assert_eq!(
            writer.writes()[1],
            BinaryWriteObservation {
                offset: 3,
                requested: 1,
                committed: 0
            }
        );
        writer.write_bytes(&[]).unwrap();
        assert_eq!(
            writer.writes()[2],
            BinaryWriteObservation {
                offset: 3,
                requested: 0,
                committed: 0
            }
        );
        writer.set_write_failure_at(None);
        writer.write_bytes(&reference.bytes()[3..]).unwrap();
        assert_eq!(writer.bytes(), reference.bytes());
        assert_eq!(writer.writes().len(), 4);
    }
    #[test]
    fn strict_string_transport_retains_surrogate_pairs_across_large_encoder_chunks() {
        let mut text: Vec<u16> = "a".repeat(41).encode_utf16().collect();
        text.extend("🦊".encode_utf16());
        text.extend("z".repeat(300).encode_utf16());
        let mut writer = BinaryWriter::default();
        writer.write_utf16(&text).unwrap();
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        assert_eq!(reader.read_string().unwrap(), text);
        assert_eq!(reader.position().unwrap() as usize, writer.bytes().len());
        let before = writer.bytes().to_vec();
        let dispatches = writer.writes().len();
        assert_eq!(writer.write_utf16(&[0xd800]), Err(BinaryError::Surrogate));
        assert_eq!(writer.bytes(), before);
        assert_eq!(writer.writes().len(), dispatches);
    }
}
