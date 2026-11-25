use crate::color_region::{
    CoreId, CoreKind, CoreRoute, DispatchLease, Member, SponsorAnchor, StoredColorValue, merge,
};
use crate::pointwise::property::wire::LegacyWireMetadata;
use crate::{
    BinaryError, ChangeDelegate, ChangeDispatchError, ChangeHandler, ChangeNotifier,
    ContextChangeArgument, ContextPropertyChangeArgs, DecimalArithmeticError, DecimalBits,
    DecimalRoundingError, ImageError, ImageMap, LimitsDefinition, LimitsStep, RangeOptionsRecord,
    SharedLimits, SharedRangeOptionsRecord, SolidColorChannelError, SolidColorProperty,
    SolidColorWireRecord, UndoObjectValue, round_decimal_places,
};
#[path = "live_solid_wire.rs"]
mod wire;
use crate::{GradientAnimation, PointwiseError, RenderCaptureSession};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex, Weak},
};

/// The two characterized kx(gz) constructor contexts, not the kx copy constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SolidColorCloneMode {
    Original,
    Clone,
}

struct State<E> {
    // Actual live stored values. Wire IO reads these owners directly.
    color: [f64; 3],
    transparency: f64,
    wire: [LegacyWireMetadata<E>; 2],
    animations: [Option<GradientAnimation<E>>; 2],
    color_apv: Option<CoreId>,
    previous: f64,
    normalized: f64,
    modifier: f64,
    allow: [bool; 2],
}
// Ordinary handle cloning retains roots. Source Clone and capture below apply
// their distinct policies explicitly, after releasing the owner guard.
impl<E> Clone for State<E> {
    fn clone(&self) -> Self {
        Self {
            color: self.color,
            transparency: self.transparency,
            wire: self.wire.clone(),
            animations: self.animations.clone(),
            color_apv: self.color_apv,
            previous: self.previous,
            normalized: self.normalized,
            modifier: self.modifier,
            allow: self.allow,
        }
    }
}
pub(crate) struct Inner<E> {
    pub(crate) route: CoreRoute<E>,
    mode: SolidColorCloneMode,
    state: Mutex<State<E>>,
    limits: [SharedLimits; 2],
    node_changed: ChangeNotifier<Envelope<E>, SolidColorCallbackError<E>>,
    property_changed: [ChangeNotifier<Envelope<E>, SolidColorCallbackError<E>>; 2],
    forwarder: ChangeHandler<Envelope<E>, SolidColorCallbackError<E>>,
}
struct Envelope<E> {
    owner: Weak<Inner<E>>,
    property: Option<SolidColorProperty>,
    args: Option<Arc<ContextChangeArgument<E>>>,
    anchor: Weak<SponsorAnchor<E>>,
}

/// Retained legacy Solid Color node with actual stored RGB/transparency,
/// property metadata, lazy animation/adjustment children and ordinary events.
/// Wire IO preserves these owners. Local animation samples the retained
/// curves; positive bindings and attached history still need a host context.
pub struct SharedSolidColorNode<E>(pub(crate) Arc<Inner<E>>, pub(crate) Arc<SponsorAnchor<E>>);
/// A public property handle retains its actual parent node and sibling roots.
/// RGB getters copy the source value fields; there is no Color previous value.
pub struct SharedSolidColorProperty<E> {
    owner: Arc<Inner<E>>,
    kind: SolidColorProperty,
    anchor: Arc<SponsorAnchor<E>>,
}
impl<E> SharedSolidColorProperty<E> {
    pub(crate) fn from_binding(
        owner: Arc<Inner<E>>,
        kind: SolidColorProperty,
        anchor: Arc<SponsorAnchor<E>>,
    ) -> Self {
        Self {
            owner,
            kind,
            anchor,
        }
    }
    pub(crate) fn binding_anchor(&self) -> &Arc<SponsorAnchor<E>> {
        &self.anchor
    }
    pub(crate) fn binding_locator(&self) -> (crate::color_region::CoreId, SolidColorProperty) {
        (self.owner.route.id, self.kind)
    }
}
impl<E> Clone for SharedSolidColorNode<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0), Arc::clone(&self.1))
    }
}
impl<E> Clone for SharedSolidColorProperty<E> {
    fn clone(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
            kind: self.kind,
            anchor: Arc::clone(&self.anchor),
        }
    }
}
impl<E> fmt::Debug for SharedSolidColorNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SharedSolidColorNode({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E> fmt::Debug for SharedSolidColorProperty<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SharedSolidColorProperty({:p}, {:?})",
            Arc::as_ptr(&self.owner),
            self.kind
        )
    }
}

#[derive(Clone, Debug)]
pub struct SolidColorNodeSnapshot {
    pub mode: SolidColorCloneMode,
    pub color: [f64; 3],
    pub transparency: f64,
}
#[derive(Clone, Debug)]
pub struct SolidColorPropertySnapshot {
    pub kind: SolidColorProperty,
    pub name: Vec<u16>,
    pub color: [f64; 3],
    pub current: f64,
    pub previous: f64,
    pub normalized: f64,
    pub modifier: f64,
    pub allow: bool,
    pub limits: SharedLimits,
    pub animation_present: bool,
    pub adjustment_present: bool,
}

/// Typed synchronous observer failures, including the actual node dispatch
/// failure propagated through the constructor's first property binding.
#[derive(Debug)]
pub enum SolidColorCallbackError<E> {
    Observer(E),
    Node(Box<ChangeDispatchError<SolidColorCallbackError<E>>>),
    SenderReleased,
}
#[derive(Debug)]
pub enum LiveSolidColorError<E> {
    Kind,
    NullColor,
    NumberCast,
    Binary(BinaryError),
    Arithmetic(DecimalArithmeticError),
    Rounding(DecimalRoundingError),
    Channel(SolidColorChannelError),
    Notification(ChangeDispatchError<SolidColorCallbackError<E>>),
    Image(ImageError),
    Property(PointwiseError<E>),
}
impl<E> SolidColorCallbackError<E> {
    pub fn source_class_with(&self, observer: impl Fn(&E) -> &'static str) -> &'static str {
        self.class(&observer)
    }
    fn class(&self, observer: &impl Fn(&E) -> &'static str) -> &'static str {
        match self {
            Self::Observer(error) => observer(error),
            Self::Node(error) => dispatch_class(error, observer),
            Self::SenderReleased => "InvalidOperationException",
        }
    }
}
impl<E> LiveSolidColorError<E> {
    pub fn source_class_with(&self, observer: impl Fn(&E) -> &'static str) -> &'static str {
        match self {
            Self::Kind => "InvalidOperationException",
            Self::NullColor => "NullReferenceException",
            Self::NumberCast => "InvalidCastException",
            Self::Binary(error) => error.source_class(),
            Self::Arithmetic(error) => error.source_class(),
            Self::Rounding(error) => error.source_class(),
            Self::Channel(error) => error.source_class(),
            Self::Notification(error) => dispatch_class(error, &observer),
            Self::Image(error) => error.source_class(),
            Self::Property(error) => match error {
                PointwiseError::Observer(e) => observer(e),
                _ => error.source_class(),
            },
        }
    }
}
fn dispatch_class<E>(
    error: &ChangeDispatchError<SolidColorCallbackError<E>>,
    observer: &impl Fn(&E) -> &'static str,
) -> &'static str {
    match error {
        ChangeDispatchError::Framework(error) => error.source_class(),
        ChangeDispatchError::Handler(error) => error.class(observer),
    }
}
impl<E: fmt::Display> fmt::Display for SolidColorCallbackError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observer(e) => write!(f, "Solid Color observer: {e}"),
            Self::Node(e) => write!(f, "Solid Color node forwarding: {e}"),
            Self::SenderReleased => f.write_str("Solid Color sender was released"),
        }
    }
}
impl<E: Error + 'static> Error for SolidColorCallbackError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observer(e) => Some(e),
            Self::Node(e) => Some(e.as_ref()),
            Self::SenderReleased => None,
        }
    }
}
impl<E: fmt::Display> fmt::Display for LiveSolidColorError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kind => f.write_str("operation does not match the Solid Color property kind"),
            Self::NullColor => f.write_str("null Color object"),
            Self::NumberCast => f.write_str("non-Double Transparency object"),
            Self::Binary(e) => e.fmt(f),
            Self::Arithmetic(e) => e.fmt(f),
            Self::Rounding(e) => e.fmt(f),
            Self::Channel(e) => e.fmt(f),
            Self::Notification(e) => e.fmt(f),
            Self::Image(e) => e.fmt(f),
            Self::Property(_) => f.write_str("Solid Color property metadata failure"),
        }
    }
}
impl<E: Error + 'static> Error for LiveSolidColorError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Binary(e) => Some(e),
            Self::Arithmetic(e) => Some(e),
            Self::Rounding(e) => Some(e),
            Self::Channel(e) => Some(e),
            Self::Notification(e) => Some(e),
            Self::Image(e) => Some(e),
            Self::Property(e) => Some(e),
            Self::Kind | Self::NullColor | Self::NumberCast => None,
        }
    }
}

// Both wrappers reuse stable core delegate identities. Diagnostic snapshots
// sponsor the owner outward; notifier storage holds only cores. As with the
// registry bridge, cross-owner forwarding-binding transfer is not supported.
macro_rules! delegates {
    ($handler:ident, $delegate:ident, $sender:ty, $construct:expr) => {
        pub struct $handler<E> {
            core: ChangeHandler<Envelope<E>, SolidColorCallbackError<E>>,
            sponsors: Vec<Arc<SponsorAnchor<E>>>,
        }
        impl<E> Clone for $handler<E> {
            fn clone(&self) -> Self {
                Self {
                    core: self.core.clone(),
                    sponsors: self.sponsors.clone(),
                }
            }
        }
        impl<E> fmt::Debug for $handler<E> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.core.fmt(f)
            }
        }
        impl<E: 'static> $handler<E> {
            pub fn new(
                callback: impl Fn(&$sender, Option<Arc<ContextChangeArgument<E>>>) -> Result<(), E>
                + Send
                + Sync
                + 'static,
            ) -> Self {
                Self::from_callback(move |sender, args| {
                    callback(sender, args).map_err(SolidColorCallbackError::Observer)
                })
            }
            pub(crate) fn from_callback(
                callback: impl Fn(
                    &$sender,
                    Option<Arc<ContextChangeArgument<E>>>,
                ) -> Result<(), SolidColorCallbackError<E>>
                + Send
                + Sync
                + 'static,
            ) -> Self {
                Self {
                    core: ChangeHandler::new(move |_, envelope: Option<Arc<Envelope<E>>>| {
                        let envelope = envelope.ok_or(SolidColorCallbackError::SenderReleased)?;
                        let owner = envelope
                            .owner
                            .upgrade()
                            .ok_or(SolidColorCallbackError::SenderReleased)?;
                        let anchor = envelope
                            .anchor
                            .upgrade()
                            .ok_or(SolidColorCallbackError::SenderReleased)?;
                        let _frame = anchor.node_frame(&owner);
                        let sender = ($construct)(owner, anchor, envelope.property)?;
                        callback(&sender, envelope.args.clone())
                    }),
                    sponsors: Vec::new(),
                }
            }
            pub fn same_binding(&self, other: &Self) -> bool {
                self.core.same_binding(&other.core)
            }
        }
        pub struct $delegate<E> {
            core: ChangeDelegate<Envelope<E>, SolidColorCallbackError<E>>,
            sponsors: Vec<Arc<SponsorAnchor<E>>>,
        }
        impl<E> Clone for $delegate<E> {
            fn clone(&self) -> Self {
                Self {
                    core: self.core.clone(),
                    sponsors: self.sponsors.clone(),
                }
            }
        }
        impl<E> Default for $delegate<E> {
            fn default() -> Self {
                Self {
                    core: ChangeDelegate::default(),
                    sponsors: Vec::new(),
                }
            }
        }
        impl<E> fmt::Debug for $delegate<E> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.core.fmt(f)
            }
        }
        impl<E> $delegate<E> {
            pub fn from_handlers(handlers: &[$handler<E>]) -> Self {
                let cores: Vec<_> = handlers.iter().map(|h| h.core.clone()).collect();
                let mut sponsors = Vec::new();
                for h in handlers {
                    for owner in &h.sponsors {
                        if !sponsors.iter().any(|held| Arc::ptr_eq(held, owner)) {
                            sponsors.push(Arc::clone(owner));
                        }
                    }
                }
                Self {
                    core: ChangeDelegate::from_handlers(&cores),
                    sponsors,
                }
            }
            pub fn is_present(&self) -> bool {
                self.core.is_present()
            }
            pub fn is_multicast(&self) -> bool {
                self.core.is_multicast()
            }
            pub fn same_invocation_array(&self, other: &Self) -> bool {
                self.core.same_invocation_array(&other.core)
            }
            pub fn handlers(&self) -> Vec<$handler<E>> {
                self.core
                    .handlers()
                    .iter()
                    .map(|core| $handler {
                        core: core.clone(),
                        sponsors: self.sponsors.clone(),
                    })
                    .collect()
            }
        }
    };
}
delegates!(
    SolidColorNodeHandler,
    SolidColorNodeDelegate,
    SharedSolidColorNode<E>,
    |owner, anchor, _: Option<SolidColorProperty>| Ok::<_, SolidColorCallbackError<E>>(
        SharedSolidColorNode(owner, anchor)
    )
);
delegates!(
    SolidColorPropertyHandler,
    SolidColorPropertyDelegate,
    SharedSolidColorProperty<E>,
    |owner, anchor, kind: Option<SolidColorProperty>| kind
        .map(|kind| SharedSolidColorProperty {
            owner,
            kind,
            anchor
        })
        .ok_or(SolidColorCallbackError::SenderReleased)
);

fn index(kind: SolidColorProperty) -> usize {
    match kind {
        SolidColorProperty::Color => 0,
        SolidColorProperty::Transparency => 1,
    }
}

impl<E: 'static> SharedSolidColorNode<E> {
    /// Actual fresh kx(gz.a/gz.b) context. No standalone property adoption,
    /// copy-constructor or arbitrary raw backing initialization is implied.
    pub fn new_unattached(mode: SolidColorCloneMode) -> Result<Self, LiveSolidColorError<E>> {
        Self::build_unattached(mode, None)
    }
    // kx(kx) / os(os,zl) copies stored values, clones retained animation and
    // adjustment roots, shares Limits, resets AllowChanges and creates events.
    pub(crate) fn clone_unadjusted_for_graph(
        &self,
    ) -> Result<Option<Self>, LiveSolidColorError<E>> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if state.color_apv.is_some() {
            return Ok(None);
        }
        // os(os,zl) copies RGB through a(rr,false) into a fresh black property
        // (os.cs:283–286; kx.cs:20–28). Equal zero components suppress that
        // assignment, retaining the constructor's positive zeros.
        if state.color == [0.0; 3] {
            state.color = [0.0; 3];
        }
        for ix in 0..2 {
            // os's copy constructor clones bh before its retained akf child.
            state.animations[ix] = state.animations[ix]
                .as_ref()
                .map(GradientAnimation::source_clone)
                .transpose()
                .map_err(|e| LiveSolidColorError::Property(e.into()))?;
            state.wire[ix] = state.wire[ix]
                .source_clone()
                .map_err(LiveSolidColorError::Property)?;
        }
        state.allow = [true; 2];
        Self::build_unattached(self.0.mode, Some((state, self.0.limits.clone()))).map(Some)
    }
    fn build_unattached(
        mode: SolidColorCloneMode,
        copied: Option<(State<E>, [SharedLimits; 2])>,
    ) -> Result<Self, LiveSolidColorError<E>> {
        let color_limits = SharedLimits::from_definition(LimitsDefinition::default())
            .map_err(LiveSolidColorError::Arithmetic)?;
        let increment =
            DecimalBits::new([1, 0, 0, 2 << 16]).map_err(LiveSolidColorError::Binary)?;
        let use_range = RangeOptionsRecord {
            flags: [false, true, true],
            ..RangeOptionsRecord::default()
        };
        let transparency_limits = SharedLimits::from_definition(LimitsDefinition {
            step: LimitsStep::Derived(increment),
            use_range: Some(SharedRangeOptionsRecord::new(use_range)),
            ..LimitsDefinition::default()
        })
        .map_err(LiveSolidColorError::Arithmetic)?;
        let (initial, limits) = copied.unwrap_or_else(|| {
            (
                State {
                    color: [0.0; 3],
                    transparency: 0.0,
                    wire: std::array::from_fn(|_| LegacyWireMetadata::new(true, true)),
                    animations: [None, None],
                    color_apv: None,
                    previous: 0.0,
                    normalized: 0.0,
                    modifier: 0.0,
                    allow: [true; 2],
                },
                [color_limits, transparency_limits],
            )
        });
        let anchor = SponsorAnchor::unpublished();
        let inner = Arc::new_cyclic(|owner: &Weak<Inner<E>>| {
            let empty = |property| {
                Arc::new(Envelope {
                    owner: owner.clone(),
                    property,
                    args: Some(ContextChangeArgument::<E>::shared_empty()),
                    anchor: Arc::downgrade(&anchor),
                })
            };
            Inner {
                route: CoreRoute::new(CoreKind::Node),
                mode,
                state: Mutex::new(initial.clone()),
                limits: limits.clone(),
                node_changed: ChangeNotifier::new(empty(None)),
                property_changed: [
                    ChangeNotifier::new(empty(Some(SolidColorProperty::Color))),
                    ChangeNotifier::new(empty(Some(SolidColorProperty::Transparency))),
                ],
                forwarder: ChangeHandler::new(|_, envelope: Option<Arc<Envelope<E>>>| {
                    let envelope = envelope.ok_or(SolidColorCallbackError::SenderReleased)?;
                    let owner = envelope
                        .owner
                        .upgrade()
                        .ok_or(SolidColorCallbackError::SenderReleased)?;
                    // zl.k7 -> sh: controls-only and speed bypass ordinary node
                    // dispatch. Mode b also has no ordinary node propagation.
                    let bypass = matches!(envelope.args.as_deref(), Some(ContextChangeArgument::Property(args)) if args.update_controls_only() || args.speed_rendering());
                    if bypass || owner.mode != SolidColorCloneMode::Original {
                        return Ok(());
                    }
                    let forwarded = Arc::new(Envelope {
                        owner: Arc::downgrade(&owner),
                        property: None,
                        args: envelope.args.clone(),
                        anchor: envelope.anchor.clone(),
                    });
                    owner
                        .node_changed
                        .emit(Some(forwarded))
                        .map_err(|e| SolidColorCallbackError::Node(Box::new(e)))
                }),
            }
        });
        // zl.g subscribes k7 during construction, before external observers.
        let forwarding = ChangeDelegate::from_handlers(std::slice::from_ref(&inner.forwarder));
        for notifier in &inner.property_changed {
            notifier.add(&forwarding);
        }
        anchor.publish(Member::Node(Arc::clone(&inner)));
        Ok(Self(inner, anchor))
    }
    /// Raw render owner, including actual rr.APV input identities. No public
    /// setters or binding effects run, and the new region owns every remapped ID.
    pub(crate) fn capture_raw(
        &self,
        inputs: &mut crate::color_region::ColorInputCapture<E>,
        session: &mut RenderCaptureSession<E>,
    ) -> Result<Self, LiveSolidColorError<E>> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let input = state
            .color_apv
            .map(|id| inputs.copy(&self.1.input_handle(id)));
        state.color_apv = input.as_ref().map(|v| v.core.route.id);
        for ix in 0..2 {
            state.wire[ix] = state.wire[ix]
                .capture_with(|a| session.copy_adjustment(a))
                .map_err(LiveSolidColorError::Property)?;
            state.animations[ix] = state.animations[ix]
                .as_ref()
                .map(|a| session.copy_animation(a))
                .transpose()
                .map_err(|e| LiveSolidColorError::Property(e.into()))?;
        }
        let copied = self.0.limits.each_ref().map(|l| session.copy_limits(l));
        let copy = Self::build_unattached(self.0.mode, Some((state, copied)))?;
        if let Some(input) = input {
            merge(&copy.1, &input.anchor);
        }
        Ok(copy)
    }

    /// zl.s8 creates kx and applies os.CloneFrom to its constructor properties.
    pub(crate) fn copy_for_final_render(
        &self,
        inputs: &mut crate::color_region::ColorInputCapture<E>,
    ) -> Result<Self, LiveSolidColorError<E>> {
        let source = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let copy = Self::new_unattached(SolidColorCloneMode::Clone)?;
        let mut color = crate::ContextColorValue::unlinked(source.color);
        if let Some(id) = source.color_apv {
            let input = inputs.copy(&self.1.input_handle(id));
            color = color.with_input_reference(&input);
        }
        copy.color_property().set_color_value(&color)?;
        copy.transparency_property()
            .set_transparency(source.transparency)?;
        let mut state = copy.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.normalized = source.normalized;
        state.modifier = source.modifier;
        for ix in 0..2 {
            state.wire[ix] = source.wire[ix]
                .source_clone()
                .map_err(LiveSolidColorError::Property)?;
            state.animations[ix] = source.animations[ix]
                .as_ref()
                .map(GradientAnimation::source_clone)
                .transpose()
                .map_err(|error| LiveSolidColorError::Property(error.into()))?;
        }
        drop(state);
        Ok(copy)
    }
    pub(crate) fn raw_color_input(&self) -> Option<crate::SharedColorInput<E>> {
        let id = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .color_apv;
        id.map(|id| self.1.input_handle(id))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn color_property(&self) -> SharedSolidColorProperty<E> {
        self.property(SolidColorProperty::Color)
    }
    pub fn transparency_property(&self) -> SharedSolidColorProperty<E> {
        self.property(SolidColorProperty::Transparency)
    }
    fn property(&self, kind: SolidColorProperty) -> SharedSolidColorProperty<E> {
        SharedSolidColorProperty {
            owner: Arc::clone(&self.0),
            kind,
            anchor: Arc::clone(&self.1),
        }
    }
    pub fn snapshot(&self) -> SolidColorNodeSnapshot {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        SolidColorNodeSnapshot {
            mode: self.0.mode,
            color: state.color,
            transparency: state.transparency,
        }
    }
    pub fn add(&self, delegate: &SolidColorNodeDelegate<E>) {
        self.0.node_changed.add(&delegate.core);
    }
    pub fn remove(
        &self,
        delegate: &SolidColorNodeDelegate<E>,
    ) -> Result<(), LiveSolidColorError<E>> {
        self.0
            .node_changed
            .remove(&delegate.core)
            .map_err(|e| LiveSolidColorError::Notification(ChangeDispatchError::Framework(e)))
    }
    /// Retained diagnostic snapshot; framework binding transfer across owners
    /// is outside the characterized ownership scope.
    pub fn changed_delegate(&self) -> SolidColorNodeDelegate<E> {
        SolidColorNodeDelegate {
            core: self.0.node_changed.snapshot().delegate,
            sponsors: vec![Arc::clone(&self.1)],
        }
    }
    pub fn set_channel(&self, channel: i32, value: f32) -> Result<(), LiveSolidColorError<E>> {
        match channel {
            0..=2 => self
                .color_property()
                .edit_color_component(channel, f64::from(value), false)
                .map(|_| ()),
            3 => self
                .transparency_property()
                .set_transparency(f64::from(value)),
            _ => Err(LiveSolidColorError::Channel(SolidColorChannelError {
                channel,
            })),
        }
    }
    pub fn constant_channel(&self, channel: i32) -> Result<f32, LiveSolidColorError<E>> {
        // kx.a6 reads raw bl()/bh(), independently of evaluated rendering.
        let state = self.snapshot();
        match channel {
            0..=2 => Ok(state.color[channel as usize] as f32),
            3 => Ok(state.transparency as f32),
            4.. => Ok(0.0),
            _ => Err(LiveSolidColorError::Channel(SolidColorChannelError {
                channel,
            })),
        }
    }
    pub fn is_channel_constant(&self, channel: i32) -> bool {
        match channel {
            3 => self
                .transparency_property()
                .is_guaranteed_constant()
                .unwrap_or(false),
            4.. => true,
            _ => self
                .color_property()
                .is_guaranteed_constant()
                .unwrap_or(false),
        }
    }
    fn sampled_values(
        &self,
        time: f64,
        mut binding: impl FnMut(
            SolidColorProperty,
        )
            -> Result<Option<crate::MappedVariableValue>, PointwiseError<E>>,
    ) -> Result<([f64; 3], f64), PointwiseError<E>> {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let color = match &state.animations[0] {
            Some(animation) if animation.is_active()? => animation.sample_color(time)?,
            _ => binding(SolidColorProperty::Color)?.map_or(state.color, |value| value.color()),
        };
        let transparency = match &state.animations[1] {
            Some(animation) if animation.is_active()? => {
                let value = f64::from(animation.sample_axis(0, time)?);
                if self.0.limits[1].snapshot().decimal_places == 0 {
                    value.floor()
                } else {
                    value
                }
            }
            _ => binding(SolidColorProperty::Transparency)?
                .map_or(state.transparency, |value| value.number()),
        };
        Ok((color, transparency))
    }

    pub fn render(
        &self,
        width: i32,
        height: i32,
        time: f64,
    ) -> Result<ImageMap, LiveSolidColorError<E>> {
        self.render_with_bindings(width, height, time, |_| Ok(None))
    }

    /// Sample an independent copy for a standalone still preview.
    pub fn capture_preview_at(
        &self,
        time: f64,
    ) -> Result<SolidColorNodeSnapshot, LiveSolidColorError<E>> {
        self.capture_still_at(time, false)
    }

    /// A timed final still uses fresh constructor properties and copied curves.
    pub fn capture_final_at(
        &self,
        time: f64,
    ) -> Result<SolidColorNodeSnapshot, LiveSolidColorError<E>> {
        self.capture_still_at(time, true)
    }

    fn capture_still_at(
        &self,
        time: f64,
        final_render: bool,
    ) -> Result<SolidColorNodeSnapshot, LiveSolidColorError<E>> {
        if !self
            .color_property()
            .is_animated()
            .map_err(LiveSolidColorError::Property)?
            && !self
                .transparency_property()
                .is_animated()
                .map_err(LiveSolidColorError::Property)?
        {
            return Ok(self.snapshot());
        }
        let mut inputs = crate::color_region::ColorInputCapture::default();
        let copy = if final_render {
            self.copy_for_final_render(&mut inputs)?
        } else {
            self.capture_raw(&mut inputs, &mut RenderCaptureSession::new())?
        };
        let (color, transparency) = copy
            .sampled_values(time, |_| Ok(None))
            .map_err(LiveSolidColorError::Property)?;
        Ok(SolidColorNodeSnapshot {
            mode: copy.0.mode,
            color,
            transparency,
        })
    }

    pub(crate) fn render_with_bindings(
        &self,
        width: i32,
        height: i32,
        time: f64,
        binding: impl FnMut(
            SolidColorProperty,
        ) -> Result<Option<crate::MappedVariableValue>, PointwiseError<E>>,
    ) -> Result<ImageMap, LiveSolidColorError<E>> {
        let (color, transparency) = self
            .sampled_values(time, binding)
            .map_err(LiveSolidColorError::Property)?;
        crate::solid_color::render_stored_values(color, transparency, width, height)
            .map_err(LiveSolidColorError::Image)
    }
    /// Copy raw stored RGB/transparency into a fresh historical wire DTO.
    /// This value projection has fresh default metadata. Live node Save uses
    /// the actual retained properties and their animation/adjustment children.
    pub fn fixed_wire_projection(&self) -> SolidColorWireRecord {
        let state = self.snapshot();
        SolidColorWireRecord {
            color: state.color,
            transparency: state.transparency,
            ..SolidColorWireRecord::default()
        }
    }
}

impl<E: 'static> SharedSolidColorProperty<E> {
    pub const fn kind(&self) -> SolidColorProperty {
        self.kind
    }
    pub(crate) fn name(&self) -> &'static str {
        match self.kind {
            SolidColorProperty::Color => "Color",
            SolidColorProperty::Transparency => "Transparency",
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.kind == other.kind && Arc::ptr_eq(&self.owner, &other.owner)
    }
    pub fn parent(&self) -> SharedSolidColorNode<E> {
        SharedSolidColorNode(Arc::clone(&self.owner), Arc::clone(&self.anchor))
    }
    pub fn limits(&self) -> SharedLimits {
        self.owner.limits[index(self.kind)].clone()
    }
    pub fn snapshot(&self) -> SolidColorPropertySnapshot {
        let (
            color,
            current,
            previous,
            normalized,
            modifier,
            allow,
            animation_present,
            adjustment_present,
        ) = {
            let state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
            let (color, current, previous, normalized, modifier, metadata) = match self.kind {
                SolidColorProperty::Color => (state.color, 0.0, 0.0, 0.0, 0.0, &state.wire[0]),
                SolidColorProperty::Transparency => (
                    [0.0; 3],
                    state.transparency,
                    state.previous,
                    state.normalized,
                    state.modifier,
                    &state.wire[1],
                ),
            };
            (
                color,
                current,
                previous,
                normalized,
                modifier,
                state.allow[index(self.kind)],
                state.animations[index(self.kind)].is_some(),
                metadata.retained_adjustment().is_some(),
            )
        };
        let name = self.name().encode_utf16().collect();
        SolidColorPropertySnapshot {
            kind: self.kind,
            name,
            color,
            current,
            previous,
            normalized,
            modifier,
            allow,
            limits: self.limits(),
            animation_present,
            adjustment_present,
        }
    }
    /// Legacy AllowChanges reshapes each emitted argument; it never locks the
    /// notifier or creates pending notifications.
    pub fn set_allow_changes(&self, allow: bool) {
        self.owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allow[index(self.kind)] = allow;
    }
    pub fn add(&self, delegate: &SolidColorPropertyDelegate<E>) {
        self.owner.property_changed[index(self.kind)].add(&delegate.core);
    }
    pub fn remove(
        &self,
        delegate: &SolidColorPropertyDelegate<E>,
    ) -> Result<(), LiveSolidColorError<E>> {
        self.owner.property_changed[index(self.kind)]
            .remove(&delegate.core)
            .map_err(|e| LiveSolidColorError::Notification(ChangeDispatchError::Framework(e)))
    }
    /// Outward snapshots retain the node. Cross-owner framework forwarding
    /// transfer is not a supported adoption or ownership bridge.
    pub fn changed_delegate(&self) -> SolidColorPropertyDelegate<E> {
        SolidColorPropertyDelegate {
            core: self.owner.property_changed[index(self.kind)]
                .snapshot()
                .delegate,
            sponsors: vec![Arc::clone(&self.anchor)],
        }
    }
    pub fn forwarding_handler(&self) -> SolidColorPropertyHandler<E> {
        SolidColorPropertyHandler {
            core: self.owner.forwarder.clone(),
            sponsors: vec![Arc::clone(&self.anchor)],
        }
    }
    /// Copies the stored, unadjusted RGB/reference. Positive references retain
    /// their actual input cores; no evaluated/animated color is implied.
    pub fn color_value(&self) -> Result<crate::ContextColorValue<E>, LiveSolidColorError<E>> {
        if self.kind != SolidColorProperty::Color {
            return Err(LiveSolidColorError::Kind);
        }
        let stored = {
            let state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
            StoredColorValue {
                components: state.color,
                apv: state.color_apv,
            }
        };
        Ok(crate::ContextColorValue::retained(stored, &self.anchor))
    }
    pub fn set_color_value(
        &self,
        value: &crate::ContextColorValue<E>,
    ) -> Result<(), LiveSolidColorError<E>> {
        self.set_color_value_temporary(value, false)
    }
    /// Ordinary source rr setter: equality ignores the input reference. A
    /// successful unequal assignment commits before synchronous callbacks;
    /// observer failure leaves that commit. An unattached property has no
    /// document undo target.
    pub fn set_color_value_temporary(
        &self,
        value: &crate::ContextColorValue<E>,
        temporary: bool,
    ) -> Result<(), LiveSolidColorError<E>> {
        self.edit_color_value(value, temporary, false).map(|_| ())
    }
    pub(crate) fn edit_color_component(
        &self,
        channel: i32,
        value: f64,
        has_history_target: bool,
    ) -> Result<Option<Arc<ContextChangeArgument<E>>>, LiveSolidColorError<E>> {
        let color = self.color_value()?;
        let mut components = color.components();
        let component =
            components
                .get_mut(channel as usize)
                .ok_or(LiveSolidColorError::Channel(SolidColorChannelError {
                    channel,
                }))?;
        *component = value;
        self.edit_color_value(
            &color.with_components(components),
            false,
            has_history_target,
        )
    }
    pub(crate) fn edit_color_value(
        &self,
        value: &crate::ContextColorValue<E>,
        temporary: bool,
        has_history_target: bool,
    ) -> Result<Option<Arc<ContextChangeArgument<E>>>, LiveSolidColorError<E>> {
        if self.kind != SolidColorProperty::Color {
            return Err(LiveSolidColorError::Kind);
        }
        let stored = value.stored();
        if let Some(sponsor) = value.sponsor() {
            if self
                .owner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .color
                == stored.components
            {
                return Ok(None);
            }
            merge(&self.anchor, sponsor);
        }
        let old = {
            let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.color == stored.components {
                return Ok(None);
            }
            let old = StoredColorValue {
                components: state.color,
                apv: state.color_apv,
            };
            state.color = stored.components;
            state.color_apv = stored.apv;
            old
        };
        self.emit_edit(has_history_target, |name| {
            let undo = crate::ContextColorUndoInfo::with_values(
                Some(name),
                6,
                crate::ContextColorValue::retained(old, &self.anchor),
                value.clone(),
                temporary,
            );
            ContextPropertyChangeArgs::with_color_undo(Some(Arc::new(undo)))
        })
    }
    /// Source reference-only stamp: no RGB change, equality filter or event.
    /// This is not PropertyControl binding/refresh or an attachment factory.
    pub fn stamp_color_input_reference(
        &self,
        input: Option<&crate::SharedColorInput<E>>,
    ) -> Result<(), LiveSolidColorError<E>> {
        if self.kind != SolidColorProperty::Color {
            return Err(LiveSolidColorError::Kind);
        }
        let apv = if let Some(input) = input {
            merge(&self.anchor, &input.anchor);
            Some(input.core.route.id)
        } else {
            None
        };
        self.owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .color_apv = apv;
        Ok(())
    }
    pub fn set_color(&self, color: [f64; 3]) -> Result<(), LiveSolidColorError<E>> {
        self.set_color_temporary(color, false)
    }
    /// Existing unlinked RGB adapter over the same live value and event pipeline.
    pub fn set_color_temporary(
        &self,
        color: [f64; 3],
        temporary: bool,
    ) -> Result<(), LiveSolidColorError<E>> {
        self.set_color_value_temporary(&crate::ContextColorValue::unlinked(color), temporary)
    }
    /// Captured null object route only; arbitrary boxed type acceptance is not
    /// part of this typed RGB boundary.
    pub fn set_color_object(&self, color: Option<[f64; 3]>) -> Result<(), LiveSolidColorError<E>> {
        self.set_color(color.ok_or(LiveSolidColorError::NullColor)?)
    }
    pub fn set_transparency(&self, value: f64) -> Result<(), LiveSolidColorError<E>> {
        self.edit_transparency(value, false).map(|_| ())
    }
    pub fn set_transparency_temporary(
        &self,
        value: f64,
        _temporary: bool,
    ) -> Result<(), LiveSolidColorError<E>> {
        self.set_transparency(value)
    }
    pub fn set_transparency_speed(&self, value: f64) -> Result<(), LiveSolidColorError<E>> {
        self.set_number(value, true, false).map(|_| ())
    }
    pub fn set_transparency_object(
        &self,
        value: UndoObjectValue,
    ) -> Result<(), LiveSolidColorError<E>> {
        match value {
            UndoObjectValue::Double(value) => self.set_transparency(value),
            UndoObjectValue::Other => Err(LiveSolidColorError::NumberCast),
        }
    }
    pub(crate) fn edit_transparency(
        &self,
        value: f64,
        has_history_target: bool,
    ) -> Result<Option<Arc<ContextChangeArgument<E>>>, LiveSolidColorError<E>> {
        self.set_number(value, false, has_history_target)
    }
    fn set_number(
        &self,
        value: f64,
        speed: bool,
        has_history_target: bool,
    ) -> Result<Option<Arc<ContextChangeArgument<E>>>, LiveSolidColorError<E>> {
        if self.kind != SolidColorProperty::Transparency {
            return Err(LiveSolidColorError::Kind);
        }
        let (value, negative_bounds) = self.normalize_transparency(value)?;
        let old = {
            let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
            let current = state.transparency;
            if current == value && (speed || state.previous == current) {
                return Ok(None);
            }
            let old = if state.previous != current {
                state.previous
            } else {
                current
            };
            state.transparency = value;
            if !speed {
                state.previous = value;
            }
            state.normalized = normalized_value(value, state.modifier, negative_bounds);
            old
        };
        if speed {
            let args = if self.allows_changes() {
                ContextPropertyChangeArgs::with_speed(true)
            } else {
                ContextPropertyChangeArgs::default()
            };
            self.emit_args(Arc::new(ContextChangeArgument::Property(args)))?;
            return Ok(None);
        }
        self.emit_edit(has_history_target, |name| {
            let undo = crate::DoubleUndoInfo::new(Some(name), 0, old, value);
            ContextPropertyChangeArgs::with_undo(Some(Arc::new(undo)))
        })
    }
    fn normalize_transparency(&self, value: f64) -> Result<(f64, bool), LiveSolidColorError<E>> {
        let limits = self.limits().snapshot();
        let mut value = round_decimal_places(value, limits.decimal_places)
            .map_err(LiveSolidColorError::Rounding)?;
        let minimum = limits.minimum.to_f64();
        let maximum = limits.maximum.to_f64();
        if value < minimum {
            value = minimum;
        }
        if value > maximum {
            value = maximum;
        }
        Ok((value, minimum < 0.0))
    }

    pub(crate) fn prepare_adjusted_transparency(
        &self,
        value: f64,
    ) -> Result<f64, LiveSolidColorError<E>> {
        self.normalize_transparency(value).map(|(value, _)| value)
    }

    pub(crate) fn write_adjusted_transparency(&self, current: f64, previous: f64) {
        let negative_bounds = self.limits().snapshot().minimum.to_f64() < 0.0;
        let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.transparency = current;
        state.previous = previous;
        state.normalized = normalized_value(current, state.modifier, negative_bounds);
    }

    pub(crate) fn notify_adjusted_transparency(&self) -> Result<(), LiveSolidColorError<E>> {
        self.emit_edit(false, |_| unreachable!("catalog edits own their undo"))
            .map(|_| ())
    }

    pub(crate) fn replace_adjusted_color(
        &self,
        expected: &crate::ContextColorValue<E>,
        value: &crate::ContextColorValue<E>,
    ) -> bool {
        debug_assert_eq!(self.kind, SolidColorProperty::Color);
        let expected = expected.stored();
        let value = value.stored();
        let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.color.map(f64::to_bits) != expected.components.map(f64::to_bits)
            || state.color_apv != expected.apv
        {
            return false;
        }
        state.color = value.components;
        state.color_apv = value.apv;
        true
    }

    pub(crate) fn notify_adjusted_color(&self) -> Result<(), LiveSolidColorError<E>> {
        self.emit_edit(false, |_| unreachable!("catalog edits own their undo"))
            .map(|_| ())
    }
    /// os binding refresh writes the stored value without producing an undo record.
    pub(crate) fn refresh_bound_value(
        &self,
        value: &crate::MappedVariableValue,
        force: bool,
        notify: bool,
    ) -> Result<(), LiveSolidColorError<E>> {
        match self.kind {
            SolidColorProperty::Color => {
                let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.color == value.color() && !force {
                    return Ok(());
                }
                state.color = value.color();
                state.color_apv = None;
            }
            SolidColorProperty::Transparency => {
                let (value, negative_bounds) = self.normalize_transparency(value.number())?;
                let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.transparency == value && !force {
                    return Ok(());
                }
                state.transparency = value;
                state.previous = value;
                state.normalized = normalized_value(value, state.modifier, negative_bounds);
            }
        }
        if notify {
            let args = if self.allows_changes() {
                ContextChangeArgument::shared_empty()
            } else {
                Arc::new(ContextChangeArgument::Property(
                    ContextPropertyChangeArgs::default(),
                ))
            };
            self.emit_args(args)?;
        }
        Ok(())
    }
    fn allows_changes(&self) -> bool {
        self.owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allow[index(self.kind)]
    }
    pub(crate) fn emit_edit(
        &self,
        has_history_target: bool,
        create_undo: impl FnOnce(Vec<u16>) -> ContextPropertyChangeArgs<E>,
    ) -> Result<Option<Arc<ContextChangeArgument<E>>>, LiveSolidColorError<E>> {
        if self.owner.mode != SolidColorCloneMode::Original {
            return Ok(None);
        }
        let eligible = has_history_target
            && !self
                .wire_metadata()
                .is_adjusted()
                .map_err(LiveSolidColorError::Property)?;
        let allow = self.allows_changes();
        let property = if !allow {
            ContextPropertyChangeArgs::default()
        } else if eligible {
            create_undo(self.name().encode_utf16().collect())
        } else {
            ContextPropertyChangeArgs::with_undo(None)
        };
        let args = Arc::new(ContextChangeArgument::Property(property));
        self.emit_args(args.clone())?;
        Ok(allow.then_some(args))
    }
    fn emit_args(&self, args: Arc<ContextChangeArgument<E>>) -> Result<(), LiveSolidColorError<E>> {
        let frame = self.clone();
        let _lease: DispatchLease<E> = frame.anchor.node_frame(&frame.owner);
        let envelope = Arc::new(Envelope {
            owner: Arc::downgrade(&frame.owner),
            property: Some(frame.kind),
            args: Some(args),
            anchor: Arc::downgrade(&frame.anchor),
        });
        frame.owner.property_changed[index(frame.kind)]
            .emit(Some(envelope))
            .map_err(LiveSolidColorError::Notification)
    }
}

// os.a: literal floating operation order, including the negative branch and
// remainder NaN behavior. Modifier is the actual fresh field, not a substitute
// normalized current value; its mutation/evaluation routes are not exposed.
fn normalized_value(value: f64, modifier: f64, negative_bounds: bool) -> f64 {
    use crate::source_float::ordered_add;
    if value >= 0.0 && modifier >= 0.0 {
        ordered_add(value, modifier) % 1_000_000.0
    } else {
        let a = if value >= 0.0 {
            value
        } else {
            -ordered_add(value, 1.0)
        };
        let b = if modifier >= 0.0 {
            modifier
        } else {
            -ordered_add(modifier, 1.0)
        };
        let remainder = ordered_add(a, b) % 1_000_000.0;
        if negative_bounds {
            -ordered_add(remainder, 1.0)
        } else {
            remainder
        }
    }
}

#[cfg(test)]
mod graph_copy_laws {
    use super::{SharedSolidColorNode, SolidColorCloneMode};

    // Static source-body composition, not an additional original execution.
    #[test]
    fn fresh_rgb_copy_suppresses_only_the_all_zero_assignment() {
        for color in [
            [-0.0, 0.0, -0.0],
            [-0.0, 0.0, 1.0],
            [f64::from_bits(0x7ff8_0000_0000_0042), -0.0, 0.0],
        ] {
            let source =
                SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
            source.color_property().set_color([1.0, 2.0, 3.0]).unwrap();
            source.color_property().set_color(color).unwrap();
            let copy = source.clone_unadjusted_for_graph().unwrap().unwrap();
            let expected = if color == [0.0; 3] { [0.0; 3] } else { color };
            assert_eq!(
                copy.snapshot().color.map(f64::to_bits),
                expected.map(f64::to_bits)
            );
            assert_eq!(
                source.snapshot().color.map(f64::to_bits),
                color.map(f64::to_bits)
            );
            assert!(!copy.same_reference(&source));
            assert!(
                copy.color_property()
                    .limits()
                    .same_reference(&source.color_property().limits())
            );
        }
    }
}

#[cfg(test)]
mod normalization_tests {
    use super::normalized_value;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::{fs, path::PathBuf};

    #[test]
    fn original_legacy_normalization_preserves_literal_negation_and_remainder() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../port/evidence/legacy-normalization-original.jsonl");
        let bytes = fs::read(path).expect("original helper corpus");
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            "ed3f8eac134ac302aefcaa7881e0a0c6de611967c2887b72a523ea6c813fdb78"
        );
        let raw = |value: &Value| {
            u64::from_str_radix(value.as_str().expect("raw bits"), 16).expect("u64")
        };
        let mut count = 0;
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let row: Value = serde_json::from_slice(line).expect("JSON");
            if row["kind"] != "case" {
                continue;
            }
            let a = raw(&row["value"]);
            let b = raw(&row["modifier"]);
            let value = f64::from_bits(a);
            let modifier = f64::from_bits(b);
            let result = normalized_value(
                value,
                modifier,
                row["negative_bounds"].as_bool().expect("context"),
            );
            assert_eq!(
                result.to_bits(),
                raw(&row["result"]),
                "source helper case {count}"
            );
            assert_eq!(value.to_bits(), a);
            assert_eq!(modifier.to_bits(), b);
            count += 1;
        }
        assert_eq!(count, 8192);
    }
}

#[cfg(test)]
mod retained_color_lifetime_tests {
    use super::*;
    use crate::{ColorInput, ColorValue};
    #[test]
    fn equal_value_does_not_merge_foreign_input_and_positive_getter_reclaims_real_peers() {
        let node = SharedSolidColorNode::<std::convert::Infallible>::new_unattached(
            SolidColorCloneMode::Original,
        )
        .unwrap();
        let property = node.color_property();
        let a = ColorInput::new_uncreated();
        let b = ColorInput::new_uncreated();
        let wa = Arc::downgrade(&a.core);
        let wb = Arc::downgrade(&b.core);
        let wn = Arc::downgrade(&node.0);
        property.stamp_color_input_reference(Some(&a)).unwrap();
        let equal = ColorValue::unlinked([0.0; 3]).with_input_reference(&b);
        property.set_color_value(&equal).unwrap();
        drop((equal, b));
        assert!(wb.upgrade().is_none());
        let retained = property.color_value().unwrap();
        drop((node, property, a));
        assert!(wn.upgrade().is_some() && wa.upgrade().is_some());
        assert!(retained.input_reference().is_some());
        drop(retained);
        assert!(wn.upgrade().is_none() && wa.upgrade().is_none());
    }
}

impl SolidColorNodeSnapshot {
    /// Render the captured raw bits without invoking setters or source Clone.
    pub fn render(&self, width: i32, height: i32) -> Result<ImageMap, ImageError> {
        crate::solid_color::render_stored_values(self.color, self.transparency, width, height)
    }
}
