//! Source `an3` variable specification, including literal option expansion.
//! The host supplies directory contents; no process-global installation path or
//! empty fallback is invented when a wildcard needs filesystem access.
use crate::{BinaryCursor, BinaryError, BinaryWriter};
use std::{fmt, marker::PhantomData, sync::Arc};

/// One resolved display label and optional resource path.
pub type VariableOption = (Vec<u16>, Option<Vec<u16>>);

pub const VARIABLE_INACTIVE: u8 = 0;
pub const VARIABLE_NUMERIC: u8 = 1;
pub const VARIABLE_SELECTION: u8 = 2;
pub const VARIABLE_COLOR: u8 = 3;
pub const VARIABLE_ENVIRONMENT: u8 = 4;
pub const VARIABLE_TEXTURE_SYNTHESIS: u8 = 5;

/// Each value is UTF-16, retaining null and unpaired code units. Unknown mode
/// bytes are preserved because the source casts without validating the enum.
#[derive(Clone, Debug, Default)]
pub struct VariableSpec {
    mode: u8,
    name: Option<Vec<u16>>,
    negative_range: bool,
    whole_numbers: bool,
    nonzero: bool,
    options: Option<Vec<u16>>,
    resolved: Option<Arc<[VariableOption]>>,
}
#[derive(Debug)]
pub enum VariableSpecError<E> {
    Binary(BinaryError),
    Host(E),
    ConflictingBounds,
}
impl<E> From<BinaryError> for VariableSpecError<E> {
    fn from(value: BinaryError) -> Self {
        Self::Binary(value)
    }
}
impl<E: fmt::Display> fmt::Display for VariableSpecError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary(e) => write!(f, "{e}"),
            Self::Host(e) => write!(f, "{e}"),
            Self::ConflictingBounds => write!(f, "Found both NegRange and NotZero."),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for VariableSpecError<E> {}

/// The caller supplies the original installation root and actual directory
/// enumeration, preserving the host's order and its real failure.
pub trait VariableFileService {
    type Error;
    fn files(&mut self, directory: &[u16]) -> Result<Vec<Vec<u16>>, Self::Error>;
}

/// Enumerates the actual host directory without sorting or silently replacing
/// filesystem failures. The mapper lets callers preserve their own error type.
pub struct NativeVariableFiles<E, F> {
    map_error: F,
    marker: PhantomData<fn() -> E>,
}

impl<E, F> NativeVariableFiles<E, F> {
    pub fn new(map_error: F) -> Self {
        Self {
            map_error,
            marker: PhantomData,
        }
    }
}

impl<E, F: FnMut(std::io::Error) -> E> VariableFileService for NativeVariableFiles<E, F> {
    type Error = E;

    fn files(&mut self, directory: &[u16]) -> Result<Vec<Vec<u16>>, E> {
        let path = String::from_utf16(directory).map_err(|error| {
            (self.map_error)(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?;
        // WTX resource paths use Windows separators even on other platforms.
        #[cfg(not(windows))]
        let path = path.replace('\\', "/");
        let entries = std::fs::read_dir(path).map_err(&mut self.map_error)?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(&mut self.map_error)?;
            if !entry.metadata().map_err(&mut self.map_error)?.is_file() {
                continue;
            }
            let path = entry.path();
            let path = path.to_str().ok_or_else(|| {
                (self.map_error)(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "directory entry is not Unicode",
                ))
            })?;
            paths.push(path.encode_utf16().collect());
        }
        Ok(paths)
    }
}
impl VariableSpec {
    pub fn new(mode: u8) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }
    pub fn mode(&self) -> u8 {
        self.mode
    }
    pub fn set_mode(&mut self, value: u8) {
        self.mode = value;
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn set_name(&mut self, value: Option<&[u16]>) {
        self.name = value.map(<[u16]>::to_vec);
    }
    pub fn numeric_flags(&self) -> [bool; 3] {
        [self.negative_range, self.whole_numbers, self.nonzero]
    }
    pub fn set_numeric_flags(&mut self, negative: bool, whole: bool, nonzero: bool) {
        self.negative_range = negative;
        self.whole_numbers = whole;
        self.nonzero = nonzero;
    }
    pub fn options(&self) -> Option<&[u16]> {
        self.options.as_deref()
    }
    pub fn resolved_options(&self) -> Option<&[VariableOption]> {
        self.resolved.as_deref()
    }
    /// Source Equals compares the serialized fields, not the derived matrix.
    pub fn source_equals(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.name == other.name
            && self.options == other.options
            && self.numeric_flags() == other.numeric_flags()
    }
    /// Assigns the source text before enumeration; on host failure the old
    /// resolved matrix remains, matching the original struct's setter prefix.
    pub fn set_options<S: VariableFileService + ?Sized>(
        &mut self,
        value: Option<&[u16]>,
        base: &[u16],
        service: &mut S,
    ) -> Result<(), VariableSpecError<S::Error>> {
        if self.options.as_deref() == value {
            return Ok(());
        }
        self.options = value.map(<[u16]>::to_vec);
        let Some(value) = value else {
            self.resolved = None;
            return Ok(());
        };
        let mut expanded = Vec::new();
        for item in value.split(|&c| c == b';' as u16) {
            let mut item = item.to_vec();
            if ends(&item, "gtx") {
                item.truncate(item.len() - 3);
                item.extend("htx".encode_utf16());
            }
            if item
                .windows(3)
                .any(|w| w == [b'*' as u16, b'.' as u16, b'*' as u16])
            {
                let prefix = &item[..item.len() - 3];
                let mut directory = base.to_vec();
                directory.extend_from_slice(prefix);
                for path in service.files(&directory).map_err(VariableSpecError::Host)? {
                    let file = path
                        .rsplit(|&c| c == b'\\' as u16 || c == b'/' as u16)
                        .next()
                        .unwrap_or(&path);
                    if has_extension(file, "htx") || has_extension(file, "wtx") {
                        let mut entry = prefix.to_vec();
                        entry.extend_from_slice(file);
                        expanded.push(entry);
                    }
                }
            } else {
                expanded.push(item);
            }
        }
        self.resolved = Some(
            expanded
                .into_iter()
                .map(|entry| {
                    let mut parts = entry.split(|&c| c == b'|' as u16);
                    let first = parts.next().unwrap_or_default();
                    let second = parts.next();
                    if second.is_none() && (ends(first, ".htx") || ends(first, ".wtx")) {
                        let start = first
                            .iter()
                            .rposition(|&c| c == b'\\' as u16)
                            .map_or(0, |i| i + 1);
                        (first[start..first.len() - 4].to_vec(), Some(first.to_vec()))
                    } else {
                        (first.to_vec(), second.map(<[u16]>::to_vec))
                    }
                })
                .collect::<Vec<_>>()
                .into(),
        );
        Ok(())
    }
    /// Mutates in source field order. Callers modeling os binary loading should
    /// load a local copy and publish only after success, as the original does.
    pub fn load<S: VariableFileService + ?Sized>(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        base: &[u16],
        service: &mut S,
    ) -> Result<(), VariableSpecError<S::Error>> {
        self.mode = reader.read_u8()?;
        self.name = Some(reader.read_string()?);
        if self.mode == VARIABLE_SELECTION {
            let options = reader.read_string()?;
            self.set_options(Some(&options), base, service)?;
        } else if self.mode == VARIABLE_NUMERIC {
            self.negative_range = reader.read_bool()?;
            self.whole_numbers = reader.read_bool()?;
            self.nonzero = reader.read_bool()?;
        }
        if self.negative_range && self.nonzero {
            return Err(VariableSpecError::ConflictingBounds);
        }
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        writer.write_u8(self.mode)?;
        writer.write_utf16(self.name.as_deref().unwrap_or_default())?;
        if self.mode == VARIABLE_SELECTION {
            writer.write_utf16(self.options.as_deref().unwrap_or_default())?;
        } else if self.mode == VARIABLE_NUMERIC {
            writer.write_bool(self.negative_range)?;
            writer.write_bool(self.whole_numbers)?;
            writer.write_bool(self.nonzero)?;
        }
        Ok(())
    }
}
fn ends(units: &[u16], suffix: &str) -> bool {
    units.ends_with(&suffix.encode_utf16().collect::<Vec<_>>())
}
fn has_extension(file: &[u16], extension: &str) -> bool {
    let Some(dot) = file.iter().rposition(|&c| c == b'.' as u16) else {
        return false;
    };
    let expected: Vec<u16> = extension.encode_utf16().collect();
    file[dot + 1..].len() == expected.len()
        && file[dot + 1..]
            .iter()
            .zip(expected)
            .all(|(&actual, expected)| {
                actual == expected
                    || (actual >= b'A' as u16 && actual <= b'Z' as u16 && actual + 32 == expected)
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Files {
        seen: Vec<Vec<u16>>,
        result: Vec<Vec<u16>>,
        fail: bool,
    }
    impl VariableFileService for Files {
        type Error = &'static str;
        fn files(&mut self, path: &[u16]) -> Result<Vec<Vec<u16>>, Self::Error> {
            self.seen.push(path.to_vec());
            if self.fail {
                Err("host")
            } else {
                Ok(self.result.clone())
            }
        }
    }
    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }
    #[test]
    fn selection_expansion_uses_host_order_source_names_and_literal_paths() {
        let mut files = Files {
            result: vec![
                u("C:\\base\\lib\\First.WTX"),
                u("C:\\base\\lib\\skip.txt"),
                u("C:\\base\\lib\\Second.htx"),
            ],
            ..Files::default()
        };
        let mut spec = VariableSpec::new(VARIABLE_SELECTION);
        spec.set_options(
            Some(&u("A|b;Wood\\Thing.gtx;lib\\*.*;plain;;")),
            &u("C:\\base\\"),
            &mut files,
        )
        .unwrap();
        assert_eq!(files.seen, vec![u("C:\\base\\lib\\")]);
        assert_eq!(
            spec.resolved_options().unwrap(),
            &[
                (u("A"), Some(u("b"))),
                (u("Thing"), Some(u("Wood\\Thing.htx"))),
                (u("lib\\First.WTX"), None),
                (u("Second"), Some(u("lib\\Second.htx"))),
                (u("plain"), None),
                (vec![], None),
                (vec![], None)
            ]
        );
    }
    #[test]
    fn binary_modes_partial_errors_and_null_empty_equality() {
        let mut original = VariableSpec::new(VARIABLE_NUMERIC);
        original.set_name(Some(&u("Timbre")));
        original.set_numeric_flags(true, true, false);
        let mut writer = BinaryWriter::default();
        original.save(&mut writer).unwrap();
        let mut read = VariableSpec::default();
        read.load(
            &mut BinaryCursor::new(writer.bytes()).unwrap(),
            &[],
            &mut Files::default(),
        )
        .unwrap();
        assert!(read.source_equals(&original));
        let mut bad = VariableSpec::new(VARIABLE_NUMERIC);
        bad.set_numeric_flags(true, false, true);
        let mut bytes = BinaryWriter::default();
        bad.save(&mut bytes).unwrap();
        assert!(matches!(
            read.load(
                &mut BinaryCursor::new(bytes.bytes()).unwrap(),
                &[],
                &mut Files::default()
            ),
            Err(VariableSpecError::ConflictingBounds)
        ));
        assert_eq!(read.numeric_flags(), [true, false, true]);
        let mut missing = Files {
            fail: true,
            ..Files::default()
        };
        let mut spec = VariableSpec::new(VARIABLE_SELECTION);
        spec.set_options(Some(&u("old")), &[], &mut Files::default())
            .unwrap();
        assert!(matches!(
            spec.set_options(Some(&u("x\\*.*")), &[], &mut missing),
            Err(VariableSpecError::Host("host"))
        ));
        assert_eq!(spec.options(), Some(u("x\\*.*").as_slice()));
        assert_eq!(spec.resolved_options().unwrap()[0].0, u("old"));
        let mut unpaired = VariableSpec::default();
        unpaired.set_name(Some(&[0xd800]));
        assert_eq!(unpaired.name(), Some([0xd800].as_slice()));
        let empty = VariableSpec::default();
        let mut not_null = VariableSpec::default();
        not_null.set_name(Some(&[]));
        assert!(!empty.source_equals(&not_null));
    }
    #[test]
    fn bundled_wizard_group_variable_payloads_read_real_legacy_bytes() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Wgroups/Wood/Rings.wtx"
        ));
        let mut reader = BinaryCursor::new(bytes).unwrap();
        reader.seek(2426).unwrap();
        let mut files = Files::default();
        for name in [
            "Compression",
            "Density",
            "Irregularity",
            "Blend",
            "Agitate",
            "Knot Strength",
        ] {
            assert_eq!(reader.read_string().unwrap(), u("Var"));
            let mut spec = VariableSpec::default();
            spec.load(&mut reader, &[], &mut files).unwrap();
            assert_eq!(spec.mode(), VARIABLE_NUMERIC);
            assert_eq!(spec.name(), Some(u(name).as_slice()));
            assert_eq!(spec.numeric_flags(), [false, false, false]);
            assert!(!reader.read_bool().unwrap());
        }
        assert_eq!(reader.read_string().unwrap(), u("Var"));
        let mut selection = VariableSpec::default();
        selection.load(&mut reader, &[], &mut files).unwrap();
        assert_eq!(selection.mode(), VARIABLE_SELECTION);
        assert_eq!(selection.name(), Some(u("Knot Density").as_slice()));
        assert_eq!(
            selection
                .resolved_options()
                .unwrap()
                .iter()
                .map(|(v, _)| v.clone())
                .collect::<Vec<_>>(),
            vec![u("Low"), u("Medium"), u("High"), u("Very High")]
        );
        assert!(!reader.read_bool().unwrap());
        for _ in 7..20 {
            assert_eq!(reader.read_string().unwrap(), u("Var"));
            let mut inactive = VariableSpec::default();
            inactive.load(&mut reader, &[], &mut files).unwrap();
            assert_eq!(inactive.mode(), VARIABLE_INACTIVE);
            assert!(!reader.read_bool().unwrap());
        }
        assert_eq!(reader.read_string().unwrap(), u("Ski"));
        assert!(files.seen.is_empty());
    }
}
