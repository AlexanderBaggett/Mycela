//! A render-frame copy, deliberately different from source Clone: preserve
//! actual warm/dirty caches, null slots and raw values; copy no host delegates.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(crate) struct CurveCapture<E> {
    copies: BTreeMap<ObjectId, Root<E>>,
    active: BTreeSet<ObjectId>,
}
impl<E> Default for CurveCapture<E> {
    fn default() -> Self {
        Self {
            copies: BTreeMap::new(),
            active: BTreeSet::new(),
        }
    }
}
impl<E> CurveCapture<E> {
    pub(crate) fn curve(
        &mut self,
        original: &SharedCurve<E>,
    ) -> Result<SharedCurve<E>, CurveError<E>> {
        self.root(&original.0).map(SharedCurve)
    }
    fn root(&mut self, original: &Root<E>) -> Result<Root<E>, CurveError<E>> {
        if self.active.contains(&original.core.id) {
            return Err(CurveError::UnsupportedCapability);
        }
        if let Some(copy) = self.copies.get(&original.core.id) {
            return Ok(copy.clone());
        }
        // Only clone retained child handles/metadata while the object is locked.
        // No child/cache read, callback or final drop crosses this guard.
        let state = original.read(|object| match object {
            Object::Curve(c) => Object::Curve(c.clone()),
            Object::Layer(l) => Object::Layer(l.clone()),
            Object::Control(c) => Object::Control(Control {
                id: c.id,
                name: c.name.clone(),
                active: c.active,
                value: c.value,
                bounds: c.bounds,
                events: Events::Absent,
            }),
        });
        self.active.insert(original.core.id);
        let copy = Root::new(Object::Curve(Composite {
            layers: Vec::new(),
            minimum: 0.0,
            maximum: 0.0,
            wrap: false,
        }));
        let result = (|| {
            let mut state = state;
            match &mut state {
                Object::Curve(c) => {
                    for id in c.layers.iter_mut().flatten() {
                        let child = self.root(&original.at(*id))?;
                        copy.join(&child);
                        *id = child.core.id;
                    }
                }
                Object::Layer(l) => {
                    for id in l.common.iter_mut().flatten() {
                        let child = self.root(&original.at(*id))?;
                        copy.join(&child);
                        *id = child.core.id;
                    }
                    for id in &mut l.extra {
                        let child = self.root(&original.at(*id))?;
                        copy.join(&child);
                        *id = child.core.id;
                    }
                    l.noise = l.noise.as_ref().map(noise::State::detached_render_copy);
                    l.custom = l.custom.as_ref().map(custom::State::detached_render_copy);
                }
                Object::Control(c) => {
                    if let Value::Modulated {
                        child: Some(id), ..
                    } = &mut c.value
                    {
                        let child = self.root(&original.at(*id))?;
                        copy.join(&child);
                        *id = child.core.id;
                    }
                }
            }
            copy.write(|slot| *slot = state);
            Ok(copy.clone())
        })();
        self.active.remove(&original.core.id);
        if result.is_ok() {
            self.copies.insert(original.core.id, copy);
        }
        result
    }
}

/// Callback-free raw render inputs, with encounter-order graph identities.
/// Addresses never enter this comparison data; warm/dirty child cache values do.
#[derive(Default)]
pub(crate) struct CurveValues {
    ids: BTreeMap<ObjectId, u64>,
    values: Vec<u64>,
}
impl CurveValues {
    pub(crate) fn read<E>(&mut self, curve: &SharedCurve<E>) -> Vec<u64> {
        self.values.clear();
        self.ids.clear();
        self.root(&curve.0);
        self.values.clone()
    }
    fn root<E>(&mut self, root: &Root<E>) {
        if let Some(id) = self.ids.get(&root.core.id) {
            self.values.extend([0, *id]);
            return;
        }
        let id = self.ids.len() as u64 + 1;
        self.ids.insert(root.core.id, id);
        self.values.extend([1, id]);
        let object: Object<E> = root.read(|o| match o {
            Object::Curve(c) => Object::Curve(c.clone()),
            Object::Layer(l) => Object::Layer(l.clone()),
            Object::Control(c) => Object::Control(Control {
                id: c.id,
                name: c.name.clone(),
                active: c.active,
                value: c.value,
                bounds: c.bounds,
                events: Events::Absent,
            }),
        });
        match object {
            Object::Curve(c) => {
                self.values.extend([
                    0,
                    c.minimum.to_bits() as u64,
                    c.maximum.to_bits() as u64,
                    c.wrap as u64,
                    c.layers.len() as u64,
                ]);
                for child in c.layers {
                    self.child(root, child);
                }
            }
            Object::Layer(l) => {
                self.values
                    .extend([1, l.kind as u64, l.mode as u64, l.common.len() as u64]);
                for child in l.common {
                    self.child(root, child);
                }
                self.values.push(l.extra.len() as u64);
                for child in l.extra {
                    self.child(root, Some(child));
                }
                match l.noise {
                    Some(s) => {
                        self.values.push(1);
                        s.render_values(&mut self.values);
                    }
                    None => self.values.push(0),
                }
                match l.custom {
                    Some(s) => {
                        self.values.push(1);
                        s.render_values(&mut self.values);
                    }
                    None => self.values.push(0),
                }
            }
            Object::Control(c) => {
                self.values.extend([2, c.id as u64, c.active as u64]);
                match c.name {
                    Some(s) => {
                        self.values.push(s.len() as u64 + 1);
                        self.values.extend(s.into_iter().map(u64::from));
                    }
                    None => self.values.push(0),
                }
                match c.bounds {
                    Some(b) => {
                        self.values
                            .extend([1, b[0].to_bits() as u64, b[1].to_bits() as u64])
                    }
                    None => self.values.push(0),
                }
                match c.value {
                    Value::Toggle(v) => self.values.extend([0, v as u64]),
                    Value::Scalar(v) => self.values.extend([1, v.to_bits() as u64]),
                    Value::Direction(v) => self.values.extend([2, v as u64]),
                    Value::Blend(v) => self.values.extend([3, v as u64]),
                    Value::Modulated {
                        scalar,
                        mode,
                        child,
                    } => {
                        self.values
                            .extend([4, scalar.to_bits() as u64, mode as u64]);
                        self.child(root, child);
                    }
                }
            }
        }
    }
    fn child<E>(&mut self, root: &Root<E>, child: Option<ObjectId>) {
        match child {
            Some(id) => {
                self.values.push(1);
                self.root(&root.at(id));
            }
            None => self.values.push(0),
        }
    }
}
