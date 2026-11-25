//! Value prefix comparison for the characterized managed en-US runtime.
//!
//! This is CompareInfo.IsPrefix with CompareOptions.None on non-null UTF-16
//! values, not a general locale service or String reference-identity operation.
//! F052–F055 prove that this immutable profile has only single-unit, non-nesting
//! replacement contractions, no buffered sort keys, and no unsafe units.

mod data {
    include!("data/prefix_replacements.rs");
}
const TABLE: &[u8; 65536 * 6] = include_bytes!("data/prefix_en_us.bin");

#[derive(Clone, Copy)]
struct Weight {
    category: u8,
    primary: u8,
    secondary: u8,
    tertiary: u8,
    flags: u8,
    extender: u8,
}
impl Weight {
    fn of(unit: u16) -> Self {
        let i = usize::from(unit) * 6;
        Self {
            category: TABLE[i],
            primary: TABLE[i + 1],
            secondary: TABLE[i + 2],
            tertiary: TABLE[i + 3],
            flags: TABLE[i + 4],
            extender: TABLE[i + 5],
        }
    }
    fn special(self) -> bool {
        self.flags & 2 != 0
    }
    fn small(self) -> bool {
        self.flags & 8 != 0
    }
    fn hiragana(self) -> bool {
        self.flags & 16 != 0
    }
    fn half(self) -> bool {
        self.flags & 32 != 0
    }
    fn punctuation_key(self) -> i32 {
        i32::from(self.primary).wrapping_shl(8 + u32::from(self.tertiary))
    }
    fn key(self, extender: u8) -> [u8; 4] {
        let secondary = match extender {
            3 => 0, // Conditional
            4 => 5, // Buggy
            _ => self.secondary,
        };
        [self.category, self.primary, secondary, self.tertiary]
    }
}

struct Resume<'a> {
    text: &'a [u16],
    index: usize,
    checkpoint: usize,
}
struct Cursor<'a> {
    text: &'a [u16],
    index: usize,
    checkpoint: usize,
    previous: Option<u16>,
    resume: Option<Resume<'a>>,
}
impl<'a> Cursor<'a> {
    fn new(text: &'a [u16]) -> Self {
        let mut index = 0;
        while index < text.len() && Weight::of(text[index]).extender != 0 {
            index += 1;
        }
        Self {
            text,
            index,
            checkpoint: index,
            previous: None,
            resume: None,
        }
    }
    fn ended(&self) -> bool {
        self.index == self.text.len()
    }
    fn current(&self) -> u16 {
        self.text[self.index]
    }
    fn restore(&mut self) -> bool {
        if let Some(saved) = self.resume.take() {
            self.text = saved.text;
            self.index = saved.index;
            self.checkpoint = saved.checkpoint;
            true
        } else {
            false
        }
    }
    fn expand(&mut self, replacement: &'static [u16]) {
        // The captured closure proves no replacement can expand again while
        // this frame is occupied. Inputs and replacement text remain borrowed.
        self.resume = Some(Resume {
            text: self.text,
            index: self.index + 1,
            checkpoint: self.checkpoint,
        });
        self.text = replacement;
        self.index = 0;
        self.checkpoint = 0;
    }
    fn rewind(&mut self) {
        let previous_checkpoint = self.checkpoint;
        self.checkpoint = self.index;
        self.index -= 1;
        while self.index > previous_checkpoint && Weight::of(self.current()).category == 1 {
            self.index -= 1;
        }
        // Every effective unit is safe in this profile, so the source's second
        // unsafe-unit backtracking loop is unreachable (checked in the corpus).
    }
    fn punctuation_position(&self) -> i32 {
        self.resume.as_ref().map_or(self.index, |saved| saved.index) as i32
    }
    fn combine(&mut self, key: &mut [u8; 4]) {
        while !self.ended() && Weight::of(self.current()).category == 1 {
            if key[2] == 0 {
                key[2] = 2;
            }
            key[2] = key[2].wrapping_add(Weight::of(self.current()).secondary);
            self.index += 1;
        }
    }
}

fn replacement(unit: u16) -> Option<&'static [u16]> {
    data::REPLACEMENTS
        .binary_search_by_key(&unit, |(source, _)| *source)
        .ok()
        .map(|index| data::REPLACEMENTS[index].1)
}
fn extender_code(previous: u16, extender: u8) -> u16 {
    let weight = Weight::of(previous);
    if extender == 3 && weight.special() {
        let vowel = weight.primary & 7;
        if (2..=6).contains(&vowel) {
            if weight.half() {
                return 65393 + u16::from(vowel - 2);
            }
            return if weight.hiragana() { 12354 } else { 12450 } + 2 * u16::from(vowel - 2);
        }
    }
    previous
}
fn dash(extender: u8) -> u8 {
    match extender {
        0 => 3,
        3 => 5,
        _ => 4,
    }
}

/// Original en-US value prefix matching over CLR-representable UTF-16 strings.
///
/// Keeps ignorable units, contextual extenders and replacement/fast-path order
/// without normalizing input. Other cultures/options, native CLR and source
/// reference-identity shortcuts are separate, unverified capabilities.
pub fn property_prefix_en_us(source: &[u16], prefix: &[u16]) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if source.is_empty() {
        return false;
    }
    let mut source = Cursor::new(source);
    let mut target = Cursor::new(prefix);
    let mut punctuation_positions = [-1_i32; 2];
    let mut punctuation_keys = [0_i32; 2];
    let mut track_punctuation = true;
    loop {
        if !source.ended() && Weight::of(source.current()).flags & 1 != 0 {
            source.index += 1;
            continue;
        }
        while !target.ended() && Weight::of(target.current()).flags & 1 != 0 {
            target.index += 1;
        }
        // Preserve this order: source exhaustion can consume a replacement
        // target segment before its original suffix is restored.
        if source.ended() {
            if source.restore() {
                continue;
            }
            break;
        }
        if target.ended() {
            if target.restore() {
                continue;
            }
            break;
        }
        if source.checkpoint < source.index && target.checkpoint < target.index {
            while !source.ended() && !target.ended() && source.current() == target.current() {
                source.index += 1;
                target.index += 1;
            }
            if source.ended() || target.ended() {
                continue;
            }
            source.rewind();
            target.rewind();
        }
        let mut source_code = source.current();
        let mut target_code = target.current();
        let source_extender = Weight::of(source_code).extender;
        if source_extender != 0 {
            let Some(previous) = source.previous else {
                source.index += 1;
                continue;
            };
            source_code = extender_code(previous, source_extender);
        }
        let target_extender = Weight::of(target_code).extender;
        if target_extender != 0 {
            let Some(previous) = target.previous else {
                target.index += 1;
                continue;
            };
            target_code = extender_code(previous, target_extender);
        }
        let source_weight = Weight::of(source_code);
        let target_weight = Weight::of(target_code);
        if source_weight.category == 6 {
            if track_punctuation {
                punctuation_positions[0] = source.punctuation_position();
                punctuation_keys[0] = source_weight.punctuation_key();
            }
            source.previous = Some(source_code);
            source.index += 1;
        }
        if target_weight.category == 6 {
            if track_punctuation {
                punctuation_positions[1] = target.punctuation_position();
                punctuation_keys[1] = target_weight.punctuation_key();
            }
            target.previous = Some(target_code);
            target.index += 1;
        }
        if source_weight.category == 6 || target_weight.category == 6 {
            if track_punctuation {
                if punctuation_keys[0] == punctuation_keys[1] {
                    punctuation_positions = [-1; 2];
                    punctuation_keys = [0; 2];
                } else {
                    track_punctuation = false;
                }
            }
            continue;
        }
        if source_extender == 0
            && let Some(text) = replacement(source.current())
        {
            source.expand(text);
            continue;
        }
        let mut source_key = source_weight.key(source_extender);
        if source_weight.category > 1 {
            source.previous = Some(source_code);
        }
        if target_extender == 0
            && let Some(text) = replacement(target.current())
        {
            target.expand(text);
            continue;
        }
        let mut target_key = target_weight.key(target_extender);
        if target_weight.category > 1 {
            target.previous = Some(target_code);
        }
        source.index += 1;
        target.index += 1;
        source.combine(&mut source_key);
        target.combine(&mut target_key);
        if source_key != target_key || source_weight.special() != target_weight.special() {
            return false;
        }
        if source_weight.special()
            && (source_weight.small() != target_weight.small()
                || dash(source_extender) != dash(target_extender)
                || source_weight.hiragana() != target_weight.hiragana()
                || source_weight.half() != target_weight.half())
        {
            return false;
        }
    }
    target.ended()
        && punctuation_positions[0] == punctuation_positions[1]
        && punctuation_keys[0] == punctuation_keys[1]
}

#[cfg(test)]
mod test {
    use super::extender_code;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::{
        collections::HashSet,
        fs::File,
        io::{BufRead, BufReader, Read},
        path::Path,
    };
    #[test]
    fn original_extender_mapping_values() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../port/evidence/property-prefix-original.jsonl");
        let mut file = File::open(&path).expect("source corpus");
        let mut hash = Sha256::new();
        let mut buffer = [0; 8192];
        loop {
            let count = file.read(&mut buffer).expect("input");
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        assert_eq!(
            format!("{:x}", hash.finalize()),
            "6e8e0310983fcb7a38e1ee325cdb4ba65c63b4da598170c0c2f21f8fea53ffb3"
        );
        let mut seen = HashSet::new();
        let mut cases = 0;
        let mut conditional_changes = 0;
        for line in BufReader::new(File::open(path).expect("corpus")).lines() {
            let row: Value = serde_json::from_str(&line.expect("line")).expect("JSON");
            if row["kind"] != "helper" {
                continue;
            }
            assert_eq!(row["name"], "filter-extender");
            assert_eq!(row["count"], 65536);
            assert_eq!(row["entry_width"], 2);
            let kind = u8::try_from(row["extender"].as_u64().expect("kind")).expect("extender");
            assert!(seen.insert(kind));
            let s = row["payload"].as_str().expect("hex");
            assert_eq!(s.len(), 65536 * 4);
            for cp in 0..=u16::MAX {
                let at = usize::from(cp) * 4;
                let bytes = [
                    u8::from_str_radix(&s[at..at + 2], 16).expect("byte"),
                    u8::from_str_radix(&s[at + 2..at + 4], 16).expect("byte"),
                ];
                let expected = u16::from_le_bytes(bytes);
                if kind == 3 && expected != cp {
                    conditional_changes += 1;
                }
                assert_eq!(extender_code(cp, kind), expected, "cp{cp:04x} kind{kind}");
                cases += 1;
            }
        }
        assert_eq!(seen, HashSet::from([0, 1, 2, 3, 4]));
        assert_eq!(cases, 327680);
        assert_eq!(conditional_changes, 258);
        println!("Extender mapping: {cases} original values / zero differences");
    }
}
