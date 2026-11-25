//! The concrete seeded awm stream shared by Noise and preview dithering.
//!
//! This private arithmetic core has one owned state. It does not implement the
//! clock-seeded constructor, subclass dispatch or the other public RNG methods.

pub(crate) struct SeededRandom {
    words: [i32; 56],
    left: usize,
    right: usize,
}

#[derive(Debug)]
pub(crate) struct SeedOverflow;

impl SeededRandom {
    pub(crate) fn from_seed(seed: i32) -> Result<Self, SeedOverflow> {
        let magnitude = seed.checked_abs().ok_or(SeedOverflow)?;
        Ok(Self::from_magnitude(magnitude))
    }

    // Preview's original seeds are channel indices0..3. The byte type excludes
    // the only Math.Abs failure, so this constructor needs no unwrap/fallback.
    pub(crate) fn for_preview_channel(seed: u8) -> Self {
        Self::from_magnitude(i32::from(seed))
    }

    fn from_magnitude(magnitude: i32) -> Self {
        let mut words = [0i32; 56];
        let mut previous = 161_803_398i32.wrapping_sub(magnitude);
        words[55] = previous;
        let mut next = 1i32;
        for i in 1..55 {
            let slot = 21 * i % 55;
            words[slot] = next;
            next = previous.wrapping_sub(next);
            if next < 0 {
                next = next.wrapping_add(i32::MAX);
            }
            previous = words[slot];
        }
        for _ in 1..5 {
            for i in 1..56 {
                words[i] = words[i].wrapping_sub(words[1 + (i + 30) % 55]);
                if words[i] < 0 {
                    words[i] = words[i].wrapping_add(i32::MAX);
                }
            }
        }
        Self {
            words,
            left: 0,
            right: 21,
        }
    }

    pub(crate) fn sample(&mut self) -> f64 {
        self.left += 1;
        if self.left >= 56 {
            self.left = 1;
        }
        self.right += 1;
        if self.right >= 56 {
            self.right = 1;
        }
        let mut value = self.words[self.left].wrapping_sub(self.words[self.right]);
        if value < 0 {
            value = value.wrapping_add(i32::MAX);
        }
        self.words[self.left] = value;
        // Exact stored source double, multiplied after Int32 -> Double.
        f64::from(value) * f64::from_bits(0x3e00_0000_0020_0000)
    }

    #[cfg(test)]
    pub(crate) fn indices(&self) -> (usize, usize) {
        (self.left, self.right)
    }

    #[cfg(test)]
    pub(crate) fn state_words(&self) -> &[i32; 56] {
        &self.words
    }
}

#[cfg(test)]
mod original_seeded_controls {
    use super::SeededRandom;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::{
        fs::File,
        io::{BufRead, BufReader},
        path::Path,
    };

    #[test]
    fn accepted_three_public_seeded_controls_and_all_state_words() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let raw = root.join("port/evidence/curve-wire-native-live-gradient-nine-r1-attempt.jsonl");
        let input = root.join("port/evidence/live-gradient-nine-input-manifest.json");
        let gate = root
            .join("port/evidence/live-gradient-nine-independent-first-source-verification.json");
        for (path, want) in [
            (
                &raw,
                "86c19eecc1add60565c25d66663a277dee5dfa283e4deb92f47024b04485840b",
            ),
            (
                &input,
                "a68df75d3aa38a2e0010fc2d2977e751997435edbb25bb6fe8c3b4f0ee96f1f4",
            ),
            (
                &gate,
                "7084d6db007c98d1d1e49d17f38e93da37070a2819673b1db0fde1b92859b121",
            ),
        ] {
            assert_eq!(
                format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
                want
            );
        }
        let manifest: Value = serde_json::from_reader(File::open(input).unwrap()).unwrap();
        let mut cases = 0;
        let mut calls = 0;
        let mut errors = 0;
        let mut skips = 0;
        let mut values = 0;
        let mut fields = 0;
        let mut hashes = 0;
        let mut words = 0;
        for line in BufReader::new(File::open(raw).unwrap()).lines() {
            let row: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let Some(id) = row["id"]
                .as_str()
                .filter(|s| s.starts_with("actual-public-seeded-awm-control-"))
            else {
                continue;
            };
            assert_eq!(row["group"], "standalone-RNG-control-not-Gradient");
            let recipe = manifest["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap();
            let operations = recipe["operations"].as_array().unwrap();
            assert_eq!(operations.len(), 5);
            assert_eq!(row["steps"].as_array().unwrap().len(), 5);
            let seed = i32::try_from(operations[0]["arguments"][0].as_i64().unwrap()).unwrap();
            let mut result = SeededRandom::from_seed(seed);
            let owner_root = row["steps"][0]["outcome"]["value"]["root"].as_i64();
            let mut array_root = None;
            for (i, (op, step)) in operations
                .iter()
                .zip(row["steps"].as_array().unwrap())
                .enumerate()
            {
                assert_eq!(op["operation_id"], i);
                assert_eq!(step["operation_id"], i);
                assert_eq!(op["kind"], "call");
                assert_eq!(step["kind"], "call");
                assert_eq!(op["assembly"], "Beta");
                let start = step["call_start"].as_u64().unwrap() as usize;
                let end = step["call_end"].as_u64().unwrap() as usize;
                if i > 0 && result.is_err() {
                    assert_eq!(step["outcome"]["kind"], "skip");
                    assert_eq!(start, end);
                    assert_eq!(
                        step["outcome"]["missing"],
                        json!([{"name":"rng","producer_operation":0}])
                    );
                    assert!(step["state"]["held"].as_object().unwrap().is_empty());
                    assert_eq!(step["state"]["root_count"], 0);
                    assert_eq!(step["field_start"], step["field_end"]);
                    assert_eq!(step["observer_start"], step["observer_end"]);
                    skips += 1;
                    continue;
                }
                assert_eq!(end, start + 1);
                calls += 1;
                let call = &row["calls"][start];
                assert_eq!(call["assembly"], "Beta");
                assert_eq!(call["token"], op["token"]);
                assert_eq!(call["operation_id"], i);
                assert_eq!(call["reflection_invocation_attempted"], true);
                assert_eq!(call["outcome"], step["outcome"]);
                if i == 0 {
                    assert_eq!(op["token"], "06000012");
                    assert_eq!(
                        call["arguments"],
                        json!([{"type":"System.Int32","value":seed}])
                    );
                    assert!(call["target"]["type"].is_null());
                    if result.is_err() {
                        assert_eq!(step["outcome"]["kind"], "source_error");
                        assert_eq!(
                            step["outcome"]["error"]["chain"],
                            json!([
                                "System.Reflection.TargetInvocationException",
                                "System.OverflowException"
                            ])
                        );
                        assert_eq!(step["outcome"]["error"]["kind"], "error");
                        assert_eq!(step["outcome"]["error"]["truncated"], false);
                        assert!(step["state"]["held"].as_object().unwrap().is_empty());
                        assert_eq!(step["state"]["root_count"], 0);
                        assert_eq!(step["field_start"], step["field_end"]);
                        assert_eq!(step["observer_start"], step["observer_end"]);
                        errors += 1;
                        continue;
                    }
                    assert_eq!(step["outcome"]["kind"], "return");
                } else {
                    assert_eq!(op["token"], "0600001a");
                    assert!(call["arguments"].as_array().unwrap().is_empty());
                    assert_eq!(call["target"]["type"], "awm");
                    assert_eq!(call["target"]["root"].as_i64(), owner_root);
                    let value = result.as_mut().unwrap().sample();
                    assert_eq!(step["outcome"]["kind"], "return");
                    assert_eq!(
                        step["outcome"]["value"]["bits"],
                        format!("{:016x}", value.to_bits())
                    );
                    values += 1;
                }
                let stream = result.as_ref().unwrap();
                let data: Vec<u8> = stream
                    .state_words()
                    .iter()
                    .flat_map(|w| w.to_le_bytes())
                    .collect();
                let digest = format!("{:x}", Sha256::digest(&data));
                let field_start = step["field_start"].as_u64().unwrap() as usize;
                let field_end = step["field_end"].as_u64().unwrap() as usize;
                assert_eq!(field_end, field_start + 3);
                for field in &row["fields"].as_array().unwrap()[field_start..field_end] {
                    assert_eq!(field["assembly"], "Beta");
                    assert_eq!(field["target"]["root"].as_i64(), owner_root);
                    match field["token"].as_str().unwrap() {
                        "04000007" => assert_eq!(field["value"]["value"], stream.indices().0),
                        "04000008" => assert_eq!(field["value"]["value"], stream.indices().1),
                        "04000009" => {
                            let state = &field["value"];
                            let observed = state["root"].as_i64().unwrap();
                            if let Some(prior) = array_root {
                                assert_eq!(observed, prior);
                            } else {
                                array_root = Some(observed);
                            }
                            assert_eq!(state["type"], "System.Int32[]");
                            assert_eq!(state["rank"], 1);
                            assert_eq!(state["dimensions"], json!([56]));
                            assert_eq!(state["lower_bounds"], json!([0]));
                            assert_eq!(state["length"], 56);
                            assert_eq!(state["byte_length"], 224);
                            assert_eq!(state["sha256"], digest);
                            assert_eq!(state["complete_entries"], true);
                            let entries = state["entries"].as_array().unwrap();
                            assert_eq!(entries.len(), 56);
                            for (j, entry) in entries.iter().enumerate() {
                                assert_eq!(entry["index"], j);
                                assert_eq!(entry["value"]["value"], stream.state_words()[j]);
                                words += 1;
                            }
                        }
                        token => panic!("unapproved field{token}"),
                    }
                    fields += 1;
                }
                let probes =
                    &row["observers"].as_array().unwrap()[step["observer_start"].as_u64().unwrap()
                        as usize
                        ..step["observer_end"].as_u64().unwrap() as usize];
                assert_eq!(probes.len(), 1);
                let probe = &probes[0];
                assert_eq!(probe["kind"], "array-content-hash-and-entry-reads");
                assert_eq!(probe["target"]["root"].as_i64(), array_root);
                assert_eq!(probe["value"]["sha256"], digest);
                assert_eq!(probe["value"]["described_entry_reads"], 56);
                hashes += 1;
            }
            assert_eq!(
                row["calls"].as_array().unwrap().len(),
                if result.is_ok() { 5 } else { 1 }
            );
            cases += 1;
        }
        assert_eq!(
            (cases, calls, errors, skips, values, fields, hashes, words),
            (3, 11, 1, 4, 8, 30, 10, 560)
        );
        eprintln!(
            "Seeded controls:3cases/15positions/11calls/1source error/4skips/8exact values/30fields/10state hashes/560state words; hidden/default/subclass streams and general API remain unproved."
        );
    }
}
