use crate::{BinaryCursor, BinaryError, BinaryWriter};
use std::{error::Error, fmt};

/// Validated DateTime ticks used by the original contributor wire format.
/// Calendar formatting and DateTime Kind are outside this value capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContributorTicks(i64);
impl ContributorTicks {
    pub const MAX: i64 = 3_155_378_975_999_999_999;

    pub const fn new(ticks: i64) -> Result<Self, ContributorRecordError> {
        if ticks < 0 || ticks > Self::MAX {
            Err(ContributorRecordError::Ticks)
        } else {
            Ok(Self(ticks))
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Owned contributor getter values. Source null text getters become empty units.
/// UTF-16 is retained, including malformed text whose serialization can fail.
#[derive(Debug, Eq, PartialEq)]
pub struct ContributorRecord {
    pub name: Vec<u16>,
    pub website: Vec<u16>,
    pub email: Vec<u16>,
    pub ticks: ContributorTicks,
    pub comment: Vec<u16>,
}
impl ContributorRecord {
    /// Constructor read: failure leaves the cursor consumed but exposes no record.
    pub fn load(reader: &mut BinaryCursor<'_>) -> Result<Self, ContributorRecordError> {
        let name = reader.read_string()?;
        let website = reader.read_string()?;
        let email = reader.read_string()?;
        let ticks = ContributorTicks::new(reader.read_i64()?)?;
        let mut comment = reader.read_string()?;
        let (start, end) = trimmed_bounds(&comment);
        comment.truncate(end);
        comment.drain(..start);
        Ok(Self {
            name,
            website,
            email,
            ticks,
            comment,
        })
    }

    /// Trims only the serialized comment; preserves stored text and partial output.
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), ContributorRecordError> {
        writer.write_utf16(&self.name)?;
        writer.write_utf16(&self.website)?;
        writer.write_utf16(&self.email)?;
        writer.write_i64(self.ticks.get())?;
        let (start, end) = trimmed_bounds(&self.comment);
        writer.write_utf16(&self.comment[start..end])?;
        Ok(())
    }

    pub fn display_name(&self) -> &[u16] {
        const ANONYMOUS: &[u16] = &[65, 110, 111, 110, 121, 109, 111, 117, 115];
        if self.name.is_empty() {
            ANONYMOUS
        } else {
            &self.name
        }
    }
}

fn trimmed_bounds(text: &[u16]) -> (usize, usize) {
    let mut start = 0;
    let mut end = text.len();
    while start < end && is_whitespace(text[start]) {
        start += 1;
    }
    while end > start && is_whitespace(text[end - 1]) {
        end -= 1;
    }
    (start, end)
}

// F062: exact executed CLR Char.IsWhiteSpace profile, rather than host Unicode.
fn is_whitespace(unit: u16) -> bool {
    matches!(unit, 9..=13 | 32 | 133 | 160 | 5760 | 8192..=8202 | 8232 | 8233 | 8239 | 8287 | 12288)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContributorRecordError {
    Binary(BinaryError),
    Ticks,
}
impl ContributorRecordError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(error) => error.source_class(),
            Self::Ticks => "ArgumentOutOfRangeException",
        }
    }
}
impl From<BinaryError> for ContributorRecordError {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl fmt::Display for ContributorRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary(error) => error.fmt(f),
            Self::Ticks => f.write_str("contributor ticks are outside the source DateTime range"),
        }
    }
}
impl Error for ContributorRecordError {}
