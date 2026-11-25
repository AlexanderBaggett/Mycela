//! Actual aoi stream shared by wu and the existing ku/Histogram authority.
use super::*;
use std::sync::{
    Weak,
    atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
pub(in crate::pointwise) struct MatrixBinding<E> {
    pub(super) id: u64,
    pub(super) node: Weak<super::super::super::graph::NodeCore<E>>,
    pub(super) parameter: usize,
}
enum Payload<E> {
    Matrix(MatrixKernel),
    LightPosition(Mutex<[f64; 2]>),
    LegacyToneCurve(Mutex<[f32; 3]>),
    LegacyGradient(Mutex<crate::LegacyColorGradient>),
    Histogram(Histogram),
    Region(region_object::RegionState<E>),
    Imported(imported_image::ImportedState),
}
struct Anchor<E> {
    owners: Mutex<Vec<(u64, super::super::super::PointwiseNode<E>)>>,
}
struct Body<E> {
    id: u64,
    payload: Payload<E>,
    listeners: Mutex<Vec<Listener<E>>>,
    anchor: Mutex<Weak<Anchor<E>>>,
    imported_route: Mutex<Weak<imported_region::Region<E>>>,
}
enum Listener<E> {
    Property(Arc<MatrixBinding<E>>),
    Host(PointwiseMatrixHandler<E>),
}
impl<E> Clone for Listener<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Property(b) => Self::Property(b.clone()),
            Self::Host(h) => Self::Host(h.clone()),
        }
    }
}
/// Real typed aoi object, not a fabricated Other discriminator. Clone retains
/// object identity; only outward handles carry the external sponsorship anchor.
pub struct PointwiseCustomObject<E = ()> {
    body: Arc<Body<E>>,
    anchor: Option<Arc<Anchor<E>>>,
    imported_region: Option<Arc<imported_region::Region<E>>>,
}
impl<E> Clone for PointwiseCustomObject<E> {
    fn clone(&self) -> Self {
        Self {
            body: self.body.clone(),
            anchor: self.anchor.clone(),
            imported_region: self.imported_region.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseCustomObject<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseCustomObject({:p})", Arc::as_ptr(&self.body))
    }
}
pub struct PointwiseMatrix<E = ()> {
    object: PointwiseCustomObject<E>,
    kernel: MatrixKernel,
}
impl<E> Clone for PointwiseMatrix<E> {
    fn clone(&self) -> Self {
        Self {
            object: self.object.clone(),
            kernel: self.kernel.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseMatrix<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseMatrix({:?})", self.object)
    }
}
type MatrixCallback<E> = dyn Fn(&PointwiseCustomObject<E>, Arc<crate::ContextChangeArgument<E>>) -> Result<(), E>
    + Send
    + Sync;
pub struct PointwiseMatrixHandler<E>(Arc<MatrixCallback<E>>);
impl<E> Clone for PointwiseMatrixHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> PointwiseMatrixHandler<E> {
    pub fn new(
        f: impl Fn(&PointwiseCustomObject<E>, Arc<crate::ContextChangeArgument<E>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(f))
    }
    pub fn same_binding(&self, h: &Self) -> bool {
        Arc::ptr_eq(&self.0, &h.0)
    }
}
impl<E> PointwiseCustomObject<E> {
    fn from_payload(payload: Payload<E>) -> Self {
        let anchor = Arc::new(Anchor {
            owners: Mutex::new(Vec::new()),
        });
        let imported = matches!(&payload, Payload::Imported(_));
        let body = Arc::new(Body {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            payload,
            listeners: Mutex::new(Vec::new()),
            anchor: Mutex::new(Arc::downgrade(&anchor)),
            imported_route: Mutex::new(Weak::new()),
        });
        let imported_region = imported.then(|| imported_region::Region::publish(&body));
        Self {
            body,
            anchor: Some(anchor),
            imported_region,
        }
    }
    /// Source gg.e/aob. Field mutation is silent; notify uses the aoi stream.
    pub fn new_light_position() -> Self {
        Self::from_payload(Payload::LightPosition(Mutex::new([1.0, 1.0])))
    }
    pub fn is_light_position(&self) -> bool {
        matches!(&self.body.payload, Payload::LightPosition(_))
    }
    pub fn light_position(&self) -> Result<[f64; 2], PointwiseError<E>> {
        match &self.body.payload {
            Payload::LightPosition(fields) => Ok(*guard(fields)),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub(in crate::pointwise) fn update_light_position(
        &self,
        before: [f64; 2],
        after: [f64; 2],
    ) -> Result<bool, PointwiseError<E>> {
        let Payload::LightPosition(fields) = &self.body.payload else {
            return Err(PointwiseError::InvalidCast);
        };
        let mut position = guard(fields);
        if position.map(f64::to_bits) != before.map(f64::to_bits) {
            return Err(ImageError::Argument.into());
        }
        let changed = before.map(f64::to_bits) != after.map(f64::to_bits);
        *position = after;
        Ok(changed)
    }
    pub fn light_position_x(&self) -> Result<f64, PointwiseError<E>> {
        match &self.body.payload {
            Payload::LightPosition(fields) => Ok(guard(fields)[0]),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn light_position_y(&self) -> Result<f64, PointwiseError<E>> {
        match &self.body.payload {
            Payload::LightPosition(fields) => Ok(guard(fields)[1]),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn set_light_position_x(&self, value: f64) -> Result<(), PointwiseError<E>> {
        match &self.body.payload {
            Payload::LightPosition(fields) => {
                guard(fields)[0] = value;
                Ok(())
            }
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn set_light_position_y(&self, value: f64) -> Result<(), PointwiseError<E>> {
        match &self.body.payload {
            Payload::LightPosition(fields) => {
                guard(fields)[1] = value;
                Ok(())
            }
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn new_histogram() -> Self {
        Self::from_payload(Payload::Histogram(Histogram::new()))
    }
    pub(in crate::pointwise) fn new_legacy_tone_curve(fields: [f32; 3]) -> Self {
        Self::from_payload(Payload::LegacyToneCurve(Mutex::new(fields)))
    }
    pub(in crate::pointwise) fn new_legacy_gradient(value: crate::LegacyColorGradient) -> Self {
        Self::from_payload(Payload::LegacyGradient(Mutex::new(value)))
    }
    pub(in crate::pointwise) fn legacy_gradient(
        &self,
    ) -> Result<crate::LegacyColorGradient, PointwiseError<E>> {
        match &self.body.payload {
            Payload::LegacyGradient(value) => Ok(guard(value).clone()),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn legacy_color_gradient_summary(&self) -> Option<(i32, usize)> {
        match &self.body.payload {
            Payload::LegacyGradient(value) => {
                let gradient = guard(value);
                Some((gradient.steps(), gradient.stops().len()))
            }
            _ => None,
        }
    }
    pub(in crate::pointwise) fn load_legacy_gradient(
        &self,
        read: &mut crate::BinaryCursor<'_>,
    ) -> Result<(), crate::BinaryError> {
        let Payload::LegacyGradient(value) = &self.body.payload else {
            unreachable!("legacy gradient dispatch")
        };
        guard(value).read(read)
    }
    pub(in crate::pointwise) fn legacy_tone_curve(&self) -> Result<[f32; 3], PointwiseError<E>> {
        match &self.body.payload {
            Payload::LegacyToneCurve(fields) => Ok(*guard(fields)),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub(in crate::pointwise) fn load_legacy_tone_curve(
        &self,
        read: &mut crate::BinaryCursor<'_>,
    ) -> Result<(), crate::BinaryError> {
        let Payload::LegacyToneCurve(fields) = &self.body.payload else {
            unreachable!("legacy curve dispatch")
        };
        for field in guard(fields).iter_mut() {
            *field = read.read_f32()?;
        }
        Ok(())
    }
    pub fn histogram(&self) -> Option<Histogram> {
        match &self.body.payload {
            Payload::Histogram(h) => Some(h.clone()),
            _ => None,
        }
    }
    pub fn matrix(&self) -> Result<PointwiseMatrix<E>, PointwiseError<E>> {
        match &self.body.payload {
            Payload::Matrix(kernel) => Ok(PointwiseMatrix {
                object: self.clone(),
                kernel: kernel.clone(),
            }),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn imported_image(&self) -> Result<PointwiseImportedImage<E>, PointwiseError<E>> {
        match &self.body.payload {
            Payload::Imported(_) => Ok(PointwiseImportedImage::from_object(self.clone())),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.body, &other.body)
    }
    pub(in crate::pointwise) fn raw(&self) -> Self {
        Self {
            body: self.body.clone(),
            anchor: None,
            imported_region: self.imported_region.clone(),
        }
    }
    pub(in crate::pointwise) fn source_clone(&self) -> Result<Self, PointwiseError<E>> {
        Ok(match &self.body.payload {
            Payload::Matrix(k) => Self::from_payload(Payload::Matrix(
                k.source_clone().map_err(matrix_error)?.internal_handle(),
            )),
            Payload::Histogram(h) => Self::from_payload(Payload::Histogram(h.source_clone())),
            Payload::LightPosition(v) => {
                let x = guard(v)[0];
                let y = guard(v)[1];
                Self::from_payload(Payload::LightPosition(Mutex::new([x, y])))
            }
            Payload::LegacyToneCurve(fields) => Self::new_legacy_tone_curve(*guard(fields)),
            Payload::LegacyGradient(value) => Self::new_legacy_gradient(guard(value).clone()),
            Payload::Region(_) => Self::from_payload(Payload::Region(
                region_object::RegionState::copied(self.region()?.fields()),
            )),
            Payload::Imported(_) => self.imported_image()?.source_clone().custom_object(),
        })
    }
    pub(super) fn subscriptions(&self) -> Vec<Arc<MatrixBinding<E>>> {
        guard(&self.body.listeners)
            .iter()
            .filter_map(|l| match l {
                Listener::Property(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }
    /// Actual property owner roots reached through this custom body and its
    /// shared imported container's retained dimension receivers. Graph sponsors
    /// merge these relationships instead of putting strong Arc backedges in aoi.
    pub(in crate::pointwise) fn owner_nodes(&self) -> Vec<crate::PointwiseNode<E>> {
        let mut bindings = self.subscriptions();
        if let Payload::Imported(_) = &self.body.payload
            && let Ok(image) = self.imported_image()
            && let Some(container) = image.container()
        {
            for receiver in container.dimension_listeners() {
                bindings.extend(receiver.subscriptions());
            }
        }
        if let Some(binding) = self.region_binding() {
            bindings.push(binding);
        }
        bindings
            .into_iter()
            .filter_map(|binding| {
                let core = binding.node.upgrade()?;
                crate::PointwiseNode::from_core(&core)
            })
            .collect()
    }
    pub(in crate::pointwise) fn subscribe(&self, binding: Arc<MatrixBinding<E>>) {
        guard(&self.body.listeners).push(Listener::Property(binding));
    }
    pub(in crate::pointwise) fn bind_node(
        owner: &crate::PointwiseNode<E>,
        parameter: usize,
    ) -> Arc<MatrixBinding<E>> {
        Arc::new(MatrixBinding {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            node: Arc::downgrade(&owner.core()),
            parameter,
        })
    }
    pub(super) fn bind(owner: &PointwiseParameter<E>) -> Arc<MatrixBinding<E>> {
        Arc::new(MatrixBinding {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            node: Arc::downgrade(&owner.owner.node().core()),
            parameter: owner.ix,
        })
    }
    pub(in crate::pointwise) fn attach_route(&self, b: &Arc<MatrixBinding<E>>)
    where
        E: 'static,
    {
        if let Some(core) = b.node.upgrade()
            && let Some(frame) = super::super::super::PointwiseNode::from_core(&core)
        {
            let anchor = guard(&self.body.anchor).upgrade();
            if let Some(anchor) = anchor {
                let old = {
                    let mut owners = guard(&anchor.owners);
                    if let Some(i) = owners.iter().position(|(id, _)| *id == b.id) {
                        Some(std::mem::replace(&mut owners[i].1, frame))
                    } else {
                        owners.push((b.id, frame));
                        None
                    }
                };
                drop(old);
            }
        }
        if let Payload::Matrix(kernel) = &self.body.payload {
            let weak = b.node.clone();
            kernel.add_owner_route(
                b.id,
                Arc::new(move || {
                    let core = weak.upgrade()?;
                    let frame = super::super::super::PointwiseNode::from_core(&core)?;
                    Some(Arc::new(frame) as Arc<dyn std::any::Any + Send + Sync>)
                }),
            );
        }
    }
    pub(in crate::pointwise) fn external(&self) -> Self {
        let anchor = {
            let mut weak = guard(&self.body.anchor);
            match weak.upgrade() {
                Some(a) => a,
                None => {
                    let a = Arc::new(Anchor {
                        owners: Mutex::new(Vec::new()),
                    });
                    *weak = Arc::downgrade(&a);
                    a
                }
            }
        };
        let mut bindings = self.subscriptions();
        if let Some(binding) = self.region_binding() {
            bindings.push(binding);
        }
        let owners = bindings
            .into_iter()
            .filter_map(|b| {
                let core = b.node.upgrade()?;
                let frame = super::super::super::PointwiseNode::from_core(&core)?;
                Some((b.id, frame))
            })
            .collect();
        let old = std::mem::replace(&mut *guard(&anchor.owners), owners);
        drop(old);
        Self {
            body: self.body.clone(),
            anchor: Some(anchor),
            imported_region: self
                .imported_region
                .clone()
                .or_else(|| imported_region::Region::for_body(&self.body)),
        }
    }
    pub(in crate::pointwise) fn unsubscribe(&self, b: &Arc<MatrixBinding<E>>) {
        let old = {
            let mut listeners = guard(&self.body.listeners);
            listeners
                .iter()
                .rposition(|l| matches!(l,Listener::Property(actual)if Arc::ptr_eq(actual,b)))
                .map(|i| listeners.remove(i))
        };
        let anchor = guard(&self.body.anchor).upgrade();
        if let Some(anchor) = anchor {
            let old = {
                let mut owners = guard(&anchor.owners);
                owners
                    .iter()
                    .position(|(id, _)| *id == b.id)
                    .map(|i| owners.remove(i))
            };
            drop(old);
        }
        if let Payload::Matrix(k) = &self.body.payload {
            k.remove_owner_route(b.id);
        }
        drop(old);
    }
    pub fn add_handler(&self, h: PointwiseMatrixHandler<E>) {
        guard(&self.body.listeners).push(Listener::Host(h));
    }
    pub fn remove_handler(&self, h: &PointwiseMatrixHandler<E>) {
        let old = {
            let mut v = guard(&self.body.listeners);
            v.iter()
                .rposition(|l| matches!(l,Listener::Host(actual)if actual.same_binding(h)))
                .map(|i| v.remove(i))
        };
        drop(old);
    }
    pub fn emit(&self, args: Arc<crate::ContextChangeArgument<E>>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        enum Invocation<E> {
            Property(PointwiseParameter<E>),
            Generator(crate::ImageGeneratorParameter<E>),
            Host(PointwiseMatrixHandler<E>),
        }
        let listeners = guard(&self.body.listeners).clone();
        // CLR multicast snapshot retains EVERY receiver before the first callback.
        let mut invocation = Vec::with_capacity(listeners.len());
        for listener in listeners {
            invocation.push(match listener {
                Listener::Host(h) => Invocation::Host(h),
                Listener::Property(b) => {
                    let core = b.node.upgrade().ok_or(PointwiseError::InvalidOperation)?;
                    let node = super::super::super::PointwiseNode::from_core(&core)
                        .ok_or(PointwiseError::InvalidOperation)?;
                    if let Some(filter) = node.filter() {
                        Invocation::Property(filter.parameter(b.parameter as i32)?)
                    } else if let Some(generator) = node.image_generator_handle() {
                        if generator.kind() != crate::ImageGeneratorKind::ImportedImage {
                            return Err(PointwiseError::InvalidCast);
                        }
                        Invocation::Generator(
                            generator.parameter(crate::ImageGeneratorParameterKind::Image),
                        )
                    } else {
                        return Err(PointwiseError::NullReference);
                    }
                }
            });
        }
        for listener in &invocation {
            match listener{Invocation::Host(h)=>(h.0)(self,args.clone()).map_err(PointwiseError::Observer)?,Invocation::Property(p)=>p.matrix_event(matches!(args.as_ref(),crate::ContextChangeArgument::Property(a)if a.speed_rendering()))?,Invocation::Generator(p)=>p.imported_custom_event(matches!(args.as_ref(),crate::ContextChangeArgument::Property(a)if a.speed_rendering()))?}
        }
        Ok(())
    }
    /// Actual caller-host membership, retaining duplicate ordered bindings.
    /// Internal property receiver bindings stay a separate existing authority.
    pub fn host_handlers(&self) -> Vec<PointwiseMatrixHandler<E>> {
        guard(&self.body.listeners)
            .iter()
            .filter_map(|listener| match listener {
                Listener::Host(handler) => Some(handler.clone()),
                Listener::Property(_) => None,
            })
            .collect()
    }
    pub fn notify(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.emit(crate::ContextChangeArgument::shared_empty())
    }
    pub fn notify_speed(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.emit(speed())
    }
    pub fn copy_and_notify(&self, source: Option<&Self>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let source = source.ok_or(PointwiseError::NullReference)?;
        match &self.body.payload {
            Payload::Matrix(k) => {
                let source = source.matrix()?;
                k.copy_from(Some(&source.kernel)).map_err(matrix_error)?;
            }
            Payload::Region(_) => {
                let fields = source.region()?.fields();
                self.region()?.set_mode(fields.0);
                self.region()?.set_x(fields.1);
                self.region()?.set_y(fields.2);
                self.region()?.set_size(fields.3);
            }
            Payload::LightPosition(fields) => {
                let x = source.light_position_x()?;
                guard(fields)[0] = x;
                let y = source.light_position_y()?;
                guard(fields)[1] = y;
            }
            Payload::LegacyToneCurve(fields) => {
                let copy = source.legacy_tone_curve()?;
                *guard(fields) = copy;
            }
            Payload::LegacyGradient(value) => {
                let copied = source.legacy_gradient()?;
                *guard(value) = copied;
            }
            Payload::Histogram(_) => {
                return Err(PointwiseError::UnsupportedCapability(
                    "ku public copy/event command",
                ));
            }
            Payload::Imported(_) => {
                let source = source.imported_image()?;
                self.imported_image()?.copy_fields_from(&source);
            }
        };
        self.notify()
    }
    pub(in super::super) fn captured_matrix(kernel: MatrixKernel) -> Self {
        Self::from_payload(Payload::Matrix(kernel.internal_handle()))
    }
    pub(in super::super) fn captured_histogram(h: Histogram) -> Self {
        Self::from_payload(Payload::Histogram(h))
    }
}
#[path = "region_object.rs"]
mod region_object;
pub use region_object::PointwiseRegion;
#[path = "imported_image.rs"]
mod imported_image;
#[path = "imported_region.rs"]
mod imported_region;
pub(in crate::pointwise) use imported_image::CapturedJpeg;
pub use imported_image::{
    ImportedImageContainer, ImportedImageDecodeError, ImportedImageReporting,
    PointwiseImportedImage,
};
impl<E> PointwiseMatrix<E> {
    pub fn new() -> Result<Self, PointwiseError<E>> {
        let kernel = MatrixKernel::new().map_err(matrix_error)?;
        let object = PointwiseCustomObject::from_payload(Payload::Matrix(kernel.internal_handle()));
        Ok(Self { object, kernel })
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.object.same_reference(&other.object)
    }
    pub(in super::super) fn captured_payload(&self) -> MatrixKernel {
        self.kernel.internal_handle()
    }
    pub fn payload(&self) -> MatrixKernel
    where
        E: 'static,
    {
        let weak = Arc::downgrade(&self.object.body);
        self.kernel.add_owner_route(
            self.object.body.id,
            Arc::new(move || {
                weak.upgrade()
                    .map(|body| body as Arc<dyn std::any::Any + Send + Sync>)
            }),
        );
        for b in self.object.subscriptions() {
            self.object.attach_route(&b);
        }
        self.kernel.external_handle()
    }
    pub fn custom_object(&self) -> PointwiseCustomObject<E> {
        self.object.clone()
    }
    pub fn add_handler(&self, h: PointwiseMatrixHandler<E>) {
        self.object.add_handler(h)
    }
    pub fn remove_handler(&self, h: &PointwiseMatrixHandler<E>) {
        self.object.remove_handler(h)
    }
    pub fn notify(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object.notify()
    }
    pub fn notify_speed(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object.notify_speed()
    }
    pub fn copy_and_notify(&self, source: Option<&Self>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object.copy_and_notify(source.map(|s| &s.object))
    }
}
