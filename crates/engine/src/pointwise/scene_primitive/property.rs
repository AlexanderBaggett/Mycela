use super::*;
use crate::live_double_property::DoublePropertyCore;
use crate::{
    ChangeDelegate, ChangeDispatchError, ChangeHandler, ChangeNotifier, ContextChangeArgument,
    ModernPropertyChangeArgs, ModernPropertyRecord, ModernPropertyUndoInfo, ModernPropertyValue,
    ModernUndoValue, SharedDoubleProperty,
};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModernNodePropertyKind {
    Int32,
    Double,
    Boolean,
    Vector3D,
}

#[cfg(test)]
mod selected_laws {
    use super::*;
    fn selected() -> ModernNodeProperty<()> {
        let parent = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube)
            .unwrap()
            .node();
        let root = super::super::make_selected_property::<()>(
            "Origin",
            &["World", "All Models", "Each Model"],
            1,
        )
        .unwrap();
        root.bind(&parent.core());
        ModernNodeProperty::from_root(parent, root)
    }

    #[test]
    fn selected_owns_fixed_options_and_constructs_source_animation_limits() {
        let p = selected();
        assert_eq!(p.value(), ModernNodeValue::Int32(1));
        assert_eq!(p.selected_options().unwrap().len(), 3);
        p.set_int32(2).unwrap();
        p.set_int32(3).unwrap();
        assert_eq!(p.value(), ModernNodeValue::Int32(2));
        guard(p.root.state().unwrap()).record.animatable = false;
        let animation = p.animation().unwrap().unwrap();
        let limits = animation
            .limits()
            .unwrap()
            .value(0)
            .unwrap()
            .unwrap()
            .snapshot();
        assert_eq!(limits.minimum.to_framework_f64(), 0.5);
        assert_eq!(limits.maximum.to_framework_f64(), 3.5);
        assert_eq!(limits.decimal_places, 1);
    }

    #[test]
    fn selected_animation_rounds_even_then_shifts_and_clamps() {
        let p = selected();
        let animation = p.animation().unwrap().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve
            .insert(
                0,
                Some(&crate::CurveLayerHandle::new(
                    crate::AnalyticCurveKind::Linear,
                )),
            )
            .unwrap();
        curve.set_minimum(0.5);
        curve.set_maximum(3.5);
        animation.flags().unwrap().set(0, true).unwrap();
        assert_eq!(p.evaluated_value(0.).unwrap(), ModernNodeValue::Int32(0));
        assert_eq!(p.evaluated_value(0.5).unwrap(), ModernNodeValue::Int32(1));
        assert_eq!(p.evaluated_value(1.).unwrap(), ModernNodeValue::Int32(2));
    }

    #[test]
    fn selected_wire_uses_actual_integer_and_retains_construction_options() {
        let p = selected();
        p.set_int32(2).unwrap();
        let mut writer = crate::BinaryWriter::default();
        p.save_binary(&mut writer).unwrap();
        let restored = selected();
        restored
            .load_binary(&mut crate::BinaryCursor::new(writer.bytes()).unwrap(), 3.75)
            .unwrap();
        assert_eq!(restored.value(), ModernNodeValue::Int32(2));
        assert_eq!(restored.selected_options(), p.selected_options());
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModernNodeValue {
    Int32(i32),
    Double(f64),
    Boolean(bool),
    Vector3D([f64; 3]),
}
#[derive(Debug)]
struct SceneDoubleCallbackError<E>(pub(super) Box<PointwiseError<E>>);
struct TypedState {
    record: ModernPropertyRecord,
    limits: Vec<SharedLimits>,
}
enum Backing<E> {
    Int32(Mutex<TypedState>),
    Boolean(Mutex<TypedState>),
    Vector3D(Mutex<TypedState>),
    Double(DoublePropertyCore<SceneDoubleCallbackError<E>>),
}
struct Envelope<E> {
    root: Weak<PropertyRoot<E>>,
    node: Weak<super::super::graph::NodeCore<E>>,
    args: Arc<ContextChangeArgument<E>>,
}
pub(in crate::pointwise) struct PropertyRoot<E> {
    animation: Mutex<Option<crate::GradientAnimation<E>>>,
    kind: ModernNodePropertyKind,
    backing: Backing<E>,
    parent: Mutex<Weak<super::super::graph::NodeCore<E>>>,
    labels: Mutex<Option<Arc<[Vec<u16>; 3]>>>,
    // SelectedProperty's fixed construction options. A mutable CLR options
    // array and its public replacement path remain a separate capability.
    selected_options: Mutex<Option<Arc<[Vec<u16>]>>>,
    notifier: ChangeNotifier<Envelope<E>, PointwiseError<E>>,
}
/// One actual modern IProperty occurrence. Clone retains its root and sponsors
/// the same graph ParentNode. Scalar observations are explicitly returned values.
pub struct ModernNodeProperty<E = ()> {
    owner: PointwiseNode<E>,
    pub(super) root: Arc<PropertyRoot<E>>,
}
impl<E> Clone for ModernNodeProperty<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            root: self.root.clone(),
        }
    }
}
impl<E> fmt::Debug for ModernNodeProperty<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ModernNodeProperty({:p})", Arc::as_ptr(&self.root))
    }
}
pub struct ModernNodePropertyHandler<E = ()> {
    handler: ChangeHandler<Envelope<E>, PointwiseError<E>>,
}
impl<E> Clone for ModernNodePropertyHandler<E> {
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
        }
    }
}
fn sender<E: 'static>(e: &Envelope<E>) -> Result<ModernNodeProperty<E>, PointwiseError<E>> {
    let root = e.root.upgrade().ok_or(PointwiseError::NullReference)?;
    let node = e
        .node
        .upgrade()
        .or_else(|| guard(&root.parent).upgrade())
        .ok_or(PointwiseError::NullReference)?;
    let sponsor =
        super::super::region::Sponsor::for_node(&node).ok_or(PointwiseError::NullReference)?;
    Ok(ModernNodeProperty {
        owner: PointwiseNode {
            id: node.id,
            sponsor,
        },
        root,
    })
}

impl<E: 'static> ModernNodePropertyHandler<E> {
    pub fn new(
        callback: impl Fn(&ModernNodeProperty<E>, Arc<ContextChangeArgument<E>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            handler: ChangeHandler::new(move |_, e: Option<Arc<Envelope<E>>>| {
                let e = e.ok_or(PointwiseError::NullReference)?;
                callback(&sender(&e)?, e.args.clone()).map_err(PointwiseError::Observer)
            }),
        }
    }
    pub fn same_binding(&self, b: &Self) -> bool {
        self.handler.same_binding(&b.handler)
    }
}
fn dispatch_error<E>(e: ChangeDispatchError<PointwiseError<E>>) -> PointwiseError<E> {
    match e {
        ChangeDispatchError::Handler(e) => e,
        ChangeDispatchError::Framework(e) => PointwiseError::PropertyNotification(e),
    }
}
fn double_error<E>(
    e: crate::DoublePropertyError<SceneDoubleCallbackError<E>>,
) -> PointwiseError<E> {
    match e {
        crate::DoublePropertyError::Notification(ChangeDispatchError::Handler(
            crate::DoubleCallbackError::Observer(SceneDoubleCallbackError(e)),
        )) => *e,
        crate::DoublePropertyError::MissingLimits => PointwiseError::NullReference,
        crate::DoublePropertyError::BackingKind => PointwiseError::InvalidCast,
        crate::DoublePropertyError::Wire(e) => PointwiseError::ModernProperty(e),
        crate::DoublePropertyError::Rounding(e) => PointwiseError::DecimalRounding(e),
        crate::DoublePropertyError::Notification(ChangeDispatchError::Framework(e)) => {
            PointwiseError::PropertyNotification(e)
        }
        crate::DoublePropertyError::Notification(_) => PointwiseError::NullReference,
    }
}
impl<E: 'static> PropertyRoot<E> {
    pub(in crate::pointwise) fn new(
        record: ModernPropertyRecord,
        limits: Vec<SharedLimits>,
        kind: ModernNodePropertyKind,
    ) -> Result<Arc<Self>, PointwiseError<E>> {
        let labels = if kind == ModernNodePropertyKind::Vector3D {
            let orientation = record.name.as_ref().is_some_and(|n| {
                n.iter().copied().eq("Orientation".encode_utf16())
                    || n.iter().copied().eq("Rotate".encode_utf16())
            });
            Some(Arc::new(
                (if orientation {
                    ["H:", "P:", "R:"]
                } else {
                    ["X:", "Y:", "Z:"]
                })
                .map(|s| s.encode_utf16().collect()),
            ))
        } else {
            None
        };
        let (double, backing) = if kind == ModernNodePropertyKind::Double {
            let double = SharedDoubleProperty::from_backing(record, limits.first().cloned())
                .map_err(double_error)?;
            let core = double.unsponsored_core();
            (Some(double), Backing::Double(core))
        } else {
            let state = Mutex::new(TypedState { record, limits });
            let backing = match kind {
                ModernNodePropertyKind::Int32 => Backing::Int32(state),
                ModernNodePropertyKind::Boolean => Backing::Boolean(state),
                ModernNodePropertyKind::Vector3D => Backing::Vector3D(state),
                ModernNodePropertyKind::Double => unreachable!(),
            };
            (None, backing)
        };
        let root = Arc::new_cyclic(|weak| Self {
            animation: Mutex::new(None),
            kind,
            backing,
            parent: Mutex::new(Weak::new()),
            labels: Mutex::new(labels),
            selected_options: Mutex::new(None),
            notifier: ChangeNotifier::new(Arc::new(Envelope {
                root: weak.clone(),
                node: Weak::new(),
                args: ContextChangeArgument::shared_empty(),
            })),
        });
        // zl.g subscribes its actual node callback before later host subscriptions.
        let forward = ChangeHandler::new(|_, e: Option<Arc<Envelope<E>>>| {
            let e = e.ok_or(PointwiseError::NullReference)?;
            let property = sender(&e)?;
            property.node_changed(e.args.clone())
        });
        root.notifier
            .add(&ChangeDelegate::from_handlers(&[forward]));
        if let Some(double) = double {
            let weak = Arc::downgrade(&root);
            let handler = crate::DoublePropertyHandler::new(
                move |_, args: Option<Arc<ContextChangeArgument<SceneDoubleCallbackError<E>>>>| {
                    let root = weak.upgrade().ok_or_else(|| {
                        SceneDoubleCallbackError(Box::new(PointwiseError::NullReference))
                    })?;
                    let args = match args.as_deref() {
                        None | Some(ContextChangeArgument::Empty) => {
                            ContextChangeArgument::shared_empty()
                        }
                        Some(ContextChangeArgument::Property(args)) => {
                            Arc::new(ContextChangeArgument::Property(
                                args.rebind_double_context().ok_or_else(|| {
                                    SceneDoubleCallbackError(Box::new(PointwiseError::InvalidCast))
                                })?,
                            ))
                        }
                        Some(ContextChangeArgument::Modern(_))
                        | Some(ContextChangeArgument::Animation(_)) => {
                            return Err(SceneDoubleCallbackError(Box::new(
                                PointwiseError::InvalidCast,
                            )));
                        }
                    };
                    root.emit(args)
                        .map_err(|e| SceneDoubleCallbackError(Box::new(e)))
                },
            );
            double.add(&crate::DoublePropertyDelegate::from_handlers(&[handler]));
        }
        Ok(root)
    }
    pub(super) fn new_selected(
        record: ModernPropertyRecord,
        limits: Vec<SharedLimits>,
        options: Vec<Vec<u16>>,
    ) -> Result<Arc<Self>, PointwiseError<E>> {
        if options.is_empty() || !matches!(record.value(), ModernPropertyValue::Selected { .. }) {
            return Err(PointwiseError::InvalidCast);
        }
        let root = Self::new(record, limits, ModernNodePropertyKind::Int32)?;
        *guard(&root.selected_options) = Some(options.into());
        Ok(root)
    }
    fn state(&self) -> Option<&Mutex<TypedState>> {
        match &self.backing {
            Backing::Int32(s) | Backing::Boolean(s) | Backing::Vector3D(s) => Some(s),
            Backing::Double(_) => None,
        }
    }
    pub(in crate::pointwise) fn bind(&self, node: &Arc<super::super::graph::NodeCore<E>>) {
        *guard(&self.parent) = Arc::downgrade(node);
    }
    fn emit(
        self: &Arc<Self>,
        args: Arc<ContextChangeArgument<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let node = guard(&self.parent).clone();
        self.notifier
            .emit(Some(Arc::new(Envelope {
                root: Arc::downgrade(self),
                node,
                args,
            })))
            .map_err(dispatch_error)
    }
    fn record(&self) -> ModernPropertyRecord {
        match &self.backing {
            Backing::Double(core) => core.record_snapshot(),
            _ => guard(self.state().expect("typed property backing"))
                .record
                .copy_record_snapshot(),
        }
    }
    pub(super) fn limits(&self) -> Vec<SharedLimits> {
        match &self.backing {
            Backing::Double(core) => core
                .retain()
                .ok()
                .and_then(|p| p.snapshot().ok())
                .and_then(|p| p.limits)
                .into_iter()
                .collect(),
            _ => guard(self.state().expect("typed property backing"))
                .limits
                .clone(),
        }
    }
    fn with_record<T>(&self, f: impl FnOnce(&mut ModernPropertyRecord) -> T) -> T {
        match &self.backing {
            Backing::Double(core) => core.with_scene_record(f),
            _ => f(&mut guard(self.state().expect("typed backing")).record),
        }
    }
    pub(in crate::pointwise) fn copied(
        &self,
        source_copy: bool,
        mut session: Option<&mut RenderCaptureSession<E>>,
    ) -> Result<Arc<Self>, PointwiseError<E>> {
        let mut record = self.record();
        if source_copy && let ModernPropertyValue::Vector { previous, .. } = record.value_mut() {
            *previous = [0.; 3];
        }
        let animation = guard(&self.animation).clone();
        let animation = match animation {
            Some(a) => Some(match session.as_deref_mut() {
                Some(session) => session.copy_animation(&a)?,
                None => a.source_clone()?,
            }),
            None => None,
        };
        let mut limits = self.limits();
        if source_copy && self.kind == ModernNodePropertyKind::Boolean {
            limits.clear();
        }
        let limits = match session {
            Some(session) => limits.iter().map(|l| session.copy_limits(l)).collect(),
            None => limits.iter().map(SharedLimits::clone_source).collect(),
        };
        let copy = Self::new(record, limits, self.kind)?;
        *guard(&copy.animation) = animation;
        *guard(&copy.labels) = guard(&self.labels).clone();
        *guard(&copy.selected_options) = guard(&self.selected_options)
            .as_ref()
            .map(|options| Arc::from(options.as_ref().to_vec()));
        Ok(copy)
    }
}
fn decimal_i32<E>(value: crate::DecimalBits) -> Result<i32, PointwiseError<E>> {
    let b = value.bits();
    let magnitude = u128::from(b[0]) | (u128::from(b[1]) << 32) | (u128::from(b[2]) << 64);
    let whole = magnitude / 10_u128.pow((b[3] >> 16) & 255);
    let negative = b[3] & 0x8000_0000 != 0;
    let maximum = if negative {
        2147483648_u128
    } else {
        2147483647_u128
    };
    if whole > maximum {
        return Err(PointwiseError::Image(crate::ImageError::Overflow));
    }
    Ok(if negative {
        (whole as i32).wrapping_neg()
    } else {
        whole as i32
    })
}
fn selected_animation_limits<E>(count: usize) -> Result<SharedLimits, PointwiseError<E>> {
    let maximum = u32::try_from(count)
        .ok()
        .and_then(|n| n.checked_mul(10))
        .and_then(|n| n.checked_add(5))
        .ok_or(PointwiseError::Arithmetic)?;
    let decimal =
        |n| crate::DecimalBits::new([n, 0, 0, 1 << 16]).map_err(|_| PointwiseError::Arithmetic);
    SharedLimits::from_definition(crate::LimitsDefinition {
        decimal_places: 1,
        step: crate::LimitsStep::Derived(decimal(1)?),
        minimum: decimal(5)?,
        maximum: decimal(maximum)?,
        used: true,
        use_range: None,
        curve_range: None,
    })
    .map_err(|_| PointwiseError::Arithmetic)
}
impl<E: 'static> ModernNodeProperty<E> {
    pub(in crate::pointwise) fn from_root(
        owner: PointwiseNode<E>,
        root: Arc<PropertyRoot<E>>,
    ) -> Self {
        Self { owner, root }
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        Arc::ptr_eq(&self.root, &b.root)
    }
    pub fn parent(&self) -> PointwiseNode<E> {
        self.owner.clone()
    }
    pub fn kind(&self) -> ModernNodePropertyKind {
        self.root.kind
    }
    pub fn name(&self) -> Option<Vec<u16>> {
        self.root.record().name
    }
    pub fn component_labels(&self) -> Option<[Vec<u16>; 3]> {
        guard(&self.root.labels)
            .as_ref()
            .map(|v| v.as_ref().clone())
    }
    /// Construction-time option text. Dynamic CLR array identity/replacement is
    /// intentionally not exposed by this finite selection-property profile.
    pub fn selected_options(&self) -> Option<Vec<Vec<u16>>> {
        guard(&self.root.selected_options)
            .as_ref()
            .map(|v| v.to_vec())
    }
    pub fn component_limits(&self, ix: usize) -> Option<SharedLimits> {
        self.root.limits().get(ix).cloned()
    }
    /// Owned value, metadata and adjustment-wire observation. Animation lives in
    /// the actual retained owner exposed by retained_animation/animation; the
    /// record animation field is not populated or used as animation authority.
    pub fn backing_snapshot(&self) -> ModernPropertyRecord {
        self.root.record()
    }
    pub fn value(&self) -> ModernNodeValue {
        match self.root.record().value() {
            ModernPropertyValue::Int32 { current, .. }
            | ModernPropertyValue::Selected { current, .. } => ModernNodeValue::Int32(*current),
            ModernPropertyValue::Double { current, .. } => ModernNodeValue::Double(*current),
            ModernPropertyValue::Boolean(v) => ModernNodeValue::Boolean(*v),
            ModernPropertyValue::Vector { current, .. } => ModernNodeValue::Vector3D(*current),
            _ => unreachable!("closed modern primitive cohort"),
        }
    }
    pub fn add_change_handler(&self, handler: &ModernNodePropertyHandler<E>) {
        self.root
            .notifier
            .add(&ChangeDelegate::from_handlers(std::slice::from_ref(
                &handler.handler,
            )));
    }
    pub fn remove_change_handler(
        &self,
        handler: &ModernNodePropertyHandler<E>,
    ) -> Result<(), PointwiseError<E>> {
        self.root
            .notifier
            .remove(&ChangeDelegate::from_handlers(std::slice::from_ref(
                &handler.handler,
            )))
            .map_err(PointwiseError::PropertyNotification)
    }
    pub fn lock_changes(&self) {
        self.root.notifier.lock_changes();
    }
    pub fn unlock_changes(&self, pending: bool) -> Result<(), PointwiseError<E>> {
        self.root.notifier.unlock(pending).map_err(dispatch_error)
    }
    fn node_changed(&self, args: Arc<ContextChangeArgument<E>>) -> Result<(), PointwiseError<E>> {
        if self.owner.scene_transform_3d().is_some()
            || self.owner.uv_texture_output().is_some()
            || self.owner.is_import_3d()
        {
            return self.owner.notify_property_change_with(args);
        }
        let primitive = self
            .owner
            .scene_primitive()
            .ok_or(PointwiseError::InvalidCast)?;
        if self.root.kind == ModernNodePropertyKind::Vector3D {
            primitive
                .state
                .transform_dirty
                .store(true, Ordering::Release);
        }
        self.owner.notify_property_change_with(args)?;
        if self.root.kind != ModernNodePropertyKind::Vector3D {
            primitive
                .state
                .geometry_dirty
                .store(true, Ordering::Release);
        }
        Ok(())
    }
    fn bounds(&self, values: &[f64]) -> bool {
        let limits = self.root.limits();
        values.iter().enumerate().all(|(i, value)| {
            let Some(limits) = limits.get(i) else {
                return false;
            };
            let limits = limits.snapshot();
            !(limits.minimum.to_f64() > *value || *value > limits.maximum.to_f64())
        })
    }
    pub fn set_double(&self, value: f64) -> Result<(), PointwiseError<E>> {
        let Backing::Double(core) = &self.root.backing else {
            return Err(PointwiseError::InvalidCast);
        };
        let _frame = self.clone();
        core.retain()
            .map_err(double_error)?
            .set_value(value)
            .map_err(double_error)
    }
    pub fn set_int32(&self, value: i32) -> Result<(), PointwiseError<E>> {
        if self.kind() != ModernNodePropertyKind::Int32 {
            return Err(PointwiseError::InvalidCast);
        }
        self.set_typed(ModernNodeValue::Int32(value), false, false, false)
    }
    pub fn set_boolean(&self, value: bool) -> Result<(), PointwiseError<E>> {
        if self.kind() != ModernNodePropertyKind::Boolean {
            return Err(PointwiseError::InvalidCast);
        }
        self.set_typed(ModernNodeValue::Boolean(value), false, false, false)
    }
    pub fn set_vector3d(&self, value: [f64; 3]) -> Result<(), PointwiseError<E>> {
        if self.kind() != ModernNodePropertyKind::Vector3D {
            return Err(PointwiseError::InvalidCast);
        }
        self.set_typed(ModernNodeValue::Vector3D(value), false, false, false)
    }
    pub fn set_temporary(
        &self,
        value: ModernNodeValue,
        temporary: bool,
    ) -> Result<(), PointwiseError<E>> {
        if let ModernNodeValue::Double(value) = value {
            let Backing::Double(core) = &self.root.backing else {
                return Err(PointwiseError::InvalidCast);
            };
            let _frame = self.clone();
            return core
                .retain()
                .map_err(double_error)?
                .set_temporary(value, temporary)
                .map_err(double_error);
        }
        self.set_typed(value, temporary, false, true)
    }
    pub fn set_speed(&self, value: ModernNodeValue) -> Result<(), PointwiseError<E>> {
        if let ModernNodeValue::Double(value) = value {
            let Backing::Double(core) = &self.root.backing else {
                return Err(PointwiseError::InvalidCast);
            };
            let _frame = self.clone();
            return core
                .retain()
                .map_err(double_error)?
                .set_speed(value)
                .map_err(double_error);
        }
        if matches!(value, ModernNodeValue::Boolean(_)) {
            return Err(PointwiseError::InvalidCast);
        }
        self.set_typed(value, false, true, false)
    }
    fn set_typed(
        &self,
        requested: ModernNodeValue,
        temporary: bool,
        speed: bool,
        explicit: bool,
    ) -> Result<(), PointwiseError<E>> {
        let _frame = self.clone();
        if !matches!(
            (self.kind(), requested),
            (ModernNodePropertyKind::Int32, ModernNodeValue::Int32(_))
                | (ModernNodePropertyKind::Boolean, ModernNodeValue::Boolean(_))
                | (
                    ModernNodePropertyKind::Vector3D,
                    ModernNodeValue::Vector3D(_)
                )
        ) {
            return Err(PointwiseError::InvalidCast);
        }
        let mut value = requested;
        match value {
            ModernNodeValue::Int32(v) => {
                let l = self
                    .component_limits(0)
                    .ok_or(PointwiseError::NullReference)?
                    .snapshot();
                let min = decimal_i32(l.minimum)?;
                let max = decimal_i32(l.maximum)?;
                if v < min || v > max {
                    return Ok(());
                }
            }
            ModernNodeValue::Vector3D(v) => {
                if !self.bounds(&v) {
                    return Ok(());
                }
                if !speed {
                    let record = self.root.record();
                    if let ModernPropertyValue::Vector { previous, .. } = record.value()
                        && previous
                            .iter()
                            .zip(v)
                            .all(|(a, b)| *a == b || a.is_nan() && b.is_nan())
                    {
                        return Ok(());
                    }
                }
                if !speed && !explicit {
                    let limits = self.root.limits();
                    let mut capped = v;
                    for i in 0..3 {
                        capped[i] =
                            crate::round_decimal_places(v[i], limits[i].snapshot().decimal_places)
                                .map_err(PointwiseError::DecimalRounding)?;
                    }
                    value = ModernNodeValue::Vector3D(capped);
                }
            }
            ModernNodeValue::Boolean(_) => {}
            ModernNodeValue::Double(_) => return Err(PointwiseError::InvalidCast),
        }
        let args = {
            let mut state = guard(self.root.state().ok_or(PointwiseError::InvalidCast)?);
            let name = state.record.name.clone();
            let adjusted = state
                .record
                .adjustment
                .as_ref()
                .is_some_and(|a| a.components().iter().any(|c| c.active));
            let pair = match (state.record.value_mut(), value) {
                (
                    ModernPropertyValue::Int32 { current, previous }
                    | ModernPropertyValue::Selected { current, previous },
                    ModernNodeValue::Int32(v),
                ) => {
                    if (if speed { *current } else { *previous }) == v {
                        return Ok(());
                    }
                    let old = ModernUndoValue::Int32(*previous);
                    *current = v;
                    (old, ModernUndoValue::Int32(v))
                }
                (ModernPropertyValue::Boolean(current), ModernNodeValue::Boolean(v)) => {
                    if *current == v {
                        return Ok(());
                    }
                    let old = ModernUndoValue::Boolean(*current);
                    *current = v;
                    (old, ModernUndoValue::Boolean(v))
                }
                (
                    ModernPropertyValue::Vector { current, previous },
                    ModernNodeValue::Vector3D(v),
                ) => {
                    let same = |a: [f64; 3], b: [f64; 3]| {
                        a.into_iter()
                            .zip(b)
                            .all(|(a, b)| a == b || a.is_nan() && b.is_nan())
                    };
                    if same(if speed { *current } else { *previous }, v) {
                        if !speed && !explicit {
                            *previous = *current;
                        }
                        return Ok(());
                    }
                    let old = ModernUndoValue::Vector3D(*previous);
                    *current = v;
                    (old, ModernUndoValue::Vector3D(v))
                }
                _ => return Err(PointwiseError::InvalidCast),
            };
            let undo = if speed || adjusted && !explicit {
                None
            } else {
                Some(Arc::new(ModernPropertyUndoInfo::new(
                    name, pair.0, pair.1, temporary,
                )))
            };
            Arc::new(ContextChangeArgument::Modern(
                ModernPropertyChangeArgs::new(speed, false, undo),
            ))
        };
        self.root.emit(args)?;
        if !speed {
            let mut state = guard(self.root.state().ok_or(PointwiseError::InvalidCast)?);
            match state.record.value_mut() {
                ModernPropertyValue::Int32 { current, previous }
                | ModernPropertyValue::Selected { current, previous } => *previous = *current,
                ModernPropertyValue::Vector { current, previous } => {
                    *previous = if explicit {
                        let ModernNodeValue::Vector3D(v) = requested else {
                            unreachable!()
                        };
                        v
                    } else {
                        *current
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Return the actual optional animation owner without invoking its lazy getter.
    pub fn retained_animation(&self) -> Option<ModernNodeAnimation<E>> {
        guard(&self.root.animation)
            .clone()
            .map(|animation| ModernNodeAnimation {
                owner: self.clone(),
                animation,
            })
    }

    pub(in crate::pointwise) fn is_animated(&self) -> Result<bool, PointwiseError<E>> {
        match guard(&self.root.animation).clone() {
            Some(animation) => Ok(animation.is_active()?),
            None => Ok(false),
        }
    }
    /// Source modern AnimationProperty getter; all returned aliases edit one root.
    pub fn animation(&self) -> Result<Option<ModernNodeAnimation<E>>, PointwiseError<E>> {
        if let Some(animation) = self.retained_animation() {
            return Ok(Some(animation));
        }
        let record = self.root.record();
        let selected_count = guard(&self.root.selected_options).as_ref().map(|v| v.len());
        if selected_count.is_none() && !record.animatable {
            return Ok(None);
        }
        let limits = self.root.limits();
        let animation = if let Some(count) = selected_count {
            crate::GradientAnimation::new(1, Some(selected_animation_limits(count)?))?
        } else if self.kind() == ModernNodePropertyKind::Vector3D {
            crate::GradientAnimation::with_limits_array(
                3,
                Some(crate::AnimationLimits::from_values(
                    limits.into_iter().map(Some).collect(),
                )),
            )?
        } else {
            crate::GradientAnimation::new(1, limits.first().cloned())?
        };
        let actual = {
            let mut root = guard(&self.root.animation);
            root.get_or_insert(animation).clone()
        };
        Ok(Some(ModernNodeAnimation {
            owner: self.clone(),
            animation: actual,
        }))
    }
    /// Exact Property<T>.AnimationProperty replacement, including source equality
    /// before Animatable and synchronous node-first ordinary callback delivery.
    pub fn set_animation(
        &self,
        animation: Option<&ModernNodeAnimation<E>>,
    ) -> Result<(), PointwiseError<E>> {
        self.replace_animation(animation.map(|a| a.animation.clone()))
    }
    pub(in crate::pointwise) fn replace_animation(
        &self,
        next: Option<crate::GradientAnimation<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let current = guard(&self.root.animation).clone();
        let changed = match current {
            Some(current) => !current.source_equals_object(
                next.as_ref()
                    .map_or(crate::SourceObject::Null, crate::SourceObject::Animation),
            )?,
            None => next.is_some(),
        };
        if !changed || !self.root.record().animatable {
            return Ok(());
        }
        let old = std::mem::replace(&mut *guard(&self.root.animation), next);
        let actual = guard(&self.root.animation).clone();
        let undo = crate::AnimationUndoInfo::replacement(self.name(), old, actual);
        self.root.emit(Arc::new(ContextChangeArgument::Animation(
            crate::AnimationPropertyChangeArgs::new(undo),
        )))
    }
    fn clear_animation_from_wire(&self) {
        let old = guard(&self.root.animation).take();
        drop(old);
    }

    pub fn evaluated_value(&self, time: f64) -> Result<ModernNodeValue, PointwiseError<E>> {
        let value = self.value();
        if self.kind() == ModernNodePropertyKind::Boolean {
            return Ok(value);
        }
        let animation = if self.kind() == ModernNodePropertyKind::Double {
            self.retained_animation()
        } else {
            self.animation()?
        };
        let Some(animation) = animation else {
            if self.kind() != ModernNodePropertyKind::Double {
                return Err(PointwiseError::NullReference);
            }
            return Ok(value);
        };
        if !animation.animation.is_active()? {
            return Ok(value);
        }
        let curve = |index: i32| -> Result<Option<crate::SharedCurve<E>>, PointwiseError<E>> {
            Ok(animation
                .animation
                .curves()
                .ok_or(PointwiseError::NullReference)?
                .value(index)?)
        };
        match value {
            ModernNodeValue::Int32(_) => {
                let value = f64::from(
                    curve(0)?
                        .ok_or(PointwiseError::NullReference)?
                        .sample(time as f32)?,
                );
                let selected_count = guard(&self.root.selected_options).as_ref().map(|v| v.len());
                let value = if let Some(count) = selected_count {
                    let index =
                        crate::matrix_coordinate_i32(value.round_ties_even()).wrapping_sub(1);
                    let max = i32::try_from(count - 1).map_err(|_| PointwiseError::Arithmetic)?;
                    index.clamp(0, max)
                } else {
                    crate::matrix_coordinate_i32(value)
                };
                Ok(ModernNodeValue::Int32(value))
            }
            ModernNodeValue::Double(_) => {
                let mut value = f64::from(
                    curve(0)?
                        .ok_or(PointwiseError::NullReference)?
                        .sample(time as f32)?,
                );
                if self
                    .component_limits(0)
                    .ok_or(PointwiseError::NullReference)?
                    .snapshot()
                    .decimal_places
                    == 0
                {
                    value = value.floor();
                }
                Ok(ModernNodeValue::Double(value))
            }
            ModernNodeValue::Vector3D(mut values) => {
                for (i, value) in values.iter_mut().enumerate() {
                    let active = animation
                        .animation
                        .flags()
                        .ok_or(PointwiseError::NullReference)?
                        .value(i as i32)
                        .map_err(|_| PointwiseError::Index)?;
                    if active && let Some(curve) = curve(i as i32)? {
                        *value = f64::from(curve.sample(time as f32)?);
                    }
                }
                Ok(ModernNodeValue::Vector3D(values))
            }
            ModernNodeValue::Boolean(_) => Ok(value),
        }
    }
    pub fn set_name(&self, name: Option<Vec<u16>>) -> Result<(), PointwiseError<E>> {
        if let Backing::Double(core) = &self.root.backing {
            return core
                .retain()
                .map_err(double_error)?
                .set_name(name)
                .map_err(double_error);
        }
        let retired = {
            let mut state = guard(self.root.state().expect("typed name backing"));
            if state.record.name == name {
                return Ok(());
            }
            std::mem::replace(&mut state.record.name, name)
        };
        drop(retired);
        self.root.emit(Arc::new(ContextChangeArgument::Modern(
            ModernPropertyChangeArgs::new(false, true, None),
        )))
    }
    fn refresh_adjustment_bounds(&self) -> Result<(), PointwiseError<E>> {
        let limits = self.root.limits();
        let selection_count = guard(&self.root.selected_options).as_ref().map(|v| v.len());
        let bounds = if let Some(count) = selection_count {
            vec![[0.5, count as f32 + 0.5]]
        } else if limits.is_empty() {
            vec![
                [0., 0.];
                if self.kind() == ModernNodePropertyKind::Vector3D {
                    3
                } else {
                    1
                }
            ]
        } else {
            limits
                .iter()
                .map(|l| {
                    let l = l.snapshot();
                    let pair = if let Some(c) = l.curve_range {
                        let c = c.copy_wire_value();
                        [c.minimum.to_framework_f64(), c.maximum.to_framework_f64()]
                    } else if let Some(r) = l.use_range {
                        let r = r.copy_wire_value();
                        if r.flags[1] {
                            [r.minimum.to_framework_f64(), r.maximum.to_framework_f64()]
                        } else {
                            [l.minimum.to_framework_f64(), l.maximum.to_framework_f64()]
                        }
                    } else {
                        return Err(PointwiseError::NullReference);
                    };
                    Ok([
                        if pair[0] < -1000. { -1000. } else { pair[0] } as f32,
                        if pair[1] > 1000. { 1000. } else { pair[1] } as f32,
                    ])
                })
                .collect::<Result<Vec<_>, PointwiseError<E>>>()?
        };
        match &self.root.backing {
            Backing::Double(core) => core.set_adjustment_bounds(bounds),
            _ => guard(self.root.state().expect("typed bounds backing"))
                .record
                .set_adjustment_bounds(bounds),
        }
        Ok(())
    }
    pub fn save_binary(&self, writer: &mut crate::BinaryWriter) -> Result<(), PointwiseError<E>> {
        let animation = self.retained_animation();
        let active = match &animation {
            Some(a) => a.animation.is_active()?,
            None => false,
        };
        writer
            .write_bool(active)
            .map_err(|e| PointwiseError::ModernProperty(e.into()))?;
        if active {
            animation
                .ok_or(PointwiseError::NullReference)?
                .animation
                .save(writer)?;
        }
        self.root
            .with_record(|r| r.save_tail(writer))
            .map_err(PointwiseError::ModernProperty)
    }
    pub fn load_binary(
        &self,
        reader: &mut crate::BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), PointwiseError<E>> {
        if version >= 3.72 || version == 3.71 || version == 3.7 {
            let animated = reader
                .read_bool()
                .map_err(|e| PointwiseError::ModernProperty(e.into()))?;
            if animated {
                let animation = self.animation()?.ok_or(PointwiseError::NullReference)?;
                animation.animation.load(reader, version)?;
            } else {
                self.clear_animation_from_wire();
            }
            let adjusted = reader
                .read_bool()
                .map_err(|e| PointwiseError::ModernProperty(e.into()))?;
            if adjusted && self.root.record().adjustment.is_none() {
                self.refresh_adjustment_bounds()?;
            }
            self.root
                .with_record(|r| r.load_adjustment_body(reader, version, adjusted))
                .map_err(PointwiseError::ModernProperty)?;
        }
        self.root
            .with_record(|r| r.load_value(reader, version))
            .map_err(PointwiseError::ModernProperty)
    }
}
/// Sponsored handle to the same real bh owner used by GetValue, wire and capture.
/// Child arrays/curves retain their own real roots; no owned record is an alias.
pub struct ModernNodeAnimation<E = ()> {
    owner: ModernNodeProperty<E>,
    pub(super) animation: crate::GradientAnimation<E>,
}
impl<E> Clone for ModernNodeAnimation<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            animation: self.animation.clone(),
        }
    }
}
impl<E: 'static> ModernNodeAnimation<E> {
    pub fn parent(&self) -> ModernNodeProperty<E> {
        self.owner.clone()
    }
    pub fn same_reference(&self, b: &Self) -> bool {
        self.animation.same_reference(&b.animation)
    }
    pub fn count(&self) -> i32 {
        self.animation.count()
    }
    pub fn flags(&self) -> Option<crate::AnimationFlags> {
        self.animation.flags()
    }
    pub fn curves(&self) -> Option<crate::AnimationCurves<E>> {
        self.animation.curves()
    }
    pub fn limits(&self) -> Option<crate::AnimationLimits> {
        self.animation.limits()
    }
    pub fn set_flags(&self, flags: Option<crate::AnimationFlags>) {
        self.animation.set_flags(flags);
    }
    pub fn set_curves(&self, curves: Option<crate::AnimationCurves<E>>) {
        self.animation.set_curves(curves);
    }
    pub fn retained_gradient(&self) -> Option<crate::SharedGradient> {
        self.animation.retained_gradient()
    }
    pub fn gradient(&self) -> Result<crate::SharedGradient, PointwiseError<E>> {
        Ok(self.animation.gradient()?)
    }
    pub fn set_gradient(&self, gradient: Option<crate::SharedGradient>) {
        self.animation.set_gradient(gradient);
    }
    pub fn ensure_curve(&self, index: i32) -> Result<(), PointwiseError<E>> {
        Ok(self.animation.ensure_curve(index)?)
    }
    pub fn with_curve_preset(
        &self,
        index: i32,
        preset: &crate::SharedCurve<E>,
        insert: bool,
    ) -> Result<Self, PointwiseError<E>> {
        Ok(Self {
            owner: self.owner.clone(),
            animation: self.animation.with_curve_preset(index, preset, insert)?,
        })
    }
}

#[cfg(test)]
mod source_wire_laws {
    use super::*;
    #[test]
    fn lazy_adjustment_creation_preserves_flag_cursor_and_animation_prefix() {
        let node = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let p = node.property(3).unwrap();
        let old = p.animation().unwrap().unwrap();
        {
            let mut state = guard(p.root.state().unwrap());
            let mut values = state.limits[0].snapshot();
            values.use_range = None;
            values.curve_range = None;
            state.limits[0] = SharedLimits::from_backing(values);
        }
        let mut reader = crate::BinaryCursor::new(&[]).unwrap();
        assert!(p.load_binary(&mut reader, 3.75).is_err());
        assert_eq!(reader.position().unwrap(), 0);
        assert!(p.retained_animation().unwrap().same_reference(&old));
        let mut wire = crate::BinaryWriter::default();
        wire.write_bool(false).unwrap();
        wire.write_bool(false).unwrap();
        wire.write_i32(4).unwrap();
        p.load_binary(&mut crate::BinaryCursor::new(wire.bytes()).unwrap(), 3.75)
            .unwrap();
        assert!(p.retained_animation().is_none());
        assert_eq!(p.value(), ModernNodeValue::Int32(4));
        let prior = p.animation().unwrap().unwrap();
        let mut legacy = crate::BinaryWriter::default();
        legacy.write_i32(5).unwrap();
        p.load_binary(&mut crate::BinaryCursor::new(legacy.bytes()).unwrap(), 3.69)
            .unwrap();
        assert_eq!(p.value(), ModernNodeValue::Int32(5));
        assert!(p.retained_animation().unwrap().same_reference(&prior));
        let prefix = [1, 0, 0, 1];
        let mut reader = crate::BinaryCursor::new(&prefix).unwrap();
        assert_eq!(
            p.load_binary(&mut reader, 3.75).unwrap_err(),
            PointwiseError::NullReference
        );
        assert_eq!(reader.position().unwrap(), 4);
        assert!(p.retained_animation().unwrap().same_reference(&prior));
        assert!(!prior.flags().unwrap().value(0).unwrap());
        assert_eq!(p.value(), ModernNodeValue::Int32(5));
        assert!(p.backing_snapshot().adjustment.is_none());
        let mut adjustment = crate::AdjustmentRecord::new(&[[-7., 9.]]);
        guard(p.root.state().unwrap()).record.adjustment = Some(adjustment.clone());
        let mut wire = crate::BinaryWriter::default();
        wire.write_bool(false).unwrap();
        wire.write_bool(true).unwrap();
        adjustment.save(&mut wire).unwrap();
        wire.write_i32(6).unwrap();
        p.load_binary(&mut crate::BinaryCursor::new(wire.bytes()).unwrap(), 3.75)
            .unwrap();
        assert_eq!(p.value(), ModernNodeValue::Int32(6));
        assert_eq!(
            p.backing_snapshot().adjustment.unwrap().components()[0].default_bounds(),
            [-7., 9.]
        );
    }
}

#[cfg(test)]
mod animation_setter_laws {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn source_equal_animation_keeps_identity_and_equality_precedes_animatable() {
        let node = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let p = node.property(3).unwrap();
        let old = p.animation().unwrap().unwrap();
        let clone = node
            .source_clone()
            .unwrap()
            .property(3)
            .unwrap()
            .retained_animation()
            .unwrap();
        assert!(!old.same_reference(&clone));
        let calls = Arc::new(AtomicUsize::new(0));
        let node_hit = calls.clone();
        node.node()
            .add_change_handler(crate::PointwiseNodeChangeHandler::new(move |_, _| {
                node_hit.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        let hit = calls.clone();
        p.add_change_handler(&ModernNodePropertyHandler::new(move |_, _| {
            hit.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        p.set_animation(Some(&clone)).unwrap();
        assert!(p.retained_animation().unwrap().same_reference(&old));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        guard(p.root.state().unwrap()).record.animatable = false;
        clone.flags().unwrap().set(0, true).unwrap();
        p.set_animation(Some(&clone)).unwrap();
        assert!(p.retained_animation().unwrap().same_reference(&old));
        old.set_flags(None);
        assert!(
            matches!(p.set_animation(Some(&clone)),Err(PointwiseError::Animation(e))if matches!(e.as_ref(),crate::AnimationError::NullReference))
        );
        assert!(p.retained_animation().unwrap().same_reference(&old));
        p.set_animation(None).unwrap();
        assert!(p.retained_animation().unwrap().same_reference(&old));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn animation_reentry_keeps_committed_owners_and_same_node_property_arguments() {
        let node = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let p = node.property(3).unwrap();
        let a = p.animation().unwrap().unwrap();
        let second = ScenePrimitiveNode::new(ScenePrimitiveKind::Cube)
            .unwrap()
            .property(3)
            .unwrap()
            .animation()
            .unwrap()
            .unwrap();
        second.flags().unwrap().set(0, true).unwrap();
        let third = ScenePrimitiveNode::new(ScenePrimitiveKind::Cube)
            .unwrap()
            .property(3)
            .unwrap()
            .animation()
            .unwrap()
            .unwrap();
        let queue = Arc::new(Mutex::new(Some((p.clone(), third.clone()))));
        let queued = queue.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let node_events = events.clone();
        node.node()
            .add_change_handler(crate::PointwiseNodeChangeHandler::new(move |_, args| {
                let ContextChangeArgument::Animation(change) = args.as_ref() else {
                    panic!("actual bh undo")
                };
                assert!(change.undo_info().temporary_change());
                change.undo_info().set_lab_undo(true);
                node_events.lock().unwrap().push(("node", args));
                let next = queued.lock().unwrap().take();
                if let Some((p, next)) = next {
                    let current = p.retained_animation().unwrap();
                    assert!(current.flags().unwrap().value(0).unwrap());
                    p.set_animation(Some(&next)).unwrap();
                }
                Ok(())
            }));
        let property_events = events.clone();
        p.add_change_handler(&ModernNodePropertyHandler::new(move |_, args| {
            let ContextChangeArgument::Animation(change) = args.as_ref() else {
                panic!("actual bh undo")
            };
            assert!(change.undo_info().is_lab_undo());
            property_events.lock().unwrap().push(("property", args));
            Ok(())
        }));
        p.set_animation(Some(&second)).unwrap();
        assert!(p.retained_animation().unwrap().same_reference(&third));
        let events = events.lock().unwrap();
        assert_eq!(
            events.iter().map(|(source, _)| *source).collect::<Vec<_>>(),
            ["node", "node", "property", "property"]
        );
        assert!(Arc::ptr_eq(&events[0].1, &events[3].1));
        assert!(Arc::ptr_eq(&events[1].1, &events[2].1));
        assert!(Arc::ptr_eq(
            &events[0].1.source_root(),
            &events[3].1.source_root()
        ));
        let ContextChangeArgument::Animation(outer) = events[0].1.as_ref() else {
            unreachable!()
        };
        let ContextChangeArgument::Animation(inner) = events[1].1.as_ref() else {
            unreachable!()
        };
        assert_eq!(
            outer.undo_info().name().unwrap(),
            "X Segments".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(outer.undo_info().property_change(), 13);
        assert!(!outer.speed_rendering());
        assert!(!outer.update_controls_only());
        assert!(
            outer
                .undo_info()
                .old_value()
                .unwrap()
                .same_reference(&a.animation)
        );
        assert!(
            outer
                .undo_info()
                .new_value()
                .unwrap()
                .same_reference(&second.animation)
        );
        assert!(
            inner
                .undo_info()
                .old_value()
                .unwrap()
                .same_reference(&second.animation)
        );
        assert!(
            inner
                .undo_info()
                .new_value()
                .unwrap()
                .same_reference(&third.animation)
        );
    }
    #[test]
    fn animation_replace_and_clear_commit_before_typed_errors_and_undo_has_no_graph_backedge() {
        for stop_node in [true, false] {
            let node = ScenePrimitiveNode::<&'static str>::new(ScenePrimitiveKind::Cube).unwrap();
            let p = node.property(3).unwrap();
            let old = if stop_node {
                Some(p.animation().unwrap().unwrap())
            } else {
                None
            };
            let next = if stop_node {
                None
            } else {
                Some(
                    ScenePrimitiveNode::new(ScenePrimitiveKind::Cube)
                        .unwrap()
                        .property(3)
                        .unwrap()
                        .animation()
                        .unwrap()
                        .unwrap(),
                )
            };
            let expected_old = old.as_ref().map(|a| a.animation.clone());
            let expected_new = next.as_ref().map(|a| a.animation.clone());
            let events = Arc::new(Mutex::new(Vec::new()));
            let node_events = events.clone();
            node.node()
                .add_change_handler(crate::PointwiseNodeChangeHandler::new(move |_, args| {
                    node_events.lock().unwrap().push(("node", args));
                    if stop_node { Err("node stop") } else { Ok(()) }
                }));
            let property_events = events.clone();
            p.add_change_handler(&ModernNodePropertyHandler::new(move |_, args| {
                property_events.lock().unwrap().push(("property", args));
                Err("property stop")
            }));
            node.state.geometry_dirty.store(false, Ordering::Release);
            assert_eq!(
                p.set_animation(next.as_ref()).unwrap_err(),
                PointwiseError::Observer(if stop_node {
                    "node stop"
                } else {
                    "property stop"
                })
            );
            let retained = p.retained_animation();
            assert_eq!(retained.is_some(), expected_new.is_some());
            if let (Some(retained), Some(expected)) = (&retained, &expected_new) {
                assert!(retained.animation.same_reference(expected));
            }
            assert_eq!(
                node.state.geometry_dirty.load(Ordering::Acquire),
                !stop_node
            );
            let events = events.lock().unwrap();
            assert_eq!(events.len(), if stop_node { 1 } else { 2 });
            let ContextChangeArgument::Animation(args) = events[0].1.as_ref() else {
                unreachable!()
            };
            let undo = args.undo_info().clone();
            assert!(undo.temporary_change());
            assert!(!undo.is_lab_undo());
            assert_eq!(undo.old_value().is_some(), expected_old.is_some());
            assert_eq!(undo.new_value().is_some(), expected_new.is_some());
            if let (Some(actual), Some(expected)) = (undo.old_value(), expected_old) {
                assert!(actual.same_reference(&expected));
            }
            if let (Some(actual), Some(expected)) = (undo.new_value(), expected_new) {
                assert!(actual.same_reference(&expected));
            }
            let graph = guard(&p.root.parent).clone();
            drop(events);
            drop(retained);
            drop(old);
            drop(next);
            drop(p);
            drop(node);
            assert!(graph.upgrade().is_none());
            assert!(undo.old_value().is_some() || undo.new_value().is_some());
        }
    }
    #[test]
    fn false_animation_wire_flag_clears_silently_before_next_flag_failure() {
        let node = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let p = node.property(3).unwrap();
        p.animation().unwrap().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let hit = calls.clone();
        node.node()
            .add_change_handler(crate::PointwiseNodeChangeHandler::new(move |_, _| {
                hit.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        let hit = calls.clone();
        p.add_change_handler(&ModernNodePropertyHandler::new(move |_, _| {
            hit.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let mut reader = crate::BinaryCursor::new(&[0]).unwrap();
        assert!(p.load_binary(&mut reader, 3.75).is_err());
        assert_eq!(reader.position().unwrap(), 1);
        assert!(p.retained_animation().is_none());
        assert_eq!(p.value(), ModernNodeValue::Int32(1));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        p.set_animation(None).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
