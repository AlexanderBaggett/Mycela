use crate::color_region::{SponsorAnchor, StoredColorValue, merge};
use crate::{ContextColorValue, DefaultContext};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    marker::PhantomData,
};

/// The observed PropertyUndoInfo<double> base data. Subclasses, general generic
/// undo values and string reference identity are separate capabilities.
#[derive(Debug)]
pub struct DoubleUndoInfo {
    name: Option<Vec<u16>>,
    old: f64,
    new: f64,
    change: i32,
    temporary: AtomicBool,
    lab: AtomicBool,
}

/// A retained mutable source Double[] value, distinct from the undo root.
#[derive(Clone, Debug)]
pub struct PairUndoArray(Arc<Mutex<[f64; 2]>>);
impl PairUndoArray {
    pub fn new(values: [f64; 2]) -> Self {
        Self(Arc::new(Mutex::new(values)))
    }
    pub fn values(&self) -> [f64; 2] {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn set_values(&self, values: [f64; 2]) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = values;
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
/// PropertyUndoInfo<Double[]> retains both actual arrays; equality is array
/// reference equality rather than elementwise numeric equality.
#[derive(Debug)]
pub struct PairUndoInfo {
    name: Option<Vec<u16>>,
    old: PairUndoArray,
    new: PairUndoArray,
    temporary: AtomicBool,
    lab: AtomicBool,
}
impl PairUndoInfo {
    pub fn new(name: Option<Vec<u16>>, old: PairUndoArray, new: PairUndoArray) -> Self {
        Self {
            name,
            old,
            new,
            temporary: AtomicBool::new(false),
            lab: AtomicBool::new(false),
        }
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn old_value(&self) -> PairUndoArray {
        self.old.clone()
    }
    pub fn new_value(&self) -> PairUndoArray {
        self.new.clone()
    }
    pub fn property_change(&self) -> i32 {
        3
    }
    pub fn temporary_change(&self) -> bool {
        self.temporary.load(Ordering::SeqCst)
    }
    pub fn set_temporary_change(&self, value: bool) {
        self.temporary.store(value, Ordering::SeqCst);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.lab.load(Ordering::SeqCst)
    }
    pub fn set_lab_undo(&self, value: bool) {
        self.lab.store(value, Ordering::SeqCst);
    }
}
/// Typed boundary for boxed Double versus null/any non-Double object.
#[derive(Clone, Copy, Debug)]
pub enum UndoObjectValue {
    Double(f64),
    Other,
}
impl DoubleUndoInfo {
    pub fn new(name: Option<Vec<u16>>, change: i32, old: f64, new: f64) -> Self {
        Self::with_temporary(name, change, old, new, false)
    }
    pub fn with_temporary(
        name: Option<Vec<u16>>,
        change: i32,
        old: f64,
        new: f64,
        temporary: bool,
    ) -> Self {
        Self {
            name,
            old,
            new,
            change,
            temporary: AtomicBool::new(temporary),
            lab: AtomicBool::new(false),
        }
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn old_value(&self) -> f64 {
        self.old
    }
    pub fn new_value(&self) -> f64 {
        self.new
    }
    pub fn property_change(&self) -> i32 {
        self.change
    }
    pub fn temporary_change(&self) -> bool {
        self.temporary.load(Ordering::SeqCst)
    }
    pub fn set_temporary_change(&self, value: bool) {
        self.temporary.store(value, Ordering::SeqCst);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.lab.load(Ordering::SeqCst)
    }
    pub fn set_lab_undo(&self, value: bool) {
        self.lab.store(value, Ordering::SeqCst);
    }
    pub fn equals_old_object(&self, value: UndoObjectValue) -> bool {
        match value {
            UndoObjectValue::Double(v) => self.old == v || self.old.is_nan() && v.is_nan(),
            UndoObjectValue::Other => false,
        }
    }
}

/// One original Color undo value/flag root. Stored APV IDs carry no owner backedge.
struct ColorUndoCore {
    name: Option<Vec<u16>>,
    old: StoredColorValue,
    new: StoredColorValue,
    change: i32,
    temporary: AtomicBool,
    lab: AtomicBool,
}
impl fmt::Debug for ColorUndoCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ColorUndoCore")
            .field("name", &self.name)
            .field("old", &self.old.components)
            .field("new", &self.new.components)
            .field("change", &self.change)
            .field("temporary", &self.temporary)
            .field("lab", &self.lab)
            .finish()
    }
}
/// A context-typed handle over one copied Color undo core. Positive binding
/// construction remains private until the genuine producer passes its target gate.
pub struct ContextColorUndoInfo<E = DefaultContext> {
    core: Arc<ColorUndoCore>,
    sponsor: Option<Arc<SponsorAnchor<E>>>,
    context: PhantomData<fn() -> E>,
}
pub type ColorUndoInfo = ContextColorUndoInfo<DefaultContext>;
/// Boxed APV-null RGB color versus null or any other source object type.
#[derive(Clone, Copy, Debug)]
pub enum ColorUndoObjectValue {
    Color([f64; 3]),
    Other,
}
impl<E> ContextColorUndoInfo<E> {
    pub fn new(name: Option<Vec<u16>>, change: i32, old: [f64; 3], new: [f64; 3]) -> Self {
        Self::with_temporary(name, change, old, new, false)
    }
    pub fn with_temporary(
        name: Option<Vec<u16>>,
        change: i32,
        old: [f64; 3],
        new: [f64; 3],
        temporary: bool,
    ) -> Self {
        Self {
            core: Arc::new(ColorUndoCore {
                name,
                old: StoredColorValue::unlinked(old),
                new: StoredColorValue::unlinked(new),
                change,
                temporary: AtomicBool::new(temporary),
                lab: AtomicBool::new(false),
            }),
            sponsor: None,
            context: PhantomData,
        }
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.core.name.as_deref()
    }
    pub fn old_value(&self) -> [f64; 3] {
        self.core.old.components
    }
    pub fn new_value(&self) -> [f64; 3] {
        self.core.new.components
    }
    pub fn old_color_value(&self) -> ContextColorValue<E> {
        match &self.sponsor {
            Some(sponsor) => ContextColorValue::retained(self.core.old, sponsor),
            None => ContextColorValue::unlinked(self.core.old.components),
        }
    }
    pub fn new_color_value(&self) -> ContextColorValue<E> {
        match &self.sponsor {
            Some(sponsor) => ContextColorValue::retained(self.core.new, sponsor),
            None => ContextColorValue::unlinked(self.core.new.components),
        }
    }
    pub fn with_values(
        name: Option<Vec<u16>>,
        change: i32,
        old: ContextColorValue<E>,
        new: ContextColorValue<E>,
        temporary: bool,
    ) -> Self {
        if let (Some(a), Some(b)) = (old.sponsor(), new.sponsor()) {
            merge(a, b);
        }
        let sponsor = old.sponsor().or_else(|| new.sponsor()).cloned();
        Self {
            core: Arc::new(ColorUndoCore {
                name,
                old: old.stored(),
                new: new.stored(),
                change,
                temporary: AtomicBool::new(temporary),
                lab: AtomicBool::new(false),
            }),
            sponsor,
            context: PhantomData,
        }
    }
    pub fn property_change(&self) -> i32 {
        self.core.change
    }
    pub fn temporary_change(&self) -> bool {
        self.core.temporary.load(Ordering::SeqCst)
    }
    pub fn set_temporary_change(&self, value: bool) {
        self.core.temporary.store(value, Ordering::SeqCst);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.core.lab.load(Ordering::SeqCst)
    }
    pub fn set_lab_undo(&self, value: bool) {
        self.core.lab.store(value, Ordering::SeqCst);
    }
    /// Exact component comparison from rr.Equals: signed zeros compare equal;
    /// any NaN component makes the comparison false, including same-bit NaNs.
    pub fn equals_old_object(&self, value: ColorUndoObjectValue) -> bool {
        match value {
            ColorUndoObjectValue::Color(v) => {
                self.core.old.components[0] == v[0]
                    && self.core.old.components[1] == v[1]
                    && self.core.old.components[2] == v[2]
            }
            ColorUndoObjectValue::Other => false,
        }
    }
}

/// The exact boxed os choice value: Int32 or one retained ae7 reference.
#[derive(Clone, Debug)]
pub enum ChoiceUndoValue {
    Index(i32),
    Remembered(Arc<ChoiceUndoAnchor>),
}
/// Retains the source ae7 index, name-preserving anchor and optional UTF16 name.
#[derive(Debug)]
pub struct ChoiceUndoAnchor {
    index: i32,
    anchor: i32,
    text: Option<Vec<u16>>,
}
impl ChoiceUndoAnchor {
    pub fn new(index: i32, anchor: i32, text: Option<Vec<u16>>) -> Self {
        Self {
            index,
            anchor,
            text,
        }
    }
    pub fn index(&self) -> i32 {
        self.index
    }
    pub fn anchor(&self) -> i32 {
        self.anchor
    }
    pub fn text(&self) -> Option<&[u16]> {
        self.text.as_deref()
    }
}
impl ChoiceUndoValue {
    pub(crate) fn source_equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Index(a), Self::Index(b)) => a == b,
            (Self::Remembered(a), Self::Remembered(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}
/// Actual PropertyUndoInfo<Int32>/PropertyUndoInfo<ae7> at os.e(int).
#[derive(Debug)]
pub struct ChoiceUndoInfo {
    name: Option<Vec<u16>>,
    old: ChoiceUndoValue,
    new: ChoiceUndoValue,
    temporary: AtomicBool,
    lab: AtomicBool,
}
impl ChoiceUndoInfo {
    pub fn new(name: Option<Vec<u16>>, old: ChoiceUndoValue, new: ChoiceUndoValue) -> Self {
        Self {
            name,
            old,
            new,
            temporary: AtomicBool::new(false),
            lab: AtomicBool::new(false),
        }
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn old_value(&self) -> &ChoiceUndoValue {
        &self.old
    }
    pub fn new_value(&self) -> &ChoiceUndoValue {
        &self.new
    }
    pub fn property_change(&self) -> i32 {
        4
    }
    pub fn temporary_change(&self) -> bool {
        self.temporary.load(Ordering::SeqCst)
    }
    pub fn set_temporary_change(&self, value: bool) {
        self.temporary.store(value, Ordering::SeqCst);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.lab.load(Ordering::SeqCst)
    }
    pub fn set_lab_undo(&self, value: bool) {
        self.lab.store(value, Ordering::SeqCst);
    }
}

/// A retained concrete undo root shared by property-change arguments. These
/// observed payload kinds do not represent all source generic subclasses.
pub enum ContextUndoInfo<E = DefaultContext> {
    Double(Arc<DoubleUndoInfo>),
    Modern(Arc<ModernPropertyUndoInfo>),
    Pair(Arc<PairUndoInfo>),
    Choice(Arc<ChoiceUndoInfo>),
    Color(Arc<ContextColorUndoInfo<E>>),
    Animation(Arc<AnimationUndoInfo<E>>),
    Environment(Arc<EnvironmentUndoInfo>),
}

impl<E> ContextUndoInfo<E> {
    pub(crate) fn mark_lab_undo(&self) {
        match self {
            Self::Double(undo) => undo.set_lab_undo(true),
            Self::Modern(undo) => undo.set_lab_undo(true),
            Self::Pair(undo) => undo.set_lab_undo(true),
            Self::Choice(undo) => undo.set_lab_undo(true),
            Self::Color(undo) => undo.set_lab_undo(true),
            Self::Animation(undo) => undo.set_lab_undo(true),
            Self::Environment(undo) => undo.lab.store(true, Ordering::SeqCst),
        }
    }
}

/// Immutable source PropertyChangedEventArgs metadata with one retained typed
/// undo root. New argument values are distinct; cloning Arc handles shares them.
pub struct ContextPropertyChangeArgs<E = DefaultContext> {
    root: Arc<SourceArgumentRoot>,
    speed: bool,
    controls: bool,
    undo: Option<ContextUndoInfo<E>>,
}
impl<E> Default for ContextPropertyChangeArgs<E> {
    fn default() -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: true,
            undo: None,
        }
    }
}
impl<E> ContextPropertyChangeArgs<E> {
    pub fn with_flags(speed: bool, controls: bool) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed,
            controls,
            undo: None,
        }
    }
    pub fn with_speed(speed: bool) -> Self {
        Self::with_flags(speed, false)
    }
    pub fn with_undo(undo: Option<Arc<DoubleUndoInfo>>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: false,
            undo: undo.map(ContextUndoInfo::Double),
        }
    }
    pub fn with_choice_undo(undo: Option<Arc<ChoiceUndoInfo>>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: false,
            undo: undo.map(ContextUndoInfo::Choice),
        }
    }
    pub fn with_pair_undo(undo: Option<Arc<PairUndoInfo>>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: false,
            undo: undo.map(ContextUndoInfo::Pair),
        }
    }
    pub fn with_color_undo(undo: Option<Arc<ContextColorUndoInfo<E>>>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: false,
            undo: undo.map(ContextUndoInfo::Color),
        }
    }
    pub fn with_animation_undo(undo: Arc<AnimationUndoInfo<E>>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: false,
            undo: Some(ContextUndoInfo::Animation(undo)),
        }
    }
    pub(crate) fn with_environment_undo(undo: Option<Arc<EnvironmentUndoInfo>>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed: false,
            controls: false,
            undo: undo.map(ContextUndoInfo::Environment),
        }
    }
    pub(crate) fn from_modern(args: &ModernPropertyChangeArgs) -> Self {
        Self {
            root: args.root.clone(),
            speed: args.speed,
            controls: args.controls,
            undo: args.undo.clone().map(ContextUndoInfo::Modern),
        }
    }
    pub(crate) fn from_animation(args: &AnimationPropertyChangeArgs<E>) -> Self {
        Self {
            root: args.root.clone(),
            speed: false,
            controls: false,
            undo: Some(ContextUndoInfo::Animation(args.undo.clone())),
        }
    }
    pub(crate) fn rebind_double_context<F>(&self) -> Option<ContextPropertyChangeArgs<F>> {
        let undo = match &self.undo {
            None => None,
            Some(ContextUndoInfo::Double(v)) => Some(ContextUndoInfo::Double(v.clone())),
            _ => return None,
        };
        Some(ContextPropertyChangeArgs {
            root: self.root.clone(),
            speed: self.speed,
            controls: self.controls,
            undo,
        })
    }
    pub fn source_root(&self) -> &Arc<SourceArgumentRoot> {
        &self.root
    }
    pub fn speed_rendering(&self) -> bool {
        self.speed
    }
    pub fn update_controls_only(&self) -> bool {
        self.controls
    }
    /// Compatibility projection exposing only Double undo. Other payloads
    /// also retain source UndoInfo; generic consumers
    /// must inspect typed_undo_info instead.
    pub fn undo_info(&self) -> Option<&Arc<DoubleUndoInfo>> {
        match &self.undo {
            Some(ContextUndoInfo::Double(undo)) => Some(undo),
            Some(ContextUndoInfo::Color(_))
            | Some(ContextUndoInfo::Modern(_))
            | Some(ContextUndoInfo::Choice(_))
            | Some(ContextUndoInfo::Pair(_))
            | Some(ContextUndoInfo::Animation(_))
            | Some(ContextUndoInfo::Environment(_))
            | None => None,
        }
    }
    pub fn typed_undo_info(&self) -> Option<&ContextUndoInfo<E>> {
        self.undo.as_ref()
    }
}
/// Source semantic argument identity. It retains no model, callback or owner.
#[derive(Debug)]
pub struct SourceArgumentRoot {
    _private: (),
}
fn empty_source_root() -> Arc<SourceArgumentRoot> {
    static ROOT: OnceLock<Arc<SourceArgumentRoot>> = OnceLock::new();
    Arc::clone(ROOT.get_or_init(|| Arc::new(SourceArgumentRoot { _private: () })))
}
/// Empty EventArgs and default PropertyChangedEventArgs remain distinct kinds.
pub enum ContextChangeArgument<E = DefaultContext> {
    Empty,
    Property(ContextPropertyChangeArgs<E>),
    Modern(ModernPropertyChangeArgs),
    Animation(AnimationPropertyChangeArgs<E>),
}
impl<E: 'static> ContextChangeArgument<E> {
    pub fn shared_empty() -> Arc<Self> {
        // Only immutable Empty wrappers enter this private registry. They retain
        // no payload, E value, callback or owner; Arc clones leave after the guard.
        type EmptyRegistry = HashMap<TypeId, Box<dyn Any + Send + Sync>>;
        static EMPTY: OnceLock<std::sync::Mutex<EmptyRegistry>> = OnceLock::new();
        let mut entries = EMPTY
            .get_or_init(std::sync::Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let stored = entries
            .entry(TypeId::of::<Self>())
            .or_insert_with(|| Box::new(Arc::new(Self::Empty)));
        let empty = stored
            .downcast_ref::<Arc<Self>>()
            .expect("private TypeId entry is its immutable typed Empty wrapper");
        assert!(matches!(empty.as_ref(), Self::Empty));
        Arc::clone(empty)
    }
    /// Cross-context Empty identity uses this nongeneric source root. Typed
    /// wrapper Arcs are globally identical within E, never claimed equal across E.
    pub fn source_root(&self) -> Arc<SourceArgumentRoot> {
        match self {
            Self::Empty => empty_source_root(),
            Self::Property(args) => Arc::clone(args.source_root()),
            Self::Modern(args) => args.root.clone(),
            Self::Animation(args) => args.root.clone(),
        }
    }
}
pub type UndoInfo = ContextUndoInfo<DefaultContext>;
pub type PropertyChangeArgs = ContextPropertyChangeArgs<DefaultContext>;
pub type ChangeArgument = ContextChangeArgument<DefaultContext>;
impl<E> fmt::Debug for ContextColorUndoInfo<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.core.fmt(f)
    }
}
impl<E> fmt::Debug for ContextUndoInfo<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Double(undo) => f.debug_tuple("Double").field(undo).finish(),
            Self::Modern(undo) => f.debug_tuple("Modern").field(undo).finish(),
            Self::Pair(undo) => f.debug_tuple("Pair").field(undo).finish(),
            Self::Choice(undo) => f.debug_tuple("Choice").field(undo).finish(),
            Self::Color(undo) => f.debug_tuple("Color").field(undo).finish(),
            Self::Animation(undo) => f.debug_tuple("Animation").field(undo).finish(),
            Self::Environment(undo) => f.debug_tuple("Environment").field(undo).finish(),
        }
    }
}
impl<E> fmt::Debug for ContextPropertyChangeArgs<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyChangeArgs")
            .field("speed", &self.speed)
            .field("controls", &self.controls)
            .field("undo", &self.undo)
            .finish()
    }
}
impl<E> fmt::Debug for ContextChangeArgument<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Property(args) => f.debug_tuple("Property").field(args).finish(),
            Self::Modern(args) => f.debug_tuple("ModernProperty").field(args).finish(),
            Self::Animation(args) => f.debug_tuple("AnimationProperty").field(args).finish(),
        }
    }
}

impl<E> Clone for ContextUndoInfo<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Double(v) => Self::Double(v.clone()),
            Self::Modern(v) => Self::Modern(v.clone()),
            Self::Pair(v) => Self::Pair(v.clone()),
            Self::Choice(v) => Self::Choice(v.clone()),
            Self::Color(v) => Self::Color(v.clone()),
            Self::Animation(v) => Self::Animation(v.clone()),
            Self::Environment(v) => Self::Environment(v.clone()),
        }
    }
}
impl<E> Clone for ContextPropertyChangeArgs<E> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            speed: self.speed,
            controls: self.controls,
            undo: self.undo.clone(),
        }
    }
}

/// Actual modern Int32/Boolean/Vector3D undo values; these are not os choices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModernUndoValue {
    Int32(i32),
    Boolean(bool),
    Vector3D([f64; 3]),
}
#[derive(Debug)]
pub struct ModernPropertyUndoInfo {
    name: Option<Vec<u16>>,
    old: ModernUndoValue,
    new: ModernUndoValue,
    temporary: AtomicBool,
    lab: AtomicBool,
}
impl ModernPropertyUndoInfo {
    pub(crate) fn new(
        name: Option<Vec<u16>>,
        old: ModernUndoValue,
        new: ModernUndoValue,
        temporary: bool,
    ) -> Self {
        Self {
            name,
            old,
            new,
            temporary: AtomicBool::new(temporary),
            lab: AtomicBool::new(false),
        }
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn old_value(&self) -> ModernUndoValue {
        self.old
    }
    pub fn new_value(&self) -> ModernUndoValue {
        self.new
    }
    pub fn property_change(&self) -> i32 {
        match self.new {
            ModernUndoValue::Int32(_) => 0,
            ModernUndoValue::Boolean(_) => 4,
            ModernUndoValue::Vector3D(_) => 20,
        }
    }
    pub fn temporary_change(&self) -> bool {
        self.temporary.load(Ordering::Acquire)
    }
    pub fn set_temporary_change(&self, v: bool) {
        self.temporary.store(v, Ordering::Release);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.lab.load(Ordering::Acquire)
    }
    pub fn set_lab_undo(&self, v: bool) {
        self.lab.store(v, Ordering::Release);
    }
}
#[derive(Clone, Debug)]
pub struct ModernPropertyChangeArgs {
    root: Arc<SourceArgumentRoot>,
    speed: bool,
    controls: bool,
    undo: Option<Arc<ModernPropertyUndoInfo>>,
}
impl ModernPropertyChangeArgs {
    pub(crate) fn new(
        speed: bool,
        controls: bool,
        undo: Option<Arc<ModernPropertyUndoInfo>>,
    ) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            speed,
            controls,
            undo,
        }
    }
    pub fn source_root(&self) -> &Arc<SourceArgumentRoot> {
        &self.root
    }
    pub fn speed_rendering(&self) -> bool {
        self.speed
    }
    pub fn update_controls_only(&self) -> bool {
        self.controls
    }
    pub fn undo_info(&self) -> Option<&Arc<ModernPropertyUndoInfo>> {
        self.undo.as_ref()
    }
}

/// The old and new Environment values for the ordinary property replacement.
pub struct EnvironmentUndoInfo {
    name: Vec<u16>,
    old: Arc<crate::EnvironmentRecord>,
    new: Arc<crate::EnvironmentRecord>,
    temporary: AtomicBool,
    lab: AtomicBool,
}

impl EnvironmentUndoInfo {
    pub(crate) fn new(
        name: Vec<u16>,
        old: Arc<crate::EnvironmentRecord>,
        new: Arc<crate::EnvironmentRecord>,
    ) -> Self {
        Self {
            name,
            old,
            new,
            temporary: AtomicBool::new(false),
            lab: AtomicBool::new(false),
        }
    }

    pub fn name(&self) -> Option<&[u16]> {
        Some(&self.name)
    }
    pub fn old_value(&self) -> Arc<crate::EnvironmentRecord> {
        self.old.clone()
    }
    pub fn new_value(&self) -> Arc<crate::EnvironmentRecord> {
        self.new.clone()
    }
    pub const fn property_change(&self) -> i32 {
        14
    }
    pub fn temporary_change(&self) -> bool {
        self.temporary.load(Ordering::SeqCst)
    }
    pub fn set_temporary_change(&self, value: bool) {
        self.temporary.store(value, Ordering::SeqCst);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.lab.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for EnvironmentUndoInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentUndoInfo")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Actual PropertyUndoInfo<bh>: child roots carry no property or graph backedge.
/// Outward readers retain those exact animation roots, never property wrappers.
pub struct AnimationUndoInfo<E = DefaultContext> {
    name: Option<Vec<u16>>,
    old: Option<crate::GradientAnimation<E>>,
    new: Option<crate::GradientAnimation<E>>,
    temporary: AtomicBool,
    lab: AtomicBool,
}
impl<E> AnimationUndoInfo<E> {
    pub(crate) fn replacement(
        name: Option<Vec<u16>>,
        old: Option<crate::GradientAnimation<E>>,
        new: Option<crate::GradientAnimation<E>>,
    ) -> Self {
        Self {
            name,
            old,
            new,
            temporary: AtomicBool::new(true),
            lab: AtomicBool::new(false),
        }
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn old_value(&self) -> Option<crate::GradientAnimation<E>> {
        self.old.clone()
    }
    pub fn new_value(&self) -> Option<crate::GradientAnimation<E>> {
        self.new.clone()
    }
    pub const fn property_change(&self) -> i32 {
        13
    }
    pub fn temporary_change(&self) -> bool {
        self.temporary.load(Ordering::SeqCst)
    }
    pub fn set_temporary_change(&self, value: bool) {
        self.temporary.store(value, Ordering::SeqCst);
    }
    pub fn is_lab_undo(&self) -> bool {
        self.lab.load(Ordering::SeqCst)
    }
    pub fn set_lab_undo(&self, value: bool) {
        self.lab.store(value, Ordering::SeqCst);
    }
}
impl<E> fmt::Debug for AnimationUndoInfo<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyUndoInfo<bh>")
            .field("name", &self.name)
            .field("old", &self.old)
            .field("new", &self.new)
            .field("temporary", &self.temporary)
            .field("lab", &self.lab)
            .finish()
    }
}
/// Concrete PropertyChangedEventArgs with the actual typed bh undo producer.
/// Its transport variant does not imply that modern history routing is bound.
pub struct AnimationPropertyChangeArgs<E = DefaultContext> {
    root: Arc<SourceArgumentRoot>,
    undo: Arc<AnimationUndoInfo<E>>,
}
impl<E> AnimationPropertyChangeArgs<E> {
    pub(crate) fn new(undo: AnimationUndoInfo<E>) -> Self {
        Self {
            root: Arc::new(SourceArgumentRoot { _private: () }),
            undo: Arc::new(undo),
        }
    }
    pub fn source_root(&self) -> &Arc<SourceArgumentRoot> {
        &self.root
    }
    pub const fn speed_rendering(&self) -> bool {
        false
    }
    pub const fn update_controls_only(&self) -> bool {
        false
    }
    pub fn undo_info(&self) -> &Arc<AnimationUndoInfo<E>> {
        &self.undo
    }
}
impl<E> fmt::Debug for AnimationPropertyChangeArgs<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PropertyChangedEventArgs<bh>")
            .field(&self.undo)
            .finish()
    }
}
