//! Detached raw storage copying for frame capture, distinct from source nz.
//! No public data getter, materialization, seal, constructor policy or callback.
use super::*;

pub(crate) struct RawImage {
    geometry: ImageGeometry,
    channels: Option<Channels>,
    region: Option<SharedFloatStore>,
    named: Option<Vec<Option<SharedNamedChannel>>>,
    anti_alias_type: i32,
    document_name: Option<Vec<u16>>,
    permanently_sealed: bool,
}
impl ImageMap {
    pub(crate) fn capture_fields(&self) -> RawImage {
        RawImage {
            geometry: self.geometry,
            channels: self.channels.as_ref().map(|c| Channels {
                constants: c.constants.clone(),
                planes: c.planes.clone(),
                materialized: c.materialized.clone(),
            }),
            region: self.region.clone(),
            named: self.named.clone(),
            anti_alias_type: self.anti_alias_type,
            document_name: self.document_name.clone(),
            permanently_sealed: self.permanently_sealed,
        }
    }
}
#[derive(Default)]
pub(crate) struct ImageCapture {
    share_backing: bool,
    buffers: Vec<(FloatBuffer, FloatBuffer)>,
    stores: Vec<(SharedFloatStore, SharedFloatStore)>,
    named: Vec<(SharedNamedChannel, SharedNamedChannel)>,
}
impl ImageCapture {
    pub(crate) fn share_image(old: RawImage) -> ImageMap {
        Self::sharing().image(old)
    }

    pub(crate) fn sharing() -> Self {
        Self {
            share_backing: true,
            ..Self::default()
        }
    }

    pub(crate) fn buffer(&mut self, old: &FloatBuffer) -> FloatBuffer {
        if let Some((_, copy)) = self.buffers.iter().find(|(v, _)| v.shares_storage(old)) {
            return copy.clone();
        }
        let copy = old.detached_copy(self.share_backing);
        self.buffers.push((old.clone(), copy.clone()));
        copy
    }
    fn store(&mut self, old: &SharedFloatStore) -> SharedFloatStore {
        if let Some((_, copy)) = self.stores.iter().find(|(v, _)| v.same_store(old)) {
            return copy.clone();
        }
        let (buffer, disposed, sealed, temporary_seal) = {
            let s = old.0.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.buffer.clone(), s.disposed, s.sealed, s.temporary_seal)
        };
        let buffer = self.buffer(&buffer);
        let copy = SharedFloatStore(Arc::new(FloatStoreCore {
            length: old.0.length,
            state: Mutex::new(FloatStoreState {
                buffer,
                disposed,
                sealed,
                temporary_seal,
            }),
        }));
        self.stores.push((old.clone(), copy.clone()));
        copy
    }
    fn named(&mut self, old: &SharedNamedChannel) -> SharedNamedChannel {
        if let Some((_, copy)) = self.named.iter().find(|(v, _)| Arc::ptr_eq(&v.0, &old.0)) {
            return copy.clone();
        }
        let data = old.0.data.as_ref().map(|v| self.store(v));
        let copy = SharedNamedChannel(Arc::new(NamedChannelCore {
            name: old.0.name.clone(),
            data,
        }));
        self.named.push((old.clone(), copy.clone()));
        copy
    }
    pub(crate) fn image(&mut self, old: RawImage) -> ImageMap {
        ImageMap {
            geometry: old.geometry,
            channels: old.channels.map(|c| Channels {
                constants: c
                    .constants
                    .iter()
                    .map(|v| v.as_ref().map(|v| self.buffer(v)))
                    .collect(),
                planes: c
                    .planes
                    .iter()
                    .map(|v| v.as_ref().map(|v| self.store(v)))
                    .collect(),
                materialized: c.materialized,
            }),
            region: old.region.as_ref().map(|v| self.store(v)),
            named: old.named.map(|v| {
                v.iter()
                    .map(|v| v.as_ref().map(|v| self.named(v)))
                    .collect()
            }),
            anti_alias_type: old.anti_alias_type,
            document_name: old.document_name,
            permanently_sealed: old.permanently_sealed,
        }
    }
}
#[derive(Default)]
pub(crate) struct ImageValues {
    buffers: Vec<FloatBuffer>,
    stores: Vec<SharedFloatStore>,
    named: Vec<SharedNamedChannel>,
}
impl ImageValues {
    pub(crate) fn buffer(&mut self, old: &FloatBuffer, out: &mut Vec<u64>) {
        let id = self
            .buffers
            .iter()
            .position(|v| v.shares_storage(old))
            .unwrap_or_else(|| {
                self.buffers.push(old.clone());
                self.buffers.len() - 1
            });
        let values = old.values();
        out.extend([id as u64, values.len() as u64]);
        out.extend(values.iter().map(|v| u64::from(v.to_bits())));
    }
    fn store(&mut self, old: &SharedFloatStore, out: &mut Vec<u64>) {
        let id = self
            .stores
            .iter()
            .position(|v| v.same_store(old))
            .unwrap_or_else(|| {
                self.stores.push(old.clone());
                self.stores.len() - 1
            });
        let (buffer, disposed, sealed, temporary) = {
            let s = old.0.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.buffer.clone(), s.disposed, s.sealed, s.temporary_seal)
        };
        out.extend([
            id as u64,
            old.0.length as u64,
            disposed as u64,
            sealed as u64,
            temporary as u64,
        ]);
        self.buffer(&buffer, out);
    }
    pub(crate) fn image(&mut self, old: RawImage, out: &mut Vec<u64>) {
        out.extend([
            old.geometry.width() as u64,
            old.geometry.height() as u64,
            old.geometry.origin()[0] as u64,
            old.geometry.origin()[1] as u64,
            old.anti_alias_type as u64,
            old.permanently_sealed as u64,
        ]);
        match old.document_name {
            None => out.push(0),
            Some(v) => {
                out.extend([1, v.len() as u64]);
                out.extend(v.into_iter().map(u64::from));
            }
        }
        match old.channels {
            None => out.push(0),
            Some(c) => {
                out.extend([1, c.constants.len() as u64]);
                for v in c.constants {
                    match v {
                        None => out.push(0),
                        Some(v) => {
                            out.push(1);
                            self.buffer(&v, out);
                        }
                    }
                }
                out.push(c.planes.len() as u64);
                for v in c.planes {
                    match v {
                        None => out.push(0),
                        Some(v) => {
                            out.push(1);
                            self.store(&v, out);
                        }
                    }
                }
                out.push(c.materialized.len() as u64);
                out.extend(c.materialized.into_iter().map(u64::from));
            }
        }
        match old.region {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                self.store(&v, out);
            }
        }
        match old.named {
            None => out.push(0),
            Some(v) => {
                out.extend([1, v.len() as u64]);
                for v in v {
                    match v {
                        None => out.push(0),
                        Some(v) => {
                            let id = self
                                .named
                                .iter()
                                .position(|n| Arc::ptr_eq(&n.0, &v.0))
                                .unwrap_or_else(|| {
                                    self.named.push(v.clone());
                                    self.named.len() - 1
                                });
                            out.extend([1, id as u64, v.0.name.len() as u64]);
                            out.extend(v.0.name.iter().copied().map(u64::from));
                            match &v.0.data {
                                None => out.push(0),
                                Some(v) => {
                                    out.push(1);
                                    self.store(v, out);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_image_capture_keeps_cross_map_store_float_region_named_aliases_and_flags() {
        let buffer = FloatBuffer::new(vec![f32::from_bits(0x80000000), f32::from_bits(0x7fc12345)]);
        let store = SharedFloatStore::new(buffer.clone()).unwrap();
        let mut a = ImageMap::new(ImageGeometry::new(2, 1));
        a.set_store(0, &store).unwrap();
        a.set_region_store(Some(store.clone()));
        a.add_named_store(Some(vec![65]), Some(store.clone()))
            .unwrap();
        let b = a.shallow_copy().unwrap();
        store.seal(true);
        let mut memo = ImageCapture::default();
        let ca = memo.image(a.capture_fields());
        let cb = memo.image(b.capture_fields());
        let sa = ca.region_store().unwrap();
        let sb = cb.region_store().unwrap();
        assert!(sa.same_store(&sb));
        assert!(!sa.same_store(&store));
        assert!(sa.is_sealed());
        assert!(sa.is_temporarily_sealed());
        assert!(!sa.buffer_observation().shares_storage(&buffer));
        assert!(
            ca.channel_observation().planes().unwrap()[0]
                .as_ref()
                .unwrap()
                .same_store(&sa)
        );
        assert!(
            ca.named_channels()[0]
                .as_ref()
                .unwrap()
                .data()
                .unwrap()
                .same_store(&sa)
        );
        buffer.write(0, 1.0).unwrap();
        assert_eq!(
            sa.buffer_observation().read(0).unwrap().to_bits(),
            0x80000000
        );
        store.dispose();
        assert!(!sa.is_disposed());
        assert_eq!(
            sa.buffer_observation().read(1).unwrap().to_bits(),
            0x7fc12345
        );
    }
    #[test]
    fn raw_capture_preserves_partial_materialization_and_failed_seal_without_effectful_read() {
        let mut partial = ImageMap::new(ImageGeometry::new(0, 1));
        assert!(partial.materialize(0).is_err());
        let mut memo = ImageCapture::default();
        let captured = memo.image(partial.capture_fields());
        let a = partial.channel_observation();
        let b = captured.channel_observation();
        assert_eq!(a.flags(), b.flags());
        assert_eq!(a.planes().unwrap().len(), b.planes().unwrap().len());
        assert!(b.planes().unwrap()[0].is_none());
        let mut image = ImageMap::new(ImageGeometry::new(1, 1));
        image.add_named_store(Some(vec![66]), None).unwrap();
        assert!(image.seal_permanently().is_err());
        let copy = memo.image(image.capture_fields());
        assert!(copy.is_permanently_sealed());
        assert!(copy.named_channels()[0].as_ref().unwrap().data().is_none());
        image.dispose();
        let disposed = memo.image(image.capture_fields());
        assert!(disposed.is_disposed());
        assert_eq!(
            disposed.is_permanently_sealed(),
            image.is_permanently_sealed()
        );
    }
}
