use crate::{FloatBuffer, ImageError, ImageGeometry, ImageMap};

/// Invert's stored, unanimated selection values and CPU image transform.
///
/// Input graph evaluation and legacy selection-property events are owned by
/// their respective future layers. This value stores no shadow input image,
/// cache guarantees, animation values, or property identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredInvert {
    options: [i32; 4],
}
impl Default for StoredInvert {
    fn default() -> Self {
        Self {
            options: [0, 0, 0, 1],
        }
    }
}
impl StoredInvert {
    pub const fn options(&self) -> [i32; 4] {
        self.options
    }
    /// Negative indices are ignored by the original selection setter; positive
    /// indices outside Yes/No remain stored and fail when text is resolved.
    pub fn set_option(&mut self, channel: i32, index: i32) -> Result<(), ImageError> {
        let slot = usize::try_from(channel)
            .ok()
            .and_then(|c| self.options.get_mut(c))
            .ok_or(ImageError::Index)?;
        if index >= 0 {
            *slot = index;
        }
        Ok(())
    }
    pub fn red_only(&mut self) {
        self.options = [0, 1, 1, 1];
    }
    pub fn bypass_input(&self) -> bool {
        self.options == [1; 4]
    }
    /// Scalar xe.a6 arithmetic over an already resolved input scalar.
    /// Out-of-range channels pass the input through, matching xe.g(int).
    /// This does not populate or emulate the source graph's scalar cache.
    pub fn scalar(&self, channel: i32, input: f32) -> f32 {
        if usize::try_from(channel)
            .ok()
            .and_then(|c| self.options.get(c))
            == Some(&0)
        {
            invert_value(input)
        } else {
            input
        }
    }
    /// Source xe.a7 with an already resolved child constant guarantee.
    pub fn is_channel_constant(&self, channel: i32, input_constant: bool) -> bool {
        input_constant && (0..4).contains(&channel)
    }
    pub fn render(
        &self,
        input: &mut ImageMap,
        width: i32,
        height: i32,
    ) -> Result<ImageMap, ImageError> {
        self.render_optional(Some(input), width, height)
    }
    /// Explicit missing-map input preserves the original k8 null error path.
    /// The upstream graph/override-array resolution occurs before this seam.
    pub fn render_optional(
        &self,
        input: Option<&mut ImageMap>,
        width: i32,
        height: i32,
    ) -> Result<ImageMap, ImageError> {
        // Original resolves all four selection strings before reading channels.
        let mut ignored = [false; 4];
        for (ignored, index) in ignored.iter_mut().zip(self.options) {
            *ignored = match index {
                0 => false,
                1 => true,
                _ => return Err(ImageError::Index),
            };
        }
        render_resolved_channels(input, width, height, ignored)
    }
}

pub(crate) fn render_resolved_channels(
    input: Option<&mut ImageMap>,
    width: i32,
    height: i32,
    ignored: [bool; 4],
) -> Result<ImageMap, ImageError> {
    let mut output = ImageMap::new(ImageGeometry::new(width, height));
    let input = input.ok_or(ImageError::NullReference)?;
    for (channel, ignored) in ignored.into_iter().enumerate() {
        let channel = channel as i32;
        let source = input.channel(channel, false)?;
        if ignored {
            output.set_buffer(channel, &source)?;
        } else {
            let destination = if source.len() == 1 {
                output.channel(channel, false)?
            } else {
                output.materialize(channel)?
            };
            InvertFloatWorker::new(Some(source), Some(destination)).run()?;
        }
        // xe.k8 seals each ignored alias or completed transformed store.
        output.seal_channel(channel)?;
    }
    output.set_region_store(input.region_store());
    output.replace_named_channels_from(input)?;
    Ok(output)
}

/// Retained source/destination arrays for aj1's ordered CPU loop. The handles
/// may identify the same array and may be absent, as in the original constructors.
#[derive(Clone, Debug)]
pub struct InvertFloatWorker {
    source: Option<FloatBuffer>,
    destination: Option<FloatBuffer>,
}
impl InvertFloatWorker {
    pub fn new(source: Option<FloatBuffer>, destination: Option<FloatBuffer>) -> Self {
        Self {
            source,
            destination,
        }
    }
    pub fn in_place(buffer: Option<FloatBuffer>) -> Self {
        Self {
            source: buffer.clone(),
            destination: buffer,
        }
    }
    pub fn len(&self) -> Result<i32, ImageError> {
        let source = self.source.as_ref().ok_or(ImageError::NullReference)?;
        i32::try_from(source.len()).map_err(|_| ImageError::Overflow)
    }
    pub fn is_empty(&self) -> Result<bool, ImageError> {
        self.len().map(|length| length == 0)
    }
    pub fn run(&self) -> Result<(), ImageError> {
        self.run_range(0, self.len()?)
    }
    /// End arithmetic wraps before the first iteration. Each source element is
    /// read before its destination write; errors preserve all preceding writes.
    pub fn run_range(&self, start: i32, count: i32) -> Result<(), ImageError> {
        let end = start.wrapping_add(count);
        let mut index = start;
        while index < end {
            let source = self.source.as_ref().ok_or(ImageError::NullReference)?;
            let value = source.read(usize::try_from(index).map_err(|_| ImageError::Index)?)?;
            let destination = self.destination.as_ref().ok_or(ImageError::NullReference)?;
            destination.write(index as usize, invert_value(value))?;
            index = index.wrapping_add(1);
        }
        Ok(())
    }
}
fn invert_value(value: f32) -> f32 {
    if value.is_nan() {
        f32::from_bits(value.to_bits() | 0x0040_0000)
    } else {
        1.0 - value
    }
}
