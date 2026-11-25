use std::{error::Error, fmt};

/// An inline BGRA pixel, owned by value and compared by its packed channels.
///
/// The numeric representation is B | (G << 8) | (R << 16) | (A << 24).
/// Byte conversion explicitly uses little endian, preserving the source packing
/// on every host without overlapping mutable references or pointer casts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct ColorBgra(u32);

impl ColorBgra {
    pub const fn new(blue: u8, green: u8, red: u8, alpha: u8) -> Self {
        Self(u32::from_le_bytes([blue, green, red, alpha]))
    }

    pub const fn from_packed(value: u32) -> Self {
        Self(value)
    }

    pub const fn packed(self) -> u32 {
        self.0
    }

    pub const fn components(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }

    /// Reads a channel in B, G, R, A order, retaining the original invalid index
    /// in an error when it is outside 0..=3.
    pub fn channel(self, index: i32) -> Result<u8, ChannelIndexError> {
        self.components()
            .get(index as usize)
            .copied()
            .ok_or(ChannelIndexError { index })
    }

    /// Changes one channel. Invalid indices leave the value unchanged.
    pub fn set_channel(&mut self, index: i32, value: u8) -> Result<(), ChannelIndexError> {
        let mut components = self.components();
        let channel = components
            .get_mut(index as usize)
            .ok_or(ChannelIndexError { index })?;
        *channel = value;
        self.0 = u32::from_le_bytes(components);
        Ok(())
    }
}

/// A channel index rejected by the engine, corresponding to the source's
/// ArgumentOutOfRangeException for the `channel` parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelIndexError {
    index: i32,
}

impl ChannelIndexError {
    pub const fn index(self) -> i32 {
        self.index
    }

    pub const fn parameter(self) -> &'static str {
        "channel"
    }
}

impl fmt::Display for ChannelIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "channel index {} is outside 0..=3", self.index)
    }
}

impl Error for ChannelIndexError {}

const _: () = assert!(std::mem::size_of::<ColorBgra>() == 4);
const _: () = assert!(std::mem::align_of::<ColorBgra>() == 4);
