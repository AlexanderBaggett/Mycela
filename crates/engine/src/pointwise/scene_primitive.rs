//! Genuine modern-property childless primitives in the existing graph region.
pub(super) mod capture_values;
pub(super) mod property;
use super::{PointwiseError, PointwiseNode, RenderCaptureSession, guard};
use crate::{
    LimitsDefinition, LimitsStep, ModernAdjustmentConfiguration, ModernPropertyConfiguration,
    ModernPropertyRecord, ModernPropertyValue, Scene, SceneAllocationEffects, SceneError,
    ScenePrimitiveKind, ScenePrimitiveParameters, SceneTransform, SceneVector, SharedLimits,
};
pub use property::{
    ModernNodeAnimation, ModernNodeProperty, ModernNodePropertyHandler, ModernNodePropertyKind,
    ModernNodeValue,
};
use std::{
    fmt,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
pub(super) struct PrimitiveState<E> {
    pub(super) kind: ScenePrimitiveKind,
    properties: Vec<Arc<property::PropertyRoot<E>>>,
    geometry_dirty: AtomicBool,
    transform_dirty: AtomicBool,
}
pub struct ScenePrimitiveNode<E = ()> {
    node: PointwiseNode<E>,
    state: Arc<PrimitiveState<E>>,
}
impl<E> Clone for ScenePrimitiveNode<E> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            state: self.state.clone(),
        }
    }
}
impl<E> fmt::Debug for ScenePrimitiveNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ScenePrimitiveNode({:?})", self.node)
    }
}
fn decimal(value: i32, scale: u32) -> crate::DecimalBits {
    crate::DecimalBits::new([
        value.unsigned_abs(),
        0,
        0,
        (scale << 16) | if value < 0 { 0x8000_0000 } else { 0 },
    ])
    .expect("finite built-in scene decimal")
}
pub(super) fn limits<E>(
    places: i32,
    increment: i32,
    scale: u32,
    minimum: i32,
    maximum: i32,
) -> Result<SharedLimits, PointwiseError<E>> {
    SharedLimits::from_definition(LimitsDefinition {
        decimal_places: places,
        step: LimitsStep::Derived(decimal(increment, scale)),
        minimum: decimal(minimum, 0),
        maximum: decimal(maximum, 0),
        used: true,
        use_range: None,
        curve_range: None,
    })
    .map_err(|_| PointwiseError::Arithmetic)
}
pub(super) fn make_property<E: 'static>(
    name: &str,
    value: ModernPropertyValue,
    limits: Vec<SharedLimits>,
    kind: ModernNodePropertyKind,
) -> Result<Arc<property::PropertyRoot<E>>, PointwiseError<E>> {
    let bounds = if kind == ModernNodePropertyKind::Boolean {
        vec![[0., 1.]]
    } else {
        limits
            .iter()
            .map(|l| {
                let v = l.snapshot();
                [v.minimum.to_f64() as f32, v.maximum.to_f64() as f32]
            })
            .collect()
    };
    let mut record = ModernPropertyRecord::new(
        value,
        ModernPropertyConfiguration {
            can_create_animation: true,
            adjustment: Some(ModernAdjustmentConfiguration {
                property_type: match kind {
                    ModernNodePropertyKind::Int32 => 0,
                    ModernNodePropertyKind::Double => 0,
                    ModernNodePropertyKind::Boolean => 4,
                    ModernNodePropertyKind::Vector3D => 14,
                },
                bounds,
            }),
        },
    )
    .map_err(PointwiseError::ModernProperty)?;
    record.name = Some(name.encode_utf16().collect());
    record.adjustable = true;
    record.animatable = true;
    property::PropertyRoot::new(record, limits, kind)
}
/// Source SelectedProperty construction for a fixed, nonempty option array.
/// Dynamic Options replacement and CLR array identity remain unsupported.
pub(super) fn make_selected_property<E: 'static>(
    name: &str,
    options: &[&str],
    value: i32,
) -> Result<Arc<property::PropertyRoot<E>>, PointwiseError<E>> {
    let max = i32::try_from(options.len())
        .map_err(|_| PointwiseError::Arithmetic)?
        .checked_sub(1)
        .ok_or(PointwiseError::InvalidCast)?;
    if max < 0 {
        return Err(PointwiseError::InvalidCast);
    }
    let limit = limits(0, 1, 0, 0, max)?;
    let mut record = ModernPropertyRecord::new(
        ModernPropertyValue::Selected {
            current: value,
            previous: value,
        },
        ModernPropertyConfiguration {
            can_create_animation: true,
            adjustment: Some(ModernAdjustmentConfiguration {
                property_type: 4,
                bounds: vec![[0.5, options.len() as f32 + 0.5]],
            }),
        },
    )
    .map_err(PointwiseError::ModernProperty)?;
    record.name = Some(name.encode_utf16().collect());
    record.adjustable = true;
    record.animatable = true;
    property::PropertyRoot::new_selected(
        record,
        vec![limit],
        options
            .iter()
            .map(|value| value.encode_utf16().collect())
            .collect(),
    )
}
pub(super) fn make_transform_vector_property<E: 'static>(
    name: &str,
    value: [f64; 3],
    rotate: bool,
) -> Result<Arc<property::PropertyRoot<E>>, PointwiseError<E>> {
    make_placement_vector_property(name, value, rotate, if rotate { 0 } else { 1 })
}

pub(super) fn make_placement_vector_property<E: 'static>(
    name: &str,
    value: [f64; 3],
    rotate: bool,
    increment_scale: u32,
) -> Result<Arc<property::PropertyRoot<E>>, PointwiseError<E>> {
    let limit = limits(
        3,
        1,
        increment_scale,
        if rotate { -360 } else { -100 },
        if rotate { 360 } else { 100 },
    )?;
    let mut data = limit.snapshot();
    if !rotate {
        let mut range = crate::RangeOptionsRecord::new(decimal(-10, 0), decimal(10, 0));
        range.decimal_places = 0;
        data.use_range = Some(crate::SharedRangeOptionsRecord::new(range));
    }
    data.curve_range = Some(crate::SharedCurveRangeRecord::new(
        crate::CurveRangeRecord {
            minimum: decimal(if rotate { -360 } else { 0 }, 0),
            maximum: decimal(if rotate { 360 } else { 10 }, 0),
            wrap_y: rotate,
        },
    ));
    let limit = SharedLimits::from_backing(data);
    make_property(
        name,
        ModernPropertyValue::Vector {
            current: value,
            previous: value,
        },
        vec![limit.clone(), limit.clone(), limit],
        ModernNodePropertyKind::Vector3D,
    )
}
pub(super) fn make_placement_properties<E: 'static>()
-> Result<Vec<Arc<property::PropertyRoot<E>>>, PointwiseError<E>> {
    let mut properties = Vec::new();
    for (name, value, increment, scale, minimum, maximum, wrap) in [
        ("Size", [1.; 3], 1, 0, 0, 10, false),
        ("Location", [0.; 3], 1, 1, 0, 10, false),
        ("Orientation", [0.; 3], 1, 0, -360, 360, true),
    ] {
        let l = limits(
            3,
            increment,
            scale,
            if wrap { -360 } else { -100 },
            if wrap { 360 } else { 100 },
        )?;
        let mut data = l.snapshot();
        if !wrap {
            let mut r = crate::RangeOptionsRecord::new(decimal(-10, 0), decimal(10, 0));
            r.decimal_places = 0;
            data.use_range = Some(crate::SharedRangeOptionsRecord::new(r));
        }
        data.curve_range = Some(crate::SharedCurveRangeRecord::new(
            crate::CurveRangeRecord {
                minimum: decimal(minimum, 0),
                maximum: decimal(maximum, 0),
                wrap_y: wrap,
            },
        ));
        let l = SharedLimits::from_backing(data);
        properties.push(make_property(
            name,
            ModernPropertyValue::Vector {
                current: value,
                previous: value,
            },
            vec![l.clone(), l.clone(), l],
            ModernNodePropertyKind::Vector3D,
        )?);
    }
    Ok(properties)
}

pub(super) fn make_boolean_property<E: 'static>(
    name: &str,
    value: bool,
) -> Result<Arc<property::PropertyRoot<E>>, PointwiseError<E>> {
    make_property(
        name,
        ModernPropertyValue::Boolean(value),
        vec![limits(0, 1, 0, 0, 1)?],
        ModernNodePropertyKind::Boolean,
    )
}

pub(super) fn make_percent_property<E: 'static>(
    name: &str,
    value: f64,
) -> Result<Arc<property::PropertyRoot<E>>, PointwiseError<E>> {
    make_property(
        name,
        ModernPropertyValue::Double {
            current: value,
            previous: value,
        },
        vec![limits(1, 1, 0, 0, 100)?],
        ModernNodePropertyKind::Double,
    )
}

impl<E: 'static> PrimitiveState<E> {
    fn new(kind: ScenePrimitiveKind) -> Result<Arc<Self>, PointwiseError<E>> {
        let mut properties = make_placement_properties()?;
        let integers: Vec<(&str, i32, i32, i32)> = match kind {
            ScenePrimitiveKind::Cone => vec![
                ("Length Segments", 10, 1, 100),
                ("Rotational Segments", 20, 1, 200),
                ("Cap Segments", 1, 1, 100),
            ],
            ScenePrimitiveKind::Cube => vec![
                ("X Segments", 1, 1, 100),
                ("Y Segments", 1, 1, 100),
                ("Z Segments", 1, 1, 100),
            ],
            ScenePrimitiveKind::Cylinder => vec![
                ("Length Segments", 1, 1, 100),
                ("Rotational Segments", 20, 3, 200),
                ("Cap Segments", 1, 1, 100),
            ],
            ScenePrimitiveKind::Disc => vec![
                ("Circular Segments", 1, 1, 100),
                ("Radial Segments", 20, 1, 200),
            ],
            ScenePrimitiveKind::Plane => vec![("X Segments", 1, 1, 100), ("Y Segmetns", 1, 1, 100)],
            ScenePrimitiveKind::Sphere => vec![
                ("Latitude Segments", 10, 2, 100),
                ("Longitude Segments", 20, 3, 200),
            ],
            ScenePrimitiveKind::Torus => {
                vec![("Ring Segments", 20, 3, 100), ("Pipe Segments", 16, 3, 200)]
            }
        };
        for (name, value, minimum, maximum) in integers {
            properties.push(make_property(
                name,
                ModernPropertyValue::Int32 {
                    current: value,
                    previous: value,
                },
                vec![limits(0, 1, 0, minimum, maximum)?],
                ModernNodePropertyKind::Int32,
            )?);
        }
        if let Some((name, default, places, step, scale)) = match kind {
            ScenePrimitiveKind::Cylinder => Some(("Taper", 0., 0, 1, 1)),
            ScenePrimitiveKind::Disc => Some(("Inner Radius", 0., 1, 1, 0)),
            ScenePrimitiveKind::Torus => Some(("Pipe Radius", 10., 1, 1, 0)),
            _ => None,
        } {
            properties.push(make_property(
                name,
                ModernPropertyValue::Double {
                    current: default,
                    previous: default,
                },
                vec![limits(places, step, scale, 0, 100)?],
                ModernNodePropertyKind::Double,
            )?);
        }
        if let Some(name) = match kind {
            ScenePrimitiveKind::Cone | ScenePrimitiveKind::Cylinder => Some("Make Caps"),
            ScenePrimitiveKind::Disc | ScenePrimitiveKind::Plane => Some("Two-sided"),
            _ => None,
        } {
            properties.push(make_boolean_property(name, true)?);
        }
        Ok(Arc::new(Self {
            kind,
            properties,
            geometry_dirty: AtomicBool::new(true),
            transform_dirty: AtomicBool::new(true),
        }))
    }
    pub(super) fn bind(&self, node: &Arc<super::graph::NodeCore<E>>) {
        for property in &self.properties {
            property.bind(node);
        }
    }
    fn copied(
        &self,
        source_copy: bool,
        mut session: Option<&mut RenderCaptureSession<E>>,
    ) -> Result<Arc<Self>, PointwiseError<E>> {
        let properties = self
            .properties
            .iter()
            .map(|p| p.copied(source_copy, session.as_deref_mut()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arc::new(Self {
            kind: self.kind,
            properties,
            geometry_dirty: AtomicBool::new(self.geometry_dirty.load(Ordering::Acquire)),
            transform_dirty: AtomicBool::new(self.transform_dirty.load(Ordering::Acquire)),
        }))
    }
}
impl<E: 'static> ScenePrimitiveNode<E> {
    pub fn new(kind: ScenePrimitiveKind) -> Result<Self, PointwiseError<E>> {
        let state = PrimitiveState::new(kind)?;
        let node = PointwiseNode::publish_scene_primitive(state.clone());
        Ok(Self { node, state })
    }
    pub fn node(&self) -> PointwiseNode<E> {
        self.node.clone()
    }
    pub fn kind(&self) -> ScenePrimitiveKind {
        self.state.kind
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        self.node.same_reference(&b.node)
    }
    pub fn properties(&self) -> Vec<ModernNodeProperty<E>> {
        self.state
            .properties
            .iter()
            .map(|root| ModernNodeProperty::from_root(self.node.clone(), root.clone()))
            .collect()
    }
    pub fn property(&self, ix: i32) -> Result<ModernNodeProperty<E>, PointwiseError<E>> {
        self.state
            .properties
            .get(usize::try_from(ix).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .map(|root| ModernNodeProperty::from_root(self.node.clone(), root))
            .ok_or(PointwiseError::Index)
    }
    pub fn source_clone(&self) -> Result<Self, PointwiseError<E>> {
        let state = self.state.copied(true, None)?;
        let node = PointwiseNode::publish_scene_primitive(state.clone());
        Ok(Self { node, state })
    }
    pub(super) fn capture_with(
        &self,
        session: &mut RenderCaptureSession<E>,
    ) -> Result<Self, PointwiseError<E>> {
        let state = self.state.copied(false, Some(session))?;
        let node = PointwiseNode::publish_scene_primitive(state.clone());
        Ok(Self { node, state })
    }
    pub fn render_scene_unbound(&self, _time: f64) -> Result<Scene, PointwiseError<E>> {
        Err(PointwiseError::Scene(SceneError::MissingAllocationServices))
    }
    pub fn render_scene(
        &self,
        time: f64,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Scene, PointwiseError<E>> {
        let _frame = self.clone();
        let mut values = Vec::new();
        for property in self.properties().into_iter().skip(3) {
            values.push(property.evaluated_value(time)?);
        }
        let int = |i: usize| match values.get(i) {
            Some(ModernNodeValue::Int32(v)) => Ok(*v),
            _ => Err(PointwiseError::InvalidCast),
        };
        let num = |i: usize| match values.get(i) {
            Some(ModernNodeValue::Double(v)) => Ok(*v),
            _ => Err(PointwiseError::InvalidCast),
        };
        let flag = |i: usize| match values.get(i) {
            Some(ModernNodeValue::Boolean(v)) => Ok(*v),
            _ => Err(PointwiseError::InvalidCast),
        };
        let p = match self.kind() {
            ScenePrimitiveKind::Cone => {
                ScenePrimitiveParameters::cone(int(0)?, int(1)?, int(2)?, flag(3)?)
            }
            ScenePrimitiveKind::Cube => ScenePrimitiveParameters::cube(int(0)?, int(1)?, int(2)?),
            ScenePrimitiveKind::Cylinder => {
                ScenePrimitiveParameters::cylinder(int(0)?, int(1)?, int(2)?, num(3)?, flag(4)?)
            }
            ScenePrimitiveKind::Disc => {
                ScenePrimitiveParameters::disc(int(0)?, int(1)?, num(2)?, flag(3)?)
            }
            ScenePrimitiveKind::Plane => {
                ScenePrimitiveParameters::plane(int(0)?, int(1)?, flag(2)?)
            }
            ScenePrimitiveKind::Sphere => ScenePrimitiveParameters::sphere(int(0)?, int(1)?),
            ScenePrimitiveKind::Torus => ScenePrimitiveParameters::torus(int(0)?, int(1)?, num(2)?),
        };
        let scene = crate::generate_primitive(p, effects).map_err(PointwiseError::Scene)?;
        // Source z8.k5 evaluates k4 AFTER yd and all geometry allocation callbacks.
        let mut vectors = [[0.; 3]; 3];
        for (i, value) in vectors.iter_mut().enumerate() {
            let ModernNodeValue::Vector3D(v) = self.property(i as i32)?.evaluated_value(time)?
            else {
                return Err(PointwiseError::InvalidCast);
            };
            *value = v;
        }
        scene
            .transform(SceneTransform::from_components(
                SceneVector::new(vectors[0][0], vectors[0][1], vectors[0][2]),
                SceneVector::new(vectors[1][0], vectors[1][1], vectors[1][2]),
                SceneVector::new(vectors[2][0], vectors[2][1], vectors[2][2]),
            ))
            .map_err(PointwiseError::Scene)?;
        Ok(scene)
    }
    pub(super) fn raw_capture_values(
        &self,
        comparison: &mut super::property::capture::capture_values::Comparison<E>,
    ) -> Result<Vec<u64>, PointwiseError<E>> {
        capture_values::primitive(self, comparison)
    }
}
impl<E: 'static> PointwiseNode<E> {
    pub fn scene_primitive(&self) -> Option<ScenePrimitiveNode<E>> {
        match &self.core().payload {
            super::graph::Payload::ScenePrimitive { state } => Some(ScenePrimitiveNode {
                node: self.clone(),
                state: state.clone(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod laws;
