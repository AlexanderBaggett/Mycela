//! Empty input prototypes for documents with retained preview sample indices.
use super::*;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::OnceLock,
};
#[derive(Clone)]
pub struct SharedDefaultInputMapping(Arc<Mutex<Vec<i32>>>);
impl SharedDefaultInputMapping {
    pub fn new(values: Vec<i32>) -> Self {
        Self(Arc::new(Mutex::new(values)))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn values(&self) -> Vec<i32> {
        guard(&self.0).clone()
    }
    pub fn set(&self, index: i32, value: i32) -> Result<(), crate::ImageError> {
        let index = usize::try_from(index).map_err(|_| crate::ImageError::Index)?;
        *guard(&self.0)
            .get_mut(index)
            .ok_or(crate::ImageError::Index)? = value;
        Ok(())
    }
    fn at<E>(&self, index: i32) -> Result<i32, PointwiseError<E>> {
        guard(&self.0)
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .copied()
            .ok_or(PointwiseError::Index)
    }
}
const SAMPLE_COUNT: usize = 4;
#[derive(Clone, Copy)]
enum InitFailure {
    Image(crate::ImageError),
    Constructor,
}
struct Defaults<E> {
    prototypes: Vec<Option<PointwiseNode<E>>>,
    mapping: Mutex<Option<SharedDefaultInputMapping>>,
    failure: Option<InitFailure>,
}
// E is the callback error type for one engine universe; source has one static
// registry. Each typed universe owns exactly one complete/failed initializer.
static REGISTRIES: OnceLock<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>> = OnceLock::new();
fn registry<E: 'static>() -> Arc<Defaults<E>> {
    let store = REGISTRIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut types = guard(store);
    if let Some(value) = types.get(&TypeId::of::<E>()) {
        return value
            .downcast_ref::<Arc<Defaults<E>>>()
            .expect("prototype callback universe TypeId")
            .clone();
    }
    // No outward callbacks/foreign providers exist on these freshly constructed
    // slotless z7s. The lock only protects this private static initialization;
    // source-copy and all public access happen after it has been released.
    let mut value = Defaults {
        prototypes: vec![None; SAMPLE_COUNT],
        mapping: Mutex::new(None),
        failure: None,
    };
    for index in 0..SAMPLE_COUNT {
        let initialized = (|| -> Result<(), InitFailure> {
            let node = PointwiseNode::<E>::default_input().map_err(|error| match error {
                PointwiseError::Image(error) => InitFailure::Image(error),
                _ => InitFailure::Constructor,
            })?;
            value.prototypes[index] = Some(node.clone());
            // Actual b2 on a fresh slotless z7: bt=true, bx/bv/b3=false,
            // ao=-1, four false guarantees/effect flags, no a6 constant reads;
            // b5 commits bu=true/by=false with absent new-node handlers.
            guard(&node.core().mode_events).prepared_default =
                Some(DefaultPreparation::initialize());
            Ok(())
        })();
        if let Err(error) = initialized {
            value.failure = Some(error);
            break;
        }
    }
    if value.failure.is_none() {
        let mapping = SharedDefaultInputMapping::new(vec![0, 0, 0]);
        *guard(&value.mapping) = Some(mapping.clone());
        for index in 0..3 {
            if let Err(error) = mapping.set(index, index) {
                value.failure = Some(InitFailure::Image(error));
                break;
            }
        }
    }
    let value = Arc::new(value);
    types.insert(TypeId::of::<E>(), Box::new(value.clone()));
    value
}
fn ready<E>(value: &Defaults<E>) -> Result<(), PointwiseError<E>> {
    if let Some(error) = value.failure {
        // Preserve failed static roots/prefix forever. CLR wrapper/native
        // initializer exception binding is not qualified; never retry/reset or
        // return a null/empty prototype as if initialization succeeded.
        let _cause = match error {
            InitFailure::Image(v) => v.source_class(),
            InitFailure::Constructor => "UnsupportedCapability",
        };
        return Err(PointwiseError::UnsupportedCapability(
            "default-input static initialization failed; CLR initializer/native error wrapper unqualified",
        ));
    }
    Ok(())
}
fn selected<E>(value: &Defaults<E>, index: i32) -> Result<usize, PointwiseError<E>> {
    ready(value)?;
    let mapping = guard(&value.mapping)
        .clone()
        .ok_or(PointwiseError::NullReference)?;
    let index = usize::try_from(mapping.at::<E>(index)?).map_err(|_| PointwiseError::Index)?;
    if index >= SAMPLE_COUNT {
        return Err(PointwiseError::Index);
    }
    Ok(index)
}
pub(super) fn prototype<E: 'static>(index: i32) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let value = registry::<E>();
    let index = selected(&value, index)?;
    value.prototypes[index]
        .clone()
        .ok_or(PointwiseError::NullReference)
}
pub(super) fn prototype_by_resource<E: 'static>(
    index: i32,
) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let value = registry::<E>();
    ready(&value)?;
    value
        .prototypes
        .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
        .ok_or(PointwiseError::Index)?
        .clone()
        .ok_or(PointwiseError::NullReference)
}
pub(super) fn preview<E: 'static>(index: i32) -> Result<crate::Bitmap, PointwiseError<E>> {
    let value = registry::<E>();
    selected(&value, index)?;
    let blank = || {
        let layout = crate::BitmapLayout::new(168, 168, crate::BitmapFormat::Argb32)?;
        crate::Bitmap::from_buffer(
            layout,
            crate::ByteBuffer::new(vec![0; layout.stride() as usize * 168]),
        )
    };
    blank().map_err(|_| PointwiseError::Image(crate::ImageError::Allocation))
}
pub(super) fn mapping<E: 'static>() -> Result<Option<SharedDefaultInputMapping>, PointwiseError<E>>
{
    let value = registry::<E>();
    ready(&value)?;
    let mapping = guard(&value.mapping).clone();
    Ok(mapping)
}
pub(super) fn set_mapping<E: 'static>(
    mapping: Option<SharedDefaultInputMapping>,
) -> Result<(), PointwiseError<E>> {
    let value = registry::<E>();
    ready(&value)?;
    let old = std::mem::replace(&mut *guard(&value.mapping), mapping);
    drop(old);
    Ok(())
}
#[cfg(test)]
#[path = "graph_defaults_native_test.rs"]
mod native_test;
