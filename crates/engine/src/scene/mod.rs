//! Retained CPU scene and indexed mesh owners from nt/c1/afs. Rendering remains
//! a separate capability; no GPU object name or ImageMap stands in for a mesh.
mod array;
mod combine;
pub(crate) mod import;
mod list;
mod math;
mod primitive;
use crate::pointwise::SharedImageMap;
pub use array::{
    MeshArray, MeshArrayElement, MeshBuffer, MeshIndexBuffer, MeshIndexStore, MeshUvBuffer,
    MeshUvStore, MeshVectorBuffer, MeshVectorStore,
};
pub use combine::combine_3d;
pub use list::{SceneList, SceneListHandler};
pub use math::{MeshUv, MeshVector, SceneTransform, SceneVector};
pub use primitive::{ScenePrimitiveKind, ScenePrimitiveParameters, generate_primitive};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneError {
    Allocation,
    NegativeCount,
    NullReference,
    Disposed,
    IndexOutOfRange,
    ZeroLengthArray,
    Arithmetic,
    MissingRenderer,
    MissingAllocationServices,
    MissingArrayStorageServices,
    InvalidProperty,
    UnsupportedAdjustment,
    CollectionModified,
}
impl SceneError {
    pub const fn is_unsupported_capability(self) -> bool {
        matches!(
            self,
            Self::MissingRenderer
                | Self::MissingAllocationServices
                | Self::MissingArrayStorageServices
                | Self::UnsupportedAdjustment
        )
    }
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Allocation => "OutOfMemoryException",
            Self::NegativeCount => "OverflowException",
            Self::NullReference => "NullReferenceException",
            Self::Disposed => "ObjectDisposedException",
            Self::IndexOutOfRange => "IndexOutOfRangeException",
            Self::ZeroLengthArray | Self::InvalidProperty => "ArgumentException",
            Self::Arithmetic => "ArithmeticException",
            Self::CollectionModified => "InvalidOperationException",
            Self::MissingRenderer
            | Self::MissingAllocationServices
            | Self::MissingArrayStorageServices
            | Self::UnsupportedAdjustment => "UnsupportedCapability",
        }
    }
}
impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "scene: {self:?}")
    }
}
impl std::error::Error for SceneError {}
/// Source SafelyCreateArray pressure site. The caller supplies its actual
/// memory/storage admission service; this is not an original count limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneArrayKind {
    IndexI32,
    Vector3F,
    UvF32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneAllocationRequest {
    elements: usize,
    bytes: usize,
    kind: SceneArrayKind,
}
impl SceneAllocationRequest {
    pub const fn element_count(self) -> usize {
        self.elements
    }
    pub const fn byte_count(self) -> usize {
        self.bytes
    }
    pub const fn element_kind(self) -> SceneArrayKind {
        self.kind
    }
}
pub struct SceneAllocationEffects<'a> {
    preflight: &'a mut dyn FnMut(SceneAllocationRequest) -> Result<(), SceneError>,
}
impl<'a> SceneAllocationEffects<'a> {
    pub fn new(
        preflight: &'a mut dyn FnMut(SceneAllocationRequest) -> Result<(), SceneError>,
    ) -> Self {
        Self { preflight }
    }
    pub(super) fn create<T>(
        &mut self,
        count: usize,
        kind: SceneArrayKind,
    ) -> Result<(), SceneError> {
        let bytes = count
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(SceneError::Allocation)?;
        (self.preflight)(SceneAllocationRequest {
            elements: count,
            bytes,
            kind,
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolygonTopology {
    Triangles,
    Quads,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshAttribute {
    Position,
    Uv,
    Normal,
    Tangent,
}
/// Public `qc` keys currently supported by a polygon group's index dictionary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum PolygonIndexKey {
    Position = 1,
    Uv = 2,
    Normal = 3,
    Tangent = 4,
    Face = 5,
}

/// A live view of a group's keys. Iteration captures the dictionary version
/// when it starts, just as the source dictionary's key enumerator does.
#[derive(Clone, Debug)]
pub struct PolygonIndexKeys(PolygonGroup);

pub struct PolygonIndexKeysIter {
    group: PolygonGroup,
    version: u32,
    next_slot: usize,
    finished: bool,
}

impl PolygonIndexKeys {
    pub fn len(&self) -> usize {
        let state = guard(&self.0.0);
        state.entries.len() - state.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> PolygonIndexKeysIter {
        PolygonIndexKeysIter {
            group: self.0.clone(),
            version: guard(&self.0.0).version,
            next_slot: 0,
            finished: false,
        }
    }
}

impl Iterator for PolygonIndexKeysIter {
    type Item = Result<PolygonIndexKey, SceneError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let state = guard(&self.group.0);
        if state.version != self.version {
            self.finished = true;
            return Some(Err(SceneError::CollectionModified));
        }
        while let Some(entry) = state.entries.get(self.next_slot) {
            self.next_slot += 1;
            if let Some(key) = entry {
                return Some(Ok(match key {
                    0 => PolygonIndexKey::Position,
                    1 => PolygonIndexKey::Uv,
                    2 => PolygonIndexKey::Normal,
                    3 => PolygonIndexKey::Tangent,
                    4 => PolygonIndexKey::Face,
                    _ => unreachable!("polygon dictionary key"),
                }));
            }
        }
        self.finished = true;
        None
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshSeal {
    Open,
    Temporary,
    Permanent,
}
type Array<T> = MeshArray<T>;
// Dictionary entry slots preserve this finite five-channel source dictionary's
// occupied order and LIFO reuse after removal. Values are the actual owners.
struct PolygonData {
    signed_count: i32,
    topology: PolygonTopology,
    indices: [Option<MeshIndexStore>; 4],
    face_indices: Option<MeshIndexStore>,
    smooth: f64,
    entries: Vec<Option<usize>>,
    free: Vec<usize>,
    version: u32,
}
impl PolygonData {
    fn store(&self, key: usize) -> Option<MeshIndexStore> {
        if key == 4 {
            self.face_indices.clone()
        } else {
            self.indices[key].clone()
        }
    }
    fn install(&mut self, key: usize, value: Option<MeshIndexStore>) {
        let present = self.store(key).is_some();
        let incoming = value.is_some();
        if incoming && !present {
            if let Some(ix) = self.free.pop() {
                self.entries[ix] = Some(key);
            } else {
                self.entries.push(Some(key));
            }
        }
        if !incoming && present {
            let ix = self
                .entries
                .iter()
                .position(|v| *v == Some(key))
                .expect("actual channel entry");
            self.entries[ix] = None;
            self.free.push(ix);
        }
        if incoming || present {
            self.version = self.version.wrapping_add(1);
        }
        if key == 4 {
            self.face_indices = value;
        } else {
            self.indices[key] = value;
        }
    }
}
/// Actual afs owner with signed count and nullable independently retained stores.
pub struct PolygonGroup(Arc<Mutex<PolygonData>>);
impl Clone for PolygonGroup {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl fmt::Debug for PolygonGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PolygonGroup({:p})", Arc::as_ptr(&self.0))
    }
}
impl PolygonGroup {
    /// Return the live key collection without reading or materializing buffers.
    pub fn index_keys(&self) -> PolygonIndexKeys {
        PolygonIndexKeys(self.clone())
    }
    /// Source count/flag constructor. Position is always allocated for positive
    /// count; flags choose UV, normal and tangent in that actual source order.
    pub fn with_attributes(
        topology: PolygonTopology,
        count: i32,
        attributes: [bool; 3],
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Self, SceneError> {
        let mut indices = std::array::from_fn(|_| None);
        let mut entries = Vec::new();
        if count > 0 {
            for (ix, enabled) in [true, attributes[0], attributes[1], attributes[2]]
                .into_iter()
                .enumerate()
            {
                if enabled {
                    effects.create::<i32>(count as usize, SceneArrayKind::IndexI32)?;
                    let mut values = Vec::new();
                    values
                        .try_reserve_exact(count as usize)
                        .map_err(|_| SceneError::Allocation)?;
                    values.resize(count as usize, 0);
                    indices[ix] = Some(Array::from_values(values, MeshSeal::Open)?);
                    entries.push(Some(ix));
                }
            }
        }
        Ok(Self(Arc::new(Mutex::new(PolygonData {
            signed_count: count,
            topology,
            indices,
            face_indices: None,
            smooth: 1.,
            entries,
            free: Vec::new(),
            version: 0,
        }))))
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.0, &b.0)
    }
    pub fn topology(&self) -> PolygonTopology {
        guard(&self.0).topology
    }
    pub fn indices(&self, a: MeshAttribute) -> Option<MeshIndexStore> {
        guard(&self.0).indices[a as usize].clone()
    }
    /// afs.a(qc,StorableArray): validate length before changing the dictionary.
    pub fn set_indices(
        &self,
        a: MeshAttribute,
        store: Option<&MeshIndexStore>,
    ) -> Result<(), SceneError> {
        self.set_store(a as usize, store.cloned())
    }
    fn set_store(&self, key: usize, store: Option<MeshIndexStore>) -> Result<(), SceneError> {
        let mut state = guard(&self.0);
        if store
            .as_ref()
            .is_some_and(|v| v.array_length() != state.signed_count)
        {
            return Err(SceneError::InvalidProperty);
        }
        state.install(key, store);
        Ok(())
    }
    pub fn dispose_indices(&self, a: MeshAttribute) {
        if let Some(store) = self.indices(a) {
            store.seal(false);
            store.dispose();
            guard(&self.0).install(a as usize, None);
        }
    }
    /// Readonly nonauthoritative upload snapshot; absent receiver is a null cause.
    pub fn try_indices(&self, a: MeshAttribute) -> Result<Vec<i32>, SceneError> {
        self.indices(a)
            .ok_or(SceneError::NullReference)?
            .try_snapshot()
    }
    pub fn index_len(&self, a: MeshAttribute) -> usize {
        self.indices(a).map_or(0, |a| a.len())
    }
    pub fn seal(&self, a: MeshAttribute) -> Option<MeshSeal> {
        self.indices(a).map(|a| a.seal_state())
    }
    pub fn has_attribute(&self, a: MeshAttribute) -> bool {
        self.indices(a).is_some()
    }
    pub fn signed_corner_count(&self) -> i32 {
        guard(&self.0).signed_count
    }
    pub fn corner_count(&self) -> usize {
        self.index_len(MeshAttribute::Position)
    }
    pub fn smooth_angle_cosine(&self) -> f64 {
        guard(&self.0).smooth
    }
    pub fn set_smooth_angle_cosine(&self, value: f64) {
        guard(&self.0).smooth = value;
    }
    pub fn retained_face_indices(&self) -> Option<MeshIndexStore> {
        guard(&self.0).face_indices.clone()
    }
    /// Actual GetFaceIndices creates its signed-count array before store wrapping,
    /// then returns the actual mutable underlying array of the cached store.
    pub fn face_indices(&self) -> Result<MeshIndexBuffer, SceneError> {
        if self.retained_face_indices().is_none() {
            let n = usize::try_from(self.signed_corner_count())
                .map_err(|_| SceneError::NegativeCount)?;
            let mut values = Vec::new();
            values
                .try_reserve_exact(n)
                .map_err(|_| SceneError::Allocation)?;
            values.extend((0..n).map(|i| i as i32));
            let store = Array::from_values(values, MeshSeal::Open)?;
            self.set_store(4, Some(store))?;
        }
        self.retained_face_indices()
            .ok_or(SceneError::NullReference)?
            .data(false)
    }
    pub fn triangle_corners(&self) -> Result<Vec<i32>, SceneError> {
        let (topology, n) = {
            let s = guard(&self.0);
            (s.topology, s.signed_count)
        };
        let count = match topology {
            PolygonTopology::Triangles => n,
            PolygonTopology::Quads => (n / 4).wrapping_mul(6),
        };
        let mut values = Vec::new();
        let count = usize::try_from(count).map_err(|_| SceneError::NegativeCount)?;
        values
            .try_reserve_exact(count)
            .map_err(|_| SceneError::Allocation)?;
        values.resize(count, 0);
        match topology {
            PolygonTopology::Triangles => {
                for (i, v) in values.iter_mut().enumerate() {
                    *v = i as i32;
                }
            }
            PolygonTopology::Quads => {
                let mut input = 0_i32;
                let mut output = 0_usize;
                while input < n {
                    for delta in [0_i32, 1, 2, 0, 2, 3] {
                        *values.get_mut(output).ok_or(SceneError::IndexOutOfRange)? =
                            input.wrapping_add(delta);
                        output += 1;
                    }
                    input = input.wrapping_add(4);
                }
            }
        }
        Ok(values)
    }
    pub fn source_copy(&self) -> Result<Self, SceneError> {
        let mut unavailable = |_| Err(SceneError::MissingAllocationServices);
        self.source_copy_with_effects(&mut SceneAllocationEffects::new(&mut unavailable))
    }
    pub fn source_copy_with_effects(
        &self,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Self, SceneError> {
        let (count, topology, smooth, version, keys) = {
            let s = guard(&self.0);
            (
                s.signed_count,
                s.topology,
                s.smooth,
                s.version,
                s.entries.iter().flatten().copied().collect::<Vec<_>>(),
            )
        };
        let copy = Self(Arc::new(Mutex::new(PolygonData {
            signed_count: count,
            topology,
            indices: std::array::from_fn(|_| None),
            face_indices: None,
            smooth,
            entries: Vec::new(),
            free: Vec::new(),
            version: 0,
        })));
        for key in keys {
            let store = {
                let s = guard(&self.0);
                if s.version != version {
                    return Err(SceneError::CollectionModified);
                }
                s.store(key).ok_or(SceneError::CollectionModified)?
            };
            let array = store.source_copy_with_effects(effects)?;
            guard(&copy.0).install(key, Some(array));
            if guard(&self.0).version != version {
                return Err(SceneError::CollectionModified);
            }
        }
        Ok(copy)
    }
}
struct MaterialData {
    ambient: [f32; 4],
    diffuse: [f32; 4],
    specular: [f32; 4],
    shininess: f32,
    reflectivity: f32,
    refractivity: f32,
    refraction_index: f32,
    transparency: f32,
    shadow_strength: f64,
    shadow_samples: i32,
    shadows: bool,
    environment_enabled: bool,
    ambient_enabled: bool,
    emissive: bool,
    max_map_parts: [i32; 2],
    map_parts: [i32; 2],
    fresnel: bool,
    depth_peeling: bool,
    processed_lights: i32,
    map: Option<SharedImageMap>,
}
/// Actual w8/abx configuration and retained map, independent from GPU programs.
pub struct SceneMaterial(Arc<Mutex<MaterialData>>);
impl Clone for SceneMaterial {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl fmt::Debug for SceneMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SceneMaterial({:p})", Arc::as_ptr(&self.0))
    }
}
impl Default for SceneMaterial {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(MaterialData {
            ambient: [1.; 4],
            diffuse: [1.; 4],
            specular: [1.; 4],
            shininess: 25.,
            reflectivity: 0.,
            refractivity: 0.,
            refraction_index: 0.75,
            transparency: 1.,
            shadow_strength: 0.,
            shadow_samples: 4,
            shadows: false,
            environment_enabled: false,
            ambient_enabled: false,
            emissive: false,
            max_map_parts: [2; 2],
            map_parts: [1; 2],
            fresnel: false,
            depth_peeling: true,
            processed_lights: 2,
            map: None,
        })))
    }
}
impl SceneMaterial {
    pub fn same_reference(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.0, &b.0)
    }
    pub fn ambient(&self) -> [f32; 3] {
        let s = guard(&self.0);
        [s.ambient[0], s.ambient[1], s.ambient[2]]
    }
    pub fn ambient_rgba(&self) -> [f32; 4] {
        guard(&self.0).ambient
    }
    pub fn diffuse(&self) -> [f32; 3] {
        let s = guard(&self.0);
        [s.diffuse[0], s.diffuse[1], s.diffuse[2]]
    }
    pub fn diffuse_rgba(&self) -> [f32; 4] {
        guard(&self.0).diffuse
    }
    pub fn specular(&self) -> [f32; 3] {
        let s = guard(&self.0);
        [s.specular[0], s.specular[1], s.specular[2]]
    }
    pub fn specular_rgba(&self) -> [f32; 4] {
        guard(&self.0).specular
    }
    pub fn shininess(&self) -> f32 {
        guard(&self.0).shininess
    }
    pub fn is_depth_peeling_enabled(&self) -> bool {
        guard(&self.0).depth_peeling
    }
    pub fn processed_lights(&self) -> i32 {
        guard(&self.0).processed_lights
    }
    pub(crate) fn replace_color_map(
        &self,
        image: &SharedImageMap,
        max_texture_size: u32,
    ) -> Result<(), SceneError> {
        let geometry = image.geometry();
        let width = u32::try_from(geometry.width()).map_err(|_| SceneError::InvalidProperty)?;
        let height = u32::try_from(geometry.height()).map_err(|_| SceneError::InvalidProperty)?;
        if width == 0 || height == 0 || max_texture_size == 0 {
            return Err(SceneError::InvalidProperty);
        }
        if width > max_texture_size || height > max_texture_size {
            return Err(SceneError::UnsupportedAdjustment);
        }
        let mut state = guard(&self.0);
        state.map_parts = [1, 1];
        state.map = Some(image.clone());
        Ok(())
    }
    pub fn map(&self) -> Option<SharedImageMap> {
        guard(&self.0).map.clone()
    }
    pub fn reflectivity(&self) -> f32 {
        guard(&self.0).reflectivity
    }
    pub fn refractivity(&self) -> f32 {
        guard(&self.0).refractivity
    }
    pub fn refraction_index(&self) -> f32 {
        guard(&self.0).refraction_index
    }
    pub fn transparency(&self) -> f32 {
        guard(&self.0).transparency
    }
    pub fn has_fresnel_effect(&self) -> bool {
        guard(&self.0).fresnel
    }
    pub fn shadow_strength(&self) -> f64 {
        guard(&self.0).shadow_strength
    }
    pub fn shadow_samples(&self) -> i32 {
        guard(&self.0).shadow_samples
    }
    pub fn are_shadows_enabled(&self) -> bool {
        guard(&self.0).shadows
    }
    pub fn is_environment_enabled(&self) -> bool {
        guard(&self.0).environment_enabled
    }
    pub fn is_ambient_enabled(&self) -> bool {
        guard(&self.0).ambient_enabled
    }
    pub fn is_emissive_enabled(&self) -> bool {
        guard(&self.0).emissive
    }
    pub fn map_parts(&self) -> [i32; 2] {
        guard(&self.0).map_parts
    }
    pub fn maximum_map_parts(&self) -> [i32; 2] {
        guard(&self.0).max_map_parts
    }
    pub fn source_copy(&self) -> Self {
        let s = guard(&self.0);
        Self(Arc::new(Mutex::new(MaterialData {
            ambient: s.ambient,
            diffuse: s.diffuse,
            specular: s.specular,
            shininess: s.shininess,
            reflectivity: s.reflectivity,
            refractivity: s.refractivity,
            refraction_index: s.refraction_index,
            transparency: s.transparency,
            shadow_strength: s.shadow_strength,
            shadow_samples: s.shadow_samples,
            shadows: s.shadows,
            environment_enabled: s.environment_enabled,
            ambient_enabled: s.ambient_enabled,
            emissive: s.emissive,
            max_map_parts: s.max_map_parts,
            map_parts: s.map_parts,
            fresnel: s.fresnel,
            depth_peeling: s.depth_peeling,
            processed_lights: s.processed_lights,
            map: s.map.clone(),
        })))
    }
}
struct ModelData {
    positions: Option<MeshVectorStore>,
    uvs: Option<MeshUvStore>,
    normals: Option<MeshVectorStore>,
    tangents: Option<MeshVectorStore>,
    groups: Vec<PolygonGroup>,
    material: SceneMaterial,
}
pub struct MeshModel(Arc<Mutex<ModelData>>);
impl Clone for MeshModel {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl fmt::Debug for MeshModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshModel({:p})", Arc::as_ptr(&self.0))
    }
}
impl MeshModel {
    /// Composes source c1() with its actual material setter. The provided material
    /// is retained, and all four source array fields start genuinely null.
    pub fn empty_with_material(material: SceneMaterial) -> Self {
        Self(Arc::new(Mutex::new(ModelData {
            positions: None,
            uvs: None,
            normals: None,
            tangents: None,
            groups: Vec::new(),
            material,
        })))
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.0, &b.0)
    }
    pub fn positions(&self) -> Option<MeshVectorStore> {
        guard(&self.0).positions.clone()
    }
    pub fn uvs(&self) -> Option<MeshUvStore> {
        guard(&self.0).uvs.clone()
    }
    pub fn normals(&self) -> Option<MeshVectorStore> {
        guard(&self.0).normals.clone()
    }
    pub fn tangents(&self) -> Option<MeshVectorStore> {
        guard(&self.0).tangents.clone()
    }
    pub fn set_positions(&self, array: Option<&MeshVectorStore>) {
        let old = std::mem::replace(&mut guard(&self.0).positions, array.cloned());
        drop(old);
    }
    pub fn set_uvs(&self, array: Option<&MeshUvStore>) {
        let old = std::mem::replace(&mut guard(&self.0).uvs, array.cloned());
        drop(old);
    }
    pub fn set_normals(&self, array: Option<&MeshVectorStore>) {
        let old = std::mem::replace(&mut guard(&self.0).normals, array.cloned());
        drop(old);
    }
    pub fn set_tangents(&self, array: Option<&MeshVectorStore>) {
        let old = std::mem::replace(&mut guard(&self.0).tangents, array.cloned());
        drop(old);
    }
    pub fn try_positions(&self) -> Result<Vec<MeshVector>, SceneError> {
        self.positions()
            .ok_or(SceneError::NullReference)?
            .try_snapshot()
    }
    pub fn try_uvs(&self) -> Result<Vec<MeshUv>, SceneError> {
        self.uvs().ok_or(SceneError::NullReference)?.try_snapshot()
    }
    pub fn try_normals(&self) -> Result<Vec<MeshVector>, SceneError> {
        self.normals()
            .ok_or(SceneError::NullReference)?
            .try_snapshot()
    }
    pub fn try_tangents(&self) -> Result<Vec<MeshVector>, SceneError> {
        self.tangents()
            .ok_or(SceneError::NullReference)?
            .try_snapshot()
    }
    pub fn attribute_len(&self, a: MeshAttribute) -> usize {
        match a {
            MeshAttribute::Position => self.positions().map_or(0, |a| a.len()),
            MeshAttribute::Uv => self.uvs().map_or(0, |a| a.len()),
            MeshAttribute::Normal => self.normals().map_or(0, |a| a.len()),
            MeshAttribute::Tangent => self.tangents().map_or(0, |a| a.len()),
        }
    }
    pub fn attribute_seal(&self, a: MeshAttribute) -> Option<MeshSeal> {
        match a {
            MeshAttribute::Position => self.positions().map(|a| a.seal_state()),
            MeshAttribute::Uv => self.uvs().map(|a| a.seal_state()),
            MeshAttribute::Normal => self.normals().map(|a| a.seal_state()),
            MeshAttribute::Tangent => self.tangents().map(|a| a.seal_state()),
        }
    }
    pub fn groups(&self) -> Vec<PolygonGroup> {
        guard(&self.0).groups.clone()
    }
    pub fn material(&self) -> SceneMaterial {
        guard(&self.0).material.clone()
    }
    pub fn transform(&self, t: SceneTransform) -> Result<(), SceneError> {
        let positions = self.positions().ok_or(SceneError::NullReference)?;
        let buffer = positions.data(false)?;
        buffer.update(|p| t.point(p));
        positions.seal(true);
        if let Some(array) = self.normals() {
            let buffer = array.data(false)?;
            buffer.update(|p| t.direction(p));
            array.seal(true);
        }
        if let Some(array) = self.tangents() {
            let buffer = array.data(false)?;
            buffer.update(|p| t.direction(p));
            array.seal(true);
        }
        Ok(())
    }
    pub fn source_copy(&self) -> Result<Self, SceneError> {
        let mut unavailable = |_| Err(SceneError::MissingAllocationServices);
        self.source_copy_with_effects(&mut SceneAllocationEffects::new(&mut unavailable))
    }
    pub fn source_copy_with_effects(
        &self,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Self, SceneError> {
        // Match c1(c1): retrieve each actual field at its reached copy operation,
        // allowing allocation callbacks to replace later source fields.
        let positions = Some(
            self.positions()
                .ok_or(SceneError::NullReference)?
                .source_copy_with_effects(effects)?,
        );
        let uvs = self
            .uvs()
            .map(|a| a.source_copy_with_effects(effects))
            .transpose()?;
        let normals = self
            .normals()
            .map(|a| a.source_copy_with_effects(effects))
            .transpose()?;
        let tangents = self
            .tangents()
            .map(|a| a.source_copy_with_effects(effects))
            .transpose()?;
        let groups = self
            .groups()
            .iter()
            .map(|g| g.source_copy_with_effects(effects))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(Arc::new(Mutex::new(ModelData {
            positions,
            uvs,
            normals,
            tangents,
            groups,
            material: self.material().source_copy(),
        }))))
    }
}
struct SceneData {
    models: Arc<Mutex<list::ListData<MeshModel>>>,
    lights: Arc<Mutex<list::ListData<SceneLight>>>,
    environment: Option<SharedImageMap>,
    notifications: SceneNotifications,
}

/// The source scene's `arz` change argument for its two owned collections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneChange {
    Models,
    LightSources,
}

/// One retained scene listener. Multiple registrations remain distinct calls;
/// removal removes the last matching registration, as delegate removal does.
#[derive(Clone)]
pub struct SceneChangeHandler(Arc<SceneChangeCallback>);
type SceneChangeCallback = dyn Fn(&Scene, SceneChange) -> Result<(), SceneError> + Send + Sync;
impl SceneChangeHandler {
    pub fn new(
        handler: impl Fn(&Scene, SceneChange) -> Result<(), SceneError> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(handler))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
struct SceneNotifications {
    allow_changes: bool,
    pending: bool,
    handlers: Vec<SceneChangeHandler>,
}
impl Default for SceneNotifications {
    fn default() -> Self {
        Self {
            allow_changes: true,
            pending: false,
            handlers: Vec::new(),
        }
    }
}
/// Retained nt scene. Primitive scenes have no explicit lights; the future
/// renderer must implement avv's real default light rather than assuming unlit.
pub struct Scene(Arc<Mutex<SceneData>>);
impl Clone for Scene {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl fmt::Debug for Scene {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Scene({:p})", Arc::as_ptr(&self.0))
    }
}
impl Scene {
    fn from_parts(
        models: Vec<MeshModel>,
        lights: Vec<SceneLight>,
        environment: Option<SharedImageMap>,
    ) -> Self {
        Self(Arc::new(Mutex::new(SceneData {
            models: Arc::new(Mutex::new(list::ListData::new(models))),
            lights: Arc::new(Mutex::new(list::ListData::new(lights))),
            environment,
            notifications: SceneNotifications::default(),
        })))
    }
    fn from_copied_parts(
        models: Vec<MeshModel>,
        lights: Vec<SceneLight>,
        environment: Option<SharedImageMap>,
    ) -> Self {
        Self(Arc::new(Mutex::new(SceneData {
            models: Arc::new(Mutex::new(list::ListData::copied(models))),
            lights: Arc::new(Mutex::new(list::ListData::copied(lights))),
            environment,
            notifications: SceneNotifications::default(),
        })))
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.0, &b.0)
    }
    // zl.kv calls awx.a on a freshly constructed default single-model plane.
    // No caller has observed this material yet; abx's change notification has
    // no external listeners on this construction path.
    pub(crate) fn attach_fresh_plane_image(
        &self,
        image: &SharedImageMap,
        max_texture_size: u32,
    ) -> Result<(), SceneError> {
        let geometry = image.geometry();
        let width = u32::try_from(geometry.width()).map_err(|_| SceneError::InvalidProperty)?;
        let height = u32::try_from(geometry.height()).map_err(|_| SceneError::InvalidProperty)?;
        if width == 0 || height == 0 || max_texture_size == 0 {
            return Err(SceneError::InvalidProperty);
        }
        if width > max_texture_size || height > max_texture_size {
            // abx would compute split counts and f.a would resize/repartition.
            return Err(SceneError::UnsupportedAdjustment);
        }
        let models = self.models();
        if models.len() != 1 {
            return Err(SceneError::InvalidProperty);
        }
        let material = models[0].material();
        let mut state = guard(&material.0);
        if state.map.is_some() || state.map_parts != [1, 1] {
            return Err(SceneError::InvalidProperty);
        }
        state.map = Some(image.clone());
        Ok(())
    }
    /// Replaces the color map on every model of a prepared scene copy.
    pub(crate) fn replace_material_maps(
        &self,
        image: &SharedImageMap,
        max_texture_size: u32,
    ) -> Result<(), SceneError> {
        for model in self.try_models_snapshot()? {
            model
                .material()
                .replace_color_map(image, max_texture_size)?;
        }
        Ok(())
    }
    pub fn models(&self) -> Vec<MeshModel> {
        self.model_list().snapshot()
    }
    /// The actual retained model-list owner, distinct from its scene and items.
    pub fn model_list(&self) -> SceneList<MeshModel> {
        SceneList {
            core: guard(&self.0).models.clone(),
            owner: self.clone(),
            kind: SceneChange::Models,
        }
    }
    /// Snapshot the current actual model roots with explicit allocation failure.
    pub fn try_models_snapshot(&self) -> Result<Vec<MeshModel>, SceneError> {
        self.model_list().try_snapshot()
    }
    pub fn environment(&self) -> Option<SharedImageMap> {
        guard(&self.0).environment.clone()
    }
    pub fn explicit_light_count(&self) -> usize {
        self.light_list().len()
    }
    pub fn lights(&self) -> Vec<SceneLight> {
        self.light_list().snapshot()
    }
    /// The actual retained light-list owner, distinct from its scene and items.
    pub fn light_list(&self) -> SceneList<SceneLight> {
        SceneList {
            core: guard(&self.0).lights.clone(),
            owner: self.clone(),
            kind: SceneChange::LightSources,
        }
    }
    /// Snapshot the current actual light roots with explicit allocation failure.
    pub fn try_lights_snapshot(&self) -> Result<Vec<SceneLight>, SceneError> {
        self.light_list().try_snapshot()
    }

    pub fn add_changed(&self, handler: &SceneChangeHandler) {
        guard(&self.0).notifications.handlers.push(handler.clone());
    }
    pub fn remove_changed(&self, handler: &SceneChangeHandler) {
        let removed = {
            let mut state = guard(&self.0);
            state
                .notifications
                .handlers
                .iter()
                .rposition(|item| item.same_reference(handler))
                .map(|index| state.notifications.handlers.remove(index))
        };
        drop(removed);
    }
    pub fn allow_changes(&self) -> bool {
        guard(&self.0).notifications.allow_changes
    }
    pub fn set_allow_changes(&self, allow: bool) {
        guard(&self.0).notifications.allow_changes = allow;
    }
    pub fn has_pending_changes(&self) -> bool {
        guard(&self.0).notifications.pending
    }
    fn notify(&self, change: SceneChange) -> Result<(), SceneError> {
        let handlers = {
            let mut state = guard(&self.0);
            if !state.notifications.allow_changes {
                state.notifications.pending = true;
                return Ok(());
            }
            state.notifications.pending = false;
            state.notifications.handlers.clone()
        };
        for handler in handlers {
            (handler.0)(self, change)?;
        }
        Ok(())
    }

    /// Add actual model roots as one `ChangableList.a(T[])` operation.
    pub fn append_models_snapshot(&self, models: &[MeshModel]) -> Result<(), SceneError> {
        self.model_list().append(models)
    }
    /// Add actual light roots as one `ChangableList.a(T[])` operation.
    pub fn append_lights_snapshot(&self, lights: &[SceneLight]) -> Result<(), SceneError> {
        self.light_list().append(lights)
    }
    pub fn source_copy(&self) -> Result<Self, SceneError> {
        let mut unavailable = |_| Err(SceneError::MissingAllocationServices);
        self.source_copy_with_effects(&mut SceneAllocationEffects::new(&mut unavailable))
    }
    pub fn source_copy_with_effects(
        &self,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Self, SceneError> {
        // nt(nt) snapshots and copies models first. Allocation callbacks can
        // change the source before its lights and environment are reached.
        let source_models = self.try_models_snapshot()?;
        let mut models = Vec::new();
        models
            .try_reserve_exact(source_models.len())
            .map_err(|_| SceneError::Allocation)?;
        for model in source_models {
            models.push(model.source_copy_with_effects(effects)?);
        }
        let source_lights = self.try_lights_snapshot()?;
        let mut lights = Vec::new();
        lights
            .try_reserve_exact(source_lights.len())
            .map_err(|_| SceneError::Allocation)?;
        lights.extend(source_lights.iter().map(SceneLight::source_copy));
        let environment = self.environment();
        Ok(Self::from_copied_parts(models, lights, environment))
    }

    pub fn transform(&self, t: SceneTransform) -> Result<(), SceneError> {
        for model in self.models() {
            model.transform(t)?;
        }
        Ok(())
    }
}
fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug)]
pub struct SceneLight(Arc<SceneLightValues>);
/// Source zd light data. Primitive nt scenes own an actual empty light list.
#[derive(Clone, Debug)]
pub struct SceneLightValues {
    enabled: bool,
    kind: i32,
    position: MeshVector,
    ambient: [f32; 3],
    diffuse: [f32; 3],
    specular: [f32; 3],
    attenuation: [f32; 3],
    spot_direction: MeshVector,
    spot_cutoff: f32,
    spot_exponent: f32,
}
impl SceneLightValues {
    #[allow(clippy::too_many_arguments)] // Exact zd constructor data.
    pub fn new(
        enabled: bool,
        kind: i32,
        position: MeshVector,
        ambient: [f32; 3],
        diffuse: [f32; 3],
        specular: [f32; 3],
        attenuation: [f32; 3],
        spot_direction: MeshVector,
        spot_cutoff: f32,
        spot_exponent: f32,
    ) -> Self {
        Self {
            enabled,
            kind,
            position,
            ambient,
            diffuse,
            specular,
            attenuation,
            spot_direction,
            spot_cutoff,
            spot_exponent,
        }
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn kind(&self) -> i32 {
        self.kind
    }
    pub fn position(&self) -> MeshVector {
        self.position
    }
    pub fn ambient(&self) -> [f32; 3] {
        self.ambient
    }
    pub fn diffuse(&self) -> [f32; 3] {
        self.diffuse
    }
    pub fn specular(&self) -> [f32; 3] {
        self.specular
    }
    pub fn attenuation(&self) -> [f32; 3] {
        self.attenuation
    }
    pub fn spot_direction(&self) -> MeshVector {
        self.spot_direction
    }
    pub fn spot_cutoff(&self) -> f32 {
        self.spot_cutoff
    }
    pub fn spot_exponent(&self) -> f32 {
        self.spot_exponent
    }
}
impl SceneLight {
    pub fn new(values: SceneLightValues) -> Self {
        Self(Arc::new(values))
    }
    pub fn values(&self) -> SceneLightValues {
        (*self.0).clone()
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.0, &b.0)
    }
    pub fn source_copy(&self) -> Self {
        Self::new(self.values())
    }
}

#[cfg(test)]
mod laws;

mod camera;
mod effects;
pub(crate) mod uv_apply;
pub(crate) mod uv_curvature;
mod uv_depth;
pub(crate) mod uv_environment_light;
pub(crate) mod uv_light;
mod uv_mesh;
pub(crate) mod uv_multi_project;
pub(crate) mod uv_occlusion;
pub(crate) mod uv_project;
pub(crate) mod uv_thickness;
pub use camera::{
    CameraChange, CameraDelegate, CameraError, CameraFrustumBound, CameraHandler,
    CameraMatrixFactory, CameraNormalTransform, CameraOrbitComponent, CameraTransform, SceneCamera,
    WeakSceneCamera,
};
pub use effects::{SceneRotationOrder, SceneTransformOrigin, SceneTransformParameters};

#[cfg(test)]
mod mapped_plane_laws {
    use super::*;
    use crate::ImageGeometry;

    #[test]
    fn fresh_plane_assignment_and_scene_copy_preserve_real_image_identity() {
        let image = SharedImageMap::new(ImageGeometry::new(2, 2));
        image.set_constant(0, 0.75).unwrap();
        image.set_constant(3, 0.25).unwrap();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let mut effects = SceneAllocationEffects::new(&mut admit);
        let plane =
            generate_primitive(ScenePrimitiveParameters::plane(1, 1, true), &mut effects).unwrap();
        plane.attach_fresh_plane_image(&image, 64).unwrap();
        let original = plane.models()[0].material();
        assert!(original.map().unwrap().same_reference(&image));
        let copied = plane.source_copy_with_effects(&mut effects).unwrap();
        let copy_material = copied.models()[0].material();
        assert!(!copy_material.same_reference(&original));
        assert!(copy_material.map().unwrap().same_reference(&image));
        assert_eq!(
            copy_material
                .map()
                .unwrap()
                .channel(0, true)
                .unwrap()
                .read(0)
                .unwrap(),
            0.75
        );
        assert_eq!(
            copy_material
                .map()
                .unwrap()
                .channel(3, true)
                .unwrap()
                .read(0)
                .unwrap(),
            0.25
        );
    }

    #[test]
    fn oversized_image_requires_the_unimplemented_split_path() {
        let image = SharedImageMap::new(ImageGeometry::new(65, 2));
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let scene = generate_primitive(
            ScenePrimitiveParameters::plane(1, 1, true),
            &mut SceneAllocationEffects::new(&mut admit),
        )
        .unwrap();
        assert_eq!(
            scene.attach_fresh_plane_image(&image, 64),
            Err(SceneError::UnsupportedAdjustment)
        );
        assert!(scene.models()[0].material().map().is_none());
    }
}
