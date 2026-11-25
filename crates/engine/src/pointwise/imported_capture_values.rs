//! Raw Imported payload comparison operands; no lazy/provider/public data reads.
use super::*;

fn append_bytes(out: &mut Vec<u64>, bytes: &[u8]) {
    out.push(bytes.len() as u64);
    let (words, tail) = bytes.as_chunks::<8>();
    out.extend(words.iter().copied().map(u64::from_le_bytes));
    if !tail.is_empty() {
        let mut last = [0; 8];
        last[..tail.len()].copy_from_slice(tail);
        out.push(u64::from_le_bytes(last));
    }
}

impl<E: 'static> Comparison<E> {
    pub(super) fn imported_fields(
        &mut self,
        image: &crate::PointwiseImportedImage<E>,
        out: &mut Vec<u64>,
        seen: &mut Vec<PointwiseCustomObject<E>>,
        containers: &mut Vec<crate::ImportedImageContainer<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let object = image.custom_object();
        if seen.iter().any(|v| v.same_reference(&object)) {
            out.push(0);
            return Ok(());
        }
        seen.push(object);
        out.push(1);
        let (dimensions, valid, container) = image.capture_fields();
        out.extend([dimensions[0] as u64, dimensions[1] as u64, valid as u64]);
        let Some(container) = container else {
            out.push(0);
            return Ok(());
        };
        let id = self
            .imported_containers
            .iter()
            .position(|v| v.same_reference(&container))
            .unwrap_or_else(|| {
                self.imported_containers.push(container.clone());
                self.imported_containers.len() - 1
            });
        out.extend([1, id as u64]);
        if containers.iter().any(|v| v.same_reference(&container)) {
            out.push(0);
            return Ok(());
        }
        containers.push(container.clone());
        out.push(1);
        match container.store() {
            None => out.push(0),
            Some(store) => {
                let id = self
                    .encoded_stores
                    .iter()
                    .position(|v| v.same_reference(&store))
                    .unwrap_or_else(|| {
                        self.encoded_stores.push(store.clone());
                        self.encoded_stores.len() - 1
                    });
                let (raw, generation) = if self.memory_only {
                    store.frozen_snapshot()
                } else {
                    store.raw_snapshot().map(|raw| (raw, None))
                }
                .map_err(|e| {
                    PointwiseError::ImportedImage(crate::ImportedImageDecodeError::Storage(e))
                })?;
                out.extend([
                    1,
                    id as u64,
                    raw.length as u64,
                    raw.disposed as u64,
                    raw.sealed as u64,
                    raw.temporary as u64,
                    raw.stored as u64,
                    raw.has_file_name as u64,
                ]);
                for bytes in [raw.resident, raw.weak] {
                    match bytes {
                        None => out.push(0),
                        Some(bytes) => {
                            let id = self
                                .encoded_bytes
                                .iter()
                                .position(|v| v.same_reference(&bytes))
                                .unwrap_or_else(|| {
                                    self.encoded_bytes.push(bytes.clone());
                                    self.encoded_bytes.len() - 1
                                });
                            out.extend([1, id as u64]);
                            append_bytes(out, &bytes.generation());
                        }
                    }
                }
                if self.memory_only {
                    out.extend([
                        u64::from(generation.is_some()),
                        generation.unwrap_or_default(),
                    ]);
                }
                match raw.file_bytes {
                    None => out.push(0),
                    Some(bytes) => {
                        out.push(1);
                        append_bytes(out, &bytes);
                    }
                }
            }
        }
        match container.cached_image() {
            None => out.push(0),
            Some(map) => {
                let id = self
                    .images
                    .iter()
                    .position(|v| v.same_reference(&map))
                    .unwrap_or_else(|| {
                        self.images.push(map.clone());
                        self.images.len() - 1
                    });
                out.extend([1, id as u64]);
                self.image_values.image(map.capture_fields(), out);
            }
        }
        let receivers = container.dimension_listeners();
        out.push(receivers.len() as u64);
        for receiver in receivers {
            let id = self
                .objects
                .iter()
                .position(|v| v.same_reference(&receiver))
                .unwrap_or_else(|| {
                    self.objects.push(receiver.clone());
                    self.objects.len() - 1
                });
            out.push(id as u64);
            match receiver.imported_image() {
                Ok(image) => self.imported_fields(&image, out, seen, containers)?,
                Err(_) => out.push(2),
            }
        }
        Ok(())
    }
}
