//! Genetica's Environment value (rh), shared by GTX controls and .emap files.
#[path = "environment_record_blend.rs"]
mod blend;
#[path = "environment_record_edit.rs"]
mod edit;

#[path = "environment_record_image.rs"]
mod image;
#[path = "environment_record_mask.rs"]
mod mask;
#[path = "environment_record_references.rs"]
mod references;
pub use references::EnvironmentColorField;
pub(crate) use references::EnvironmentLabReference;
#[path = "environment_record_render.rs"]
mod render;
#[path = "environment_resource_library.rs"]
mod resource_library;

use crate::{
    BinaryCursor, BinaryError, BinaryWriter, GradientRecord, ModernAdjustmentConfiguration,
    ModernPropertyConfiguration, ModernPropertyError, ModernPropertyRecord, ModernPropertyValue,
    PropertyRecordError, SharedGradientRecord,
};
pub use blend::EnvironmentBlendMode;
pub use edit::{
    EnvironmentEditError, EnvironmentField, EnvironmentLayerKind, EnvironmentLight,
    EnvironmentNumericReceiver, EnvironmentTransformFilter,
};
pub use image::{EnvironmentImage, EnvironmentImageProjection};
pub use mask::{EnvironmentMaskFill, EnvironmentMaskHemisphere};
pub use render::{
    EnvironmentDisplayTone, EnvironmentLayerPreview, EnvironmentPreviewConverter,
    EnvironmentRenderSettings,
};
pub use resource_library::EnvironmentResourceLibrary;
use std::{error::Error, fmt, sync::Arc};

#[derive(Debug)]
pub enum EnvironmentWireError {
    Binary(BinaryError),
    Property(ModernPropertyError),
    Gradient(PropertyRecordError),
    LayerKind(i32),
    AssetKind(i32),
    ImageFormat(i32),
    Format,
    Version(i32),
}

impl From<BinaryError> for EnvironmentWireError {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}

impl From<ModernPropertyError> for EnvironmentWireError {
    fn from(error: ModernPropertyError) -> Self {
        Self::Property(error)
    }
}

impl fmt::Display for EnvironmentWireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Environment wire: {self:?}")
    }
}

impl Error for EnvironmentWireError {}

#[derive(Debug)]
pub struct EnvironmentRecord {
    layers: Vec<Arc<EnvironmentLayer>>,
    master: [ModernPropertyRecord; 2],
    transform: Transform,
}

impl Default for EnvironmentRecord {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            master: [EXPOSURE.record(), GAMMA.with_current(1.)],
            transform: Transform::default(),
        }
    }
}

impl EnvironmentRecord {
    fn library_image(&self, index: usize) -> Option<&ImageAsset> {
        let LayerSource::Image { asset, .. } = &self.layers.get(index)?.source else {
            return None;
        };
        asset
            .as_deref()
            .filter(|asset| matches!(asset, ImageAsset::Library { .. }))
    }

    pub fn library_image_path(&self, index: usize) -> Option<String> {
        let ImageAsset::Library { path, .. } = self.library_image(index)? else {
            return None;
        };
        Some(String::from_utf16_lossy(path))
    }

    pub(crate) fn uses_library_images(&self) -> bool {
        (0..self.layer_count()).any(|index| self.library_image(index).is_some())
    }

    pub fn read(reader: &mut BinaryCursor<'_>, version: f64) -> Result<Self, EnvironmentWireError> {
        let mut value = Self::default();
        value.load(reader, version)?;
        Ok(value)
    }

    pub(crate) fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), EnvironmentWireError> {
        self.layers.clear();
        let count = reader.read_i32()?;
        for _ in 0..count {
            let layer = EnvironmentLayer::read(reader, version)?;
            self.layers
                .try_reserve(1)
                .map_err(|_| BinaryError::Allocation)?;
            self.layers.push(Arc::new(layer));
        }
        for property in &mut self.master {
            property.load(reader, version)?;
        }
        self.transform = Transform::read(reader, version)?;
        Ok(())
    }

    /// Write the current (3.75) Environment representation.
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        writer.write_i32(i32::try_from(self.layers.len()).map_err(|_| BinaryError::Count)?)?;
        for layer in &self.layers {
            layer.save(writer)?;
        }
        save_properties(writer, &self.master)?;
        self.transform.save(writer)
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    pub fn exposure(&self) -> f64 {
        self.master[0].double_pair().expect("master Exposure").0
    }

    pub fn set_exposure(&mut self, exposure: f64) {
        if (-20.0..=20.0).contains(&exposure) && exposure != self.exposure() {
            let exposure = crate::round_decimal_places(exposure, 2).expect("Exposure precision");
            self.master[0].overwrite_double_current(exposure);
            self.master[0].copy_double_current_to_previous();
        }
    }
}

/// A library environment file retains its encoded thumbnail and editable value.
#[derive(Debug)]
pub struct EnvironmentPreset {
    thumbnail: Vec<u8>,
    environment: EnvironmentRecord,
}

impl EnvironmentPreset {
    const MAGIC: &str = "GeneticaEnvMap-@";

    pub fn new(environment: EnvironmentRecord, thumbnail: Vec<u8>) -> Self {
        Self {
            thumbnail,
            environment,
        }
    }

    pub fn thumbnail(&self) -> &[u8] {
        &self.thumbnail
    }

    pub fn read(reader: &mut BinaryCursor<'_>) -> Result<Self, EnvironmentWireError> {
        let (encoding, thumbnail) = Self::read_preview(reader)?;
        let environment = EnvironmentRecord::read(reader, f64::from(encoding) / 100.)?;
        Ok(Self {
            thumbnail,
            environment,
        })
    }

    /// Read only the catalog preview and version, leaving the cursor at the map.
    pub fn read_preview(
        reader: &mut BinaryCursor<'_>,
    ) -> Result<(i32, Vec<u8>), EnvironmentWireError> {
        if !reader
            .read_string()?
            .into_iter()
            .eq(Self::MAGIC.encode_utf16())
        {
            return Err(EnvironmentWireError::Format);
        }
        let encoding = reader.read_i32()?;
        if encoding > 375 {
            return Err(EnvironmentWireError::Version(encoding));
        }
        let length = reader.read_i32()?;
        let thumbnail = read_exact_bytes(reader, length)?;
        Ok((encoding, thumbnail))
    }

    pub fn environment(&self) -> &EnvironmentRecord {
        &self.environment
    }

    pub fn environment_mut(&mut self) -> &mut EnvironmentRecord {
        &mut self.environment
    }

    pub fn into_environment(self) -> EnvironmentRecord {
        self.environment
    }

    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        writer.write_utf16(&Self::MAGIC.encode_utf16().collect::<Vec<_>>())?;
        writer.write_i32(375)?;
        writer.write_i32(i32::try_from(self.thumbnail.len()).map_err(|_| BinaryError::Count)?)?;
        writer.write_bytes(&self.thumbnail)?;
        self.environment.save(writer)
    }
}

#[derive(Debug)]
struct EnvironmentLayer {
    source: LayerSource,
    mask: Mask,
    transform: Transform,
    color: [ModernPropertyRecord; 5],
    recolor: Option<ModernPropertyRecord>,
    blend: [ModernPropertyRecord; 9],
}

impl EnvironmentLayer {
    fn read(reader: &mut BinaryCursor<'_>, version: f64) -> Result<Self, EnvironmentWireError> {
        let source = LayerSource::read(reader, version)?;
        let mask = Mask::read(reader)?;
        let transform = Transform::read(reader, version)?;
        // ar writes its Boolean before Colorize, then Recolor only if enabled.
        let color = read_properties(reader, version, [CONTRAST, ANGLE, CONTRAST, FLAG, COLOR])?;
        let recolor = if matches!(color[3].value(), ModernPropertyValue::Boolean(true)) {
            let [gradient] = read_properties(reader, version, [Property::Gradient])?;
            Some(gradient)
        } else {
            None
        };
        let blend = read_properties(
            reader,
            version,
            [
                Property::Selection(18),
                Property::Integer([0., 400.]),
                GAMMA,
                FLAG,
                CONTRAST,
                CONTRAST,
                PERCENT,
                ANGLE,
                ELEVATION,
            ],
        )?;
        Ok(Self {
            source,
            mask,
            transform,
            color,
            recolor,
            blend,
        })
    }

    fn recolor_enabled(&self) -> bool {
        matches!(self.color[3].value(), ModernPropertyValue::Boolean(true))
    }

    fn mixer_enabled(&self) -> bool {
        matches!(self.blend[3].value(), ModernPropertyValue::Boolean(true))
    }

    fn recolor_value(&self) -> Option<&SharedGradientRecord> {
        let ModernPropertyValue::Gradient(gradient) = self.recolor.as_ref()?.value() else {
            return None;
        };
        gradient.as_ref()
    }

    fn save(&self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        self.source.save(writer)?;
        self.mask.save(writer)?;
        self.transform.save(writer)?;
        save_properties(writer, &self.color)?;
        if self.recolor_enabled() {
            self.recolor
                .as_ref()
                .ok_or(EnvironmentWireError::Format)?
                .copy_record_snapshot()
                .save(writer)?;
        }
        save_properties(writer, &self.blend)
    }
}

#[derive(Clone, Debug)]
struct Mask {
    fill: EnvironmentMaskFill,
    hemisphere: EnvironmentMaskHemisphere,
    amount: f64,
    colors: [[f64; 4]; 2],
    gradient: GradientRecord,
}

impl Mask {
    fn read(reader: &mut BinaryCursor<'_>) -> Result<Self, EnvironmentWireError> {
        let fill = EnvironmentMaskFill::from_wire(reader.read_i32()?);
        let hemisphere = EnvironmentMaskHemisphere::from_wire(reader.read_i32()?);
        let amount = reader.read_f64()?;
        let mut colors = [[0.; 4]; 2];
        for channel in colors.iter_mut().flatten() {
            *channel = reader.read_f64()?;
        }
        let mut gradient = GradientRecord::default();
        gradient
            .load(reader)
            .map_err(EnvironmentWireError::Gradient)?;
        Ok(Self {
            fill,
            hemisphere,
            amount,
            colors,
            gradient,
        })
    }

    fn save(&self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        writer.write_i32(self.fill as i32)?;
        writer.write_i32(self.hemisphere as i32)?;
        writer.write_f64(self.amount)?;
        for &channel in self.colors.iter().flatten() {
            writer.write_f64(channel)?;
        }
        self.gradient
            .save(writer)
            .map_err(EnvironmentWireError::Gradient)
    }
}

#[derive(Debug)]
struct Transform {
    properties: [ModernPropertyRecord; 6],
    filter_values: [Option<[f64; 2]>; 5],
}

impl Transform {
    fn read(reader: &mut BinaryCursor<'_>, version: f64) -> Result<Self, EnvironmentWireError> {
        let mut properties = read_properties(
            reader,
            version,
            [
                ANGLE,
                ANGLE,
                ANGLE,
                Property::Selection(4),
                PERCENT,
                PERCENT,
            ],
        )?;
        if version < 3.7 {
            // awj's legacy layout stores angles in radians.
            for property in &mut properties[..3] {
                if let ModernPropertyValue::Double { current, previous } = property.value_mut() {
                    *current = *current / std::f64::consts::PI * 180.;
                    *previous = *current;
                }
            }
        }
        Ok(Self {
            properties,
            filter_values: [None; 5],
        })
    }

    fn save(&self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        save_properties(writer, &self.properties)
    }
}

#[derive(Debug)]
enum LayerSource {
    Image {
        tone: Box<[ModernPropertyRecord; 2]>,
        asset: Option<Arc<ImageAsset>>,
        projection: i32,
    },
    Light(Box<[ModernPropertyRecord; 5]>),
    Color(Box<ModernPropertyRecord>),
}

impl LayerSource {
    fn read(reader: &mut BinaryCursor<'_>, version: f64) -> Result<Self, EnvironmentWireError> {
        Ok(match reader.read_i32()? {
            0 => Self::Image {
                tone: Box::new(read_properties(reader, version, [GAMMA, EXPOSURE])?),
                asset: if reader.read_bool()? {
                    Some(Arc::new(ImageAsset::read(reader, version)?))
                } else {
                    None
                },
                projection: reader.read_i32()?,
            },
            1 => Self::Light(Box::new(read_properties(
                reader,
                version,
                [Property::Selection(4), ANGLE, ELEVATION, PERCENT, PERCENT],
            )?)),
            2 => {
                let [color] = read_properties(reader, version, [COLOR])?;
                Self::Color(Box::new(color))
            }
            tag => return Err(EnvironmentWireError::LayerKind(tag)),
        })
    }

    fn save(&self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        match self {
            Self::Image {
                tone,
                asset,
                projection,
            } => {
                writer.write_i32(0)?;
                save_properties(writer, tone.as_ref())?;
                writer.write_bool(asset.is_some())?;
                if let Some(asset) = asset {
                    asset.save(writer)?;
                }
                writer.write_i32(*projection)?;
            }
            Self::Light(properties) => {
                writer.write_i32(1)?;
                save_properties(writer, properties.as_ref())?;
            }
            Self::Color(color) => {
                writer.write_i32(2)?;
                color.copy_record_snapshot().save(writer)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum ImageAsset {
    Library {
        path: Vec<u16>,
        file_name: Vec<u16>,
        size: [i32; 2],
        extension: Vec<u16>,
        library_values: [i32; 2],
    },
    Embedded(EnvironmentImage),
}

impl ImageAsset {
    fn blank_source<E>() -> Result<crate::SharedImageMap, crate::PointwiseError<E>> {
        let blank = crate::SharedImageMap::new(crate::ImageGeometry::new(256, 256));
        blank.set_constant(3, 1.0)?;
        Ok(blank)
    }

    pub(crate) fn source_geometry<E>(
        &self,
    ) -> Result<crate::ImageGeometry, crate::PointwiseError<E>> {
        match self {
            Self::Library {
                size: [width, height],
                ..
            } => Ok(crate::ImageGeometry::new(*width, *height)),
            Self::Embedded(image) => Ok(image
                .decoded()
                .map_err(|error| {
                    crate::PointwiseError::ImportedImage(crate::ImportedImageDecodeError::Decode(
                        error,
                    ))
                })?
                .geometry()),
        }
    }

    pub(crate) fn synthesis_source<E>(
        &self,
        width: i32,
        library: Option<&EnvironmentResourceLibrary>,
        execution: crate::ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<crate::SharedImageMap, crate::PointwiseError<E>> {
        match self {
            Self::Library {
                path,
                extension,
                size,
                ..
            } => {
                if let Some(library) = library {
                    let path = path
                        .strip_prefix(&[u16::from(b'.'), u16::from(b'\\')])
                        .unwrap_or(path);
                    if let Some(image) =
                        library.resolve(path, extension, *size, width, execution)?
                    {
                        return Ok(image);
                    }
                }
                Self::blank_source()
            }
            Self::Embedded(image) => {
                let source = image.decoded().map_err(|error| {
                    crate::PointwiseError::ImportedImage(crate::ImportedImageDecodeError::Decode(
                        error,
                    ))
                })?;
                image.fit_width(source, width, execution, cancelled)
            }
        }
    }

    pub(crate) fn synthesis_editor_source<E>(
        &self,
        embedded_width: i32,
        library: Option<&EnvironmentResourceLibrary>,
        execution: crate::ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<crate::SharedImageMap, crate::PointwiseError<E>> {
        if let Self::Library {
            path,
            extension,
            size,
            ..
        } = self
        {
            let path = path
                .strip_prefix(&[u16::from(b'.'), u16::from(b'\\')])
                .unwrap_or(path);
            return library
                .and_then(|library| library.editor_source(path, extension, *size))
                .map_or_else(Self::blank_source, Ok);
        }
        self.synthesis_source(embedded_width, library, execution, cancelled)
    }

    pub(crate) fn read(
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<Self, EnvironmentWireError> {
        Ok(match reader.read_i32()? {
            0 => {
                let mut path = reader.read_string()?;
                if version == 3.58 {
                    if path.starts_with(&[92, 0xfffd, 92]) {
                        path.drain(..3);
                    }
                    while path.starts_with(&[0xfffd, 92]) {
                        path.drain(..2);
                    }
                }
                Self::Library {
                    path,
                    file_name: reader.read_string()?,
                    size: [reader.read_i32()?, reader.read_i32()?],
                    extension: reader.read_string()?,
                    library_values: [reader.read_i32()?, reader.read_i32()?],
                }
            }
            1 => {
                let format = reader.read_i32()?;
                if !(0..=3).contains(&format) {
                    return Err(EnvironmentWireError::ImageFormat(format));
                }
                let length = reader.read_i32()?.max(0);
                Self::Embedded(EnvironmentImage::from_encoded(
                    format,
                    read_exact_bytes(reader, length)?,
                ))
            }
            tag => return Err(EnvironmentWireError::AssetKind(tag)),
        })
    }

    pub(crate) fn save(&self, writer: &mut BinaryWriter) -> Result<(), EnvironmentWireError> {
        match self {
            Self::Library {
                path,
                file_name,
                size,
                extension,
                library_values,
            } => {
                writer.write_i32(0)?;
                writer.write_utf16(path)?;
                writer.write_utf16(file_name)?;
                for dimension in size {
                    writer.write_i32(*dimension)?;
                }
                writer.write_utf16(extension)?;
                for value in library_values {
                    writer.write_i32(*value)?;
                }
            }
            Self::Embedded(image) => {
                let (format, bytes) = image.encoded();
                writer.write_i32(1)?;
                writer.write_i32(format)?;
                writer.write_i32(i32::try_from(bytes.len()).map_err(|_| BinaryError::Count)?)?;
                writer.write_bytes(bytes)?;
            }
        }
        Ok(())
    }
}

fn read_exact_bytes(reader: &mut BinaryCursor<'_>, count: i32) -> Result<Vec<u8>, BinaryError> {
    if count < 0 {
        return Err(BinaryError::Count);
    }
    if count > reader.length()?.saturating_sub(reader.position()?) {
        return Err(BinaryError::End);
    }
    reader.read_bytes(count)
}

#[derive(Clone, Copy)]
enum Property {
    Double([f32; 2]),
    Integer([f32; 2]),
    Selection(i32),
    Flag,
    Color,
    Gradient,
}

const ANGLE: Property = Property::Double([0., 360.]);
const EXPOSURE: Property = Property::Double([-2., 2.]);
const GAMMA: Property = Property::Double([0.2, 5.]);
const PERCENT: Property = Property::Double([0., 100.]);
const CONTRAST: Property = Property::Double([-100., 100.]);
const ELEVATION: Property = Property::Double([0., 90.]);
const COLOR: Property = Property::Color;
const FLAG: Property = Property::Flag;

impl Property {
    fn with_current(self, current: f64) -> ModernPropertyRecord {
        let mut record = self.record();
        record.overwrite_double_current(current);
        record.copy_double_current_to_previous();
        record
    }

    fn record(self) -> ModernPropertyRecord {
        use ModernPropertyValue as Value;
        let (value, tag, bounds, animatable) = match self {
            Self::Double(bounds) => (
                Value::Double {
                    current: 0.,
                    previous: 0.,
                },
                0,
                bounds,
                true,
            ),
            Self::Integer(bounds) => (
                Value::Int32 {
                    current: 0,
                    previous: 0,
                },
                0,
                bounds,
                true,
            ),
            Self::Selection(count) => (
                Value::Selected {
                    current: 0,
                    previous: 0,
                },
                4,
                [0.5, count as f32 + 0.5],
                true,
            ),
            Self::Flag => (Value::Boolean(false), 4, [0., 1.], true),
            Self::Color => (
                Value::Color {
                    argb: [0.; 4],
                    grayscale: false,
                },
                6,
                [0., 100.],
                true,
            ),
            Self::Gradient => (
                Value::Gradient(Some(SharedGradientRecord::new(GradientRecord::default()))),
                7,
                [0., 0.],
                false,
            ),
        };
        ModernPropertyRecord::new(
            value,
            ModernPropertyConfiguration {
                can_create_animation: animatable,
                adjustment: Some(ModernAdjustmentConfiguration {
                    property_type: tag,
                    bounds: vec![bounds],
                }),
            },
        )
        .expect("Environment properties each have one wire component")
    }
}

fn read_properties<const N: usize>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    specs: [Property; N],
) -> Result<[ModernPropertyRecord; N], EnvironmentWireError> {
    let mut properties = specs.map(Property::record);
    for property in &mut properties {
        property.load(reader, version)?;
    }
    Ok(properties)
}

fn save_properties(
    writer: &mut BinaryWriter,
    properties: &[ModernPropertyRecord],
) -> Result<(), EnvironmentWireError> {
    for property in properties {
        property.copy_record_snapshot().save(writer)?;
    }
    Ok(())
}
