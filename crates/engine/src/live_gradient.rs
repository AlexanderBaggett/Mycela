//! Retained scalar Gradient capability. Handle Clone retains identity; source_clone
//! performs the original per-occurrence copy. DialogInput, GDI/RNG/hash methods,
//! arbitrary subclass/cyclic list entries and concurrent mutation remain separate.
use crate::{BinaryCursor, BinaryError, BinaryWriter, GradientRecord, RgbColor};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GradientError {
    NullReference,
    Index,
    ArgumentIndex,
    InvalidCast,
    Overflow,
    Allocation,
    Binary(BinaryError),
}
impl GradientError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::NullReference => "NullReferenceException",
            Self::Index => "IndexOutOfRangeException",
            Self::ArgumentIndex => "ArgumentOutOfRangeException",
            Self::InvalidCast => "InvalidCastException",
            Self::Overflow => "OverflowException",
            Self::Allocation => "OutOfMemoryException",
            Self::Binary(e) => e.source_class(),
        }
    }
}
impl From<BinaryError> for GradientError {
    fn from(e: BinaryError) -> Self {
        Self::Binary(e)
    }
}
impl fmt::Display for GradientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gradient {}", self.source_class())
    }
}
impl Error for GradientError {}
fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
macro_rules! identity {
    ($name:ident) => {
        impl $name {
            pub fn same_reference(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.0, &other.0)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    concat!(stringify!($name), "({:p})"),
                    Arc::as_ptr(&self.0)
                )
            }
        }
    };
}
#[derive(Clone)]
pub struct ColorMixingPoint(Arc<Mutex<(i32, RgbColor)>>);
identity!(ColorMixingPoint);
impl ColorMixingPoint {
    pub fn new(coordinate: i32, color: RgbColor) -> Self {
        Self(Arc::new(Mutex::new((coordinate, color))))
    }
    pub fn coordinate(&self) -> i32 {
        guard(&self.0).0
    }
    pub fn set_coordinate(&self, v: i32) {
        guard(&self.0).0 = v;
    }
    pub fn color(&self) -> RgbColor {
        guard(&self.0).1
    }
    pub fn set_color(&self, v: RgbColor) {
        guard(&self.0).1 = v;
    }
    pub fn source_clone(&self) -> Self {
        let (x, c) = *guard(&self.0);
        Self::new(x, c)
    }
    pub fn clipped_copy(&self) -> Self {
        let c = self.color().argb();
        fn clip(v: f64) -> f64 {
            if v > 1.0 {
                1.0
            } else if v > 0.0 {
                v
            } else {
                0.0
            }
        }
        Self::new(
            self.coordinate(),
            RgbColor::from_argb([c[0], clip(c[1]), clip(c[2]), clip(c[3])]),
        )
    }
    pub fn source_equals(&self, other: &Self) -> bool {
        self.same_reference(other)
            || (self.coordinate() == other.coordinate() && self.color().equals(other.color()))
    }
}
#[derive(Clone)]
pub struct AlphaMixingPoint(Arc<Mutex<(i32, f64)>>);
identity!(AlphaMixingPoint);
impl AlphaMixingPoint {
    pub fn new(coordinate: i32, value: f64) -> Self {
        Self(Arc::new(Mutex::new((coordinate, value))))
    }
    pub fn coordinate(&self) -> i32 {
        guard(&self.0).0
    }
    pub fn set_coordinate(&self, v: i32) {
        guard(&self.0).0 = v;
    }
    pub fn value(&self) -> f64 {
        guard(&self.0).1
    }
    pub fn set_value(&self, v: f64) {
        guard(&self.0).1 = v;
    }
    pub fn source_clone(&self) -> Self {
        let (x, v) = *guard(&self.0);
        Self::new(x, v)
    }
    pub fn source_equals(&self, other: &Self) -> bool {
        self.same_reference(other)
            || (self.coordinate() == other.coordinate()
                && (self.value() - other.value()).abs() < 1e-5)
    }
}
/// A genuine retained box for the observed foreign Int32 list entry kind.
#[derive(Clone)]
pub struct GradientInteger(Arc<i32>);
identity!(GradientInteger);
impl GradientInteger {
    pub fn new(v: i32) -> Self {
        Self(Arc::new(v))
    }
    pub fn value(&self) -> i32 {
        *self.0
    }
}
#[derive(Clone, Debug)]
pub enum GradientEntry {
    Null,
    Color(ColorMixingPoint),
    Alpha(AlphaMixingPoint),
    Integer(GradientInteger),
}
impl GradientEntry {
    pub fn same_reference(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Color(a), Self::Color(b)) => a.same_reference(b),
            (Self::Alpha(a), Self::Alpha(b)) => a.same_reference(b),
            (Self::Integer(a), Self::Integer(b)) => a.same_reference(b),
            _ => false,
        }
    }
    fn equals(&self, other: &Self) -> Result<bool, GradientError> {
        match (self, other) {
            (Self::Null, _) => Err(GradientError::NullReference),
            (Self::Color(a), Self::Color(b)) => Ok(a.source_equals(b)),
            (Self::Alpha(a), Self::Alpha(b)) => Ok(a.source_equals(b)),
            (Self::Integer(a), Self::Integer(b)) => Ok(a.value() == b.value()),
            _ => Ok(false),
        }
    }
    fn color(&self) -> Option<ColorMixingPoint> {
        if let Self::Color(p) = self {
            Some(p.clone())
        } else {
            None
        }
    }
    fn alpha(&self) -> Option<AlphaMixingPoint> {
        if let Self::Alpha(p) = self {
            Some(p.clone())
        } else {
            None
        }
    }
    fn cast_color(&self) -> Result<ColorMixingPoint, GradientError> {
        match self {
            Self::Null => Err(GradientError::NullReference),
            Self::Color(p) => Ok(p.clone()),
            _ => Err(GradientError::InvalidCast),
        }
    }
    fn cast_alpha(&self) -> Result<AlphaMixingPoint, GradientError> {
        match self {
            Self::Null => Err(GradientError::NullReference),
            Self::Alpha(p) => Ok(p.clone()),
            _ => Err(GradientError::InvalidCast),
        }
    }
}
#[derive(Clone)]
pub struct GradientPointList(Arc<Mutex<Vec<GradientEntry>>>);
identity!(GradientPointList);
impl Default for GradientPointList {
    fn default() -> Self {
        Self::new()
    }
}
impl GradientPointList {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    pub fn len(&self) -> usize {
        guard(&self.0).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn entry(&self, index: i32) -> Result<GradientEntry, GradientError> {
        guard(&self.0)
            .get(index as usize)
            .cloned()
            .ok_or(GradientError::ArgumentIndex)
    }
    pub fn push(&self, v: GradientEntry) -> i32 {
        let mut l = guard(&self.0);
        let i = l.len() as i32;
        l.push(v);
        i
    }
    pub fn clear(&self) {
        let old = std::mem::take(&mut *guard(&self.0));
        drop(old);
    }
    pub fn set(&self, index: i32, v: GradientEntry) -> Result<(), GradientError> {
        let old = {
            let mut l = guard(&self.0);
            std::mem::replace(
                l.get_mut(index as usize)
                    .ok_or(GradientError::ArgumentIndex)?,
                v,
            )
        };
        drop(old);
        Ok(())
    }
    pub fn reverse(&self) {
        guard(&self.0).reverse();
    }
    fn contains(&self, value: &GradientEntry) -> Result<bool, GradientError> {
        for i in 0..self.len() {
            let v = self.entry(i as i32)?;
            if matches!(v, GradientEntry::Null) {
                if matches!(value, GradientEntry::Null) {
                    return Ok(true);
                }
            } else if v.equals(value)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn remove(&self, value: &GradientEntry) -> Result<(), GradientError> {
        for i in 0..self.len() {
            let v = self.entry(i as i32)?;
            let same = if matches!(v, GradientEntry::Null) {
                matches!(value, GradientEntry::Null)
            } else {
                v.equals(value)?
            };
            if same {
                let old = guard(&self.0).remove(i);
                drop(old);
                break;
            }
        }
        Ok(())
    }
    fn sort(&self, alpha: bool) -> Result<(), GradientError> {
        let mut changed = true;
        while changed {
            changed = false;
            for i in 1..self.len() {
                let a = self.entry(i as i32 - 1)?;
                let b = self.entry(i as i32)?;
                let (x, y) = if alpha {
                    (
                        a.alpha().ok_or(GradientError::NullReference)?.coordinate(),
                        b.alpha().ok_or(GradientError::NullReference)?.coordinate(),
                    )
                } else {
                    (
                        a.color().ok_or(GradientError::NullReference)?.coordinate(),
                        b.color().ok_or(GradientError::NullReference)?.coordinate(),
                    )
                };
                if x > y {
                    guard(&self.0).swap(i - 1, i);
                    changed = true;
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct GradientCache(Arc<Mutex<Box<[RgbColor]>>>);
identity!(GradientCache);
impl GradientCache {
    pub fn new(length: i32) -> Result<Self, GradientError> {
        if length < 0 {
            return Err(GradientError::Overflow);
        }
        let mut v = Vec::new();
        v.try_reserve_exact(length as usize)
            .map_err(|_| GradientError::Allocation)?;
        v.resize(length as usize, RgbColor::default());
        Ok(Self::from_values(v))
    }
    pub fn from_values(v: Vec<RgbColor>) -> Self {
        Self(Arc::new(Mutex::new(v.into_boxed_slice())))
    }
    pub fn len(&self) -> usize {
        guard(&self.0).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn value(&self, index: i32) -> Result<RgbColor, GradientError> {
        guard(&self.0)
            .get(index as usize)
            .copied()
            .ok_or(GradientError::Index)
    }
    pub(crate) fn sample(&self, t: f64) -> Result<RgbColor, GradientError> {
        let colors = guard(&self.0);
        if t >= 1.0 {
            return colors.last().copied().ok_or(GradientError::Index);
        }
        if t <= 0.0 {
            return colors.first().copied().ok_or(GradientError::Index);
        }
        let pos = t * f64::from((colors.len() as i32).wrapping_sub(1));
        let i = crate::framework_cast::integer(pos);
        let part = pos - f64::from(i);
        let a = colors.get(i as usize).ok_or(GradientError::Index)?.argb();
        let b = colors
            .get(i.wrapping_add(1) as usize)
            .ok_or(GradientError::Index)?
            .argb();
        Ok(RgbColor::from_argb(std::array::from_fn(|n| {
            a[n] + part * (b[n] - a[n])
        })))
    }
    pub fn set(&self, index: i32, value: RgbColor) -> Result<(), GradientError> {
        *guard(&self.0)
            .get_mut(index as usize)
            .ok_or(GradientError::Index)? = value;
        Ok(())
    }
    fn channel(&self, index: i32, channel: usize, value: f64) -> Result<(), GradientError> {
        let old = self.value(index)?;
        let mut c = old.argb();
        c[channel] = value;
        self.set(index, RgbColor::from_argb(c).grayscale(old.is_grayscale()))
    }
}
struct State {
    width: i32,
    colors: Option<GradientPointList>,
    alpha: Option<GradientPointList>,
    cache: Option<GradientCache>,
}
#[derive(Clone)]
pub struct SharedGradient(Arc<Mutex<State>>);
identity!(SharedGradient);
impl Default for SharedGradient {
    fn default() -> Self {
        Self::new()
    }
}
impl SharedGradient {
    pub fn new() -> Self {
        Self::with_width(1000)
    }
    fn with_width(width: i32) -> Self {
        Self(Arc::new(Mutex::new(State {
            width,
            colors: Some(GradientPointList::new()),
            alpha: Some(GradientPointList::new()),
            cache: None,
        })))
    }
    /// Explicit detached wire import. This is not an original source constructor.
    pub fn from_wire_record(record: GradientRecord) -> Self {
        let g = Self::with_width(record.width);
        let colors = g.color_points().unwrap();
        for p in record.colors {
            colors.push(GradientEntry::Color(ColorMixingPoint::new(
                p.coordinate,
                RgbColor::from_argb(p.argb),
            )));
        }
        let alphas = g.alpha_points().unwrap();
        for p in record.transparency {
            alphas.push(GradientEntry::Alpha(AlphaMixingPoint::new(
                p.coordinate,
                p.value,
            )));
        }
        g
    }
    pub fn width(&self) -> i32 {
        guard(&self.0).width
    }
    pub fn set_width(&self, v: i32) {
        guard(&self.0).width = v;
    }
    pub fn color_points(&self) -> Option<GradientPointList> {
        guard(&self.0).colors.clone()
    }
    pub fn alpha_points(&self) -> Option<GradientPointList> {
        guard(&self.0).alpha.clone()
    }
    pub fn cache(&self) -> Option<GradientCache> {
        guard(&self.0).cache.clone()
    }
    pub fn set_color_points(&self, v: Option<GradientPointList>) {
        let old = std::mem::replace(&mut guard(&self.0).colors, v);
        drop(old);
    }
    pub fn set_alpha_points(&self, v: Option<GradientPointList>) {
        let old = std::mem::replace(&mut guard(&self.0).alpha, v);
        drop(old);
    }
    pub fn set_cache(&self, v: Option<GradientCache>) {
        let old = std::mem::replace(&mut guard(&self.0).cache, v);
        drop(old);
    }
    fn colors(&self) -> Result<GradientPointList, GradientError> {
        self.color_points().ok_or(GradientError::NullReference)
    }
    fn alphas(&self) -> Result<GradientPointList, GradientError> {
        self.alpha_points().ok_or(GradientError::NullReference)
    }
    fn buffer(&self) -> Result<GradientCache, GradientError> {
        self.cache().ok_or(GradientError::NullReference)
    }
    pub fn color_count(&self) -> Result<i32, GradientError> {
        Ok(self.colors()?.len() as i32)
    }
    pub fn alpha_count(&self) -> Result<i32, GradientError> {
        Ok(self.alphas()?.len() as i32)
    }
    pub fn color_point(&self, i: i32) -> Result<Option<ColorMixingPoint>, GradientError> {
        Ok(self.colors()?.entry(i)?.color())
    }
    pub fn alpha_point(&self, i: i32) -> Result<Option<AlphaMixingPoint>, GradientError> {
        Ok(self.alphas()?.entry(i)?.alpha())
    }
    pub fn resize_cache(&self, width: i32) -> Result<(), GradientError> {
        self.set_width(width);
        self.reset_cache()
    }
    /// Direct OnDeserialization body only, not framework deserialization machinery.
    pub fn reset_cache(&self) -> Result<(), GradientError> {
        let next = GradientCache::new(self.width())?;
        self.set_cache(Some(next));
        Ok(())
    }
    pub fn add_normalized_color(
        &self,
        t: f64,
        r: f64,
        g: f64,
        b: f64,
    ) -> Result<(), GradientError> {
        self.append_color(Some(ColorMixingPoint::new(
            crate::framework_cast::integer(t * f64::from(self.width())),
            RgbColor::new(r, g, b, 1.0),
        )))
    }
    pub fn add_normalized_alpha(&self, t: f64, v: f64) -> Result<(), GradientError> {
        self.append_alpha(Some(AlphaMixingPoint::new(
            crate::framework_cast::integer(t * f64::from(self.width())),
            v,
        )))
    }
    pub fn append_color(&self, p: Option<ColorMixingPoint>) -> Result<(), GradientError> {
        self.colors()?
            .push(p.map_or(GradientEntry::Null, GradientEntry::Color));
        Ok(())
    }
    pub fn append_alpha(&self, p: Option<AlphaMixingPoint>) -> Result<(), GradientError> {
        self.alphas()?
            .push(p.map_or(GradientEntry::Null, GradientEntry::Alpha));
        Ok(())
    }
    pub fn add_color(&self, x: i32, color: RgbColor) -> Result<(), GradientError> {
        self.append_color(Some(ColorMixingPoint::new(x, color)))?;
        self.colors()?.sort(false)
    }
    pub fn add_alpha(&self, x: i32, v: f64) -> Result<AlphaMixingPoint, GradientError> {
        let p = AlphaMixingPoint::new(x, v);
        self.append_alpha(Some(p.clone()))?;
        self.alphas()?.sort(true)?;
        Ok(p)
    }
    pub fn clear_points(&self) -> Result<(), GradientError> {
        self.colors()?.clear();
        self.alphas()?.clear();
        Ok(())
    }
    pub fn set_color_at(&self, i: i32, c: RgbColor) -> Result<(), GradientError> {
        self.color_point(i)?
            .ok_or(GradientError::NullReference)?
            .set_color(c);
        Ok(())
    }
    pub fn remove_color(&self, p: Option<ColorMixingPoint>) -> Result<(), GradientError> {
        self.colors()?
            .remove(&p.map_or(GradientEntry::Null, GradientEntry::Color))
    }
    pub fn remove_alpha(&self, p: Option<AlphaMixingPoint>) -> Result<(), GradientError> {
        self.alphas()?
            .remove(&p.map_or(GradientEntry::Null, GradientEntry::Alpha))
    }
    pub fn contains(&self, p: &GradientEntry) -> Result<bool, GradientError> {
        if self.colors()?.contains(p)? {
            Ok(true)
        } else {
            self.alphas()?.contains(p)
        }
    }
    pub fn point_reference_index(&self, p: &GradientEntry) -> Result<i32, GradientError> {
        let list = match p {
            GradientEntry::Color(_) => self.colors()?,
            GradientEntry::Alpha(_) => self.alphas()?,
            _ => return Ok(-1),
        };
        for i in 0..list.len() {
            if p.same_reference(&list.entry(i as i32)?) {
                return Ok(i as i32);
            }
        }
        Ok(-1)
    }
    pub fn nearest_color(&self, x: i32, radius: i32) -> Result<i32, GradientError> {
        let l = self.colors()?;
        let mut found = -1;
        for i in 0..l.len() {
            let distance = l
                .entry(i as i32)?
                .cast_color()?
                .coordinate()
                .wrapping_sub(x)
                .checked_abs()
                .ok_or(GradientError::Overflow)?;
            if distance <= radius {
                found = i as i32
            }
        }
        Ok(found)
    }
    pub fn nearest_alpha(&self, x: i32, radius: i32) -> Result<i32, GradientError> {
        let l = self.alphas()?;
        let (mut found, mut best) = (-1, i32::MAX);
        for i in 0..l.len() {
            let distance = l
                .entry(i as i32)?
                .cast_alpha()?
                .coordinate()
                .wrapping_sub(x)
                .checked_abs()
                .ok_or(GradientError::Overflow)?;
            if distance <= radius && distance < best {
                found = i as i32;
                best = distance
            }
        }
        Ok(found)
    }
    pub fn mirror(&self) -> Result<(), GradientError> {
        let l = self.colors()?;
        for i in 0..l.len() {
            let p = l.entry(i as i32)?.cast_color()?;
            p.set_coordinate(self.width().wrapping_sub(p.coordinate()));
        }
        let a = self.alphas()?;
        for i in 0..a.len() {
            let p = a.entry(i as i32)?.cast_alpha()?;
            p.set_coordinate(self.width().wrapping_sub(p.coordinate()));
        }
        l.reverse();
        a.reverse();
        Ok(())
    }
    pub fn source_clone(&self) -> Result<Self, GradientError> {
        let g = Self::with_width(self.width());
        let c = self.colors()?;
        for i in 0..c.len() {
            g.append_color(Some(c.entry(i as i32)?.cast_color()?.source_clone()))?;
        }
        let a = self.alphas()?;
        for i in 0..a.len() {
            g.append_alpha(Some(a.entry(i as i32)?.cast_alpha()?.source_clone()))?;
        }
        Ok(g)
    }
    pub fn source_equals(&self, other: Option<&Self>) -> Result<bool, GradientError> {
        let Some(other) = other else { return Ok(false) };
        let c = other.colors()?;
        let mine = self.colors()?;
        if c.len() != mine.len() {
            return Ok(false);
        }
        let a = other.alphas()?;
        let ma = self.alphas()?;
        if a.len() != ma.len() {
            return Ok(false);
        }
        for i in 0..mine.len() {
            if !mine.entry(i as i32)?.equals(&c.entry(i as i32)?)? {
                return Ok(false);
            }
        }
        for i in 0..ma.len() {
            if !ma.entry(i as i32)?.equals(&a.entry(i as i32)?)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn has_transparency(&self) -> Result<bool, GradientError> {
        let a = self.alphas()?;
        for i in 0..a.len() {
            if !(a.entry(i as i32)?.cast_alpha()?.value() - 1.0)
                .abs()
                .lt(&1e-5)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn sample(&self, t: f64) -> Result<RgbColor, GradientError> {
        self.buffer()?.sample(t)
    }
    pub fn insert_sampled_color(&self, x: i32) -> Result<ColorMixingPoint, GradientError> {
        let count = self.color_count()?;
        if count != 0 {
            self.rebuild()?
        }
        let x = x.max(0).min(self.width().wrapping_sub(1));
        let color = if count != 0 {
            self.buffer()?.value(x)?
        } else {
            RgbColor::from_argb([1.0, 0.0, 0.0, 0.0])
        };
        let p = ColorMixingPoint::new(x, color);
        self.append_color(Some(p.clone()))?;
        let mut c = p.color().argb();
        c[0] = 1.0;
        p.set_color(RgbColor::from_argb(c).grayscale(p.color().is_grayscale()));
        self.colors()?.sort(false)?;
        Ok(p)
    }
    pub fn insert_sampled_alpha(&self, x: i32) -> Result<AlphaMixingPoint, GradientError> {
        self.rebuild_alpha()?;
        let x = x.max(0).min(self.width().wrapping_sub(1));
        let value = self.buffer()?.value(x)?.argb()[0];
        self.add_alpha(x, value)
    }
    pub fn rebuild(&self) -> Result<(), GradientError> {
        if self.cache().is_none() {
            self.reset_cache()?
        }
        let c = self.buffer()?;
        let l = self.colors()?;
        if l.is_empty() {
            for i in 0..c.len() {
                for ch in 1..4 {
                    c.channel(i as i32, ch, 0.0)?
                }
            }
            return self.rebuild_alpha();
        }
        l.sort(false)?;
        let mut n = 0;
        let mut p = l.entry(0)?.color();
        let mut next = if l.len() > 1 {
            l.entry(1)?.color()
        } else {
            None
        };
        for x in 0..self.width() {
            let left = p.as_ref().ok_or(GradientError::NullReference)?;
            if (x <= left.coordinate() && n == 0) || n + 1 == l.len() {
                c.set(x, left.color())?;
                continue;
            }
            let right = next.as_ref().ok_or(GradientError::NullReference)?;
            let distance = f64::from(right.coordinate().wrapping_sub(left.coordinate()));
            if distance < 1e-5 {
                for ch in 1..4 {
                    c.channel(
                        x,
                        ch,
                        (left.color().argb()[ch] + right.color().argb()[ch]) / 2.0,
                    )?
                }
            } else {
                let part = 1.0 - f64::from(x.wrapping_sub(left.coordinate())) / distance;
                for ch in 1..4 {
                    c.channel(
                        x,
                        ch,
                        left.color().argb()[ch] * part + right.color().argb()[ch] * (1.0 - part),
                    )?
                }
            }
            if x >= right.coordinate() {
                n += 1;
                p = l.entry(n as i32)?.color();
                next = if l.len() > n + 1 {
                    l.entry(n as i32 + 1)?.color()
                } else {
                    None
                };
            }
        }
        self.rebuild_alpha()
    }
    fn rebuild_alpha(&self) -> Result<(), GradientError> {
        let a = self.alphas()?;
        if a.is_empty() {
            let c = self.buffer()?;
            for i in 0..c.len() {
                c.channel(i as i32, 0, 1.0)?
            }
            return Ok(());
        }
        a.sort(true)?;
        let mut n = 0;
        for x in 0..self.width() {
            let left = a
                .entry(n as i32)?
                .alpha()
                .ok_or(GradientError::NullReference)?;
            if x < left.coordinate() || n + 1 == a.len() {
                self.buffer()?.channel(x, 0, left.value())?;
                continue;
            }
            let right = a
                .entry(n as i32 + 1)?
                .alpha()
                .ok_or(GradientError::NullReference)?;
            let distance = f64::from(right.coordinate().wrapping_sub(left.coordinate()));
            let value = if distance < 1e-5 {
                (left.value() + right.value()) / 2.0
            } else {
                let part = 1.0 - f64::from(x.wrapping_sub(left.coordinate())) / distance;
                left.value() * part + right.value() * (1.0 - part)
            };
            self.buffer()?.channel(x, 0, value)?;
            if x >= right.coordinate() {
                n += 1
            }
        }
        Ok(())
    }
    pub fn render_channels(&self) -> Result<[Vec<f64>; 4], GradientError> {
        let temp = GradientCache::new(self.width())?;
        let old = self.cache();
        let width = self.width();
        self.set_cache(Some(temp.clone()));
        self.set_width(width);
        self.rebuild()?;
        self.set_cache(old);
        self.set_width(width);
        let colors = self.colors()?;
        let alphas = self.alphas()?;
        let mut channels: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
        for (ch, out) in channels.iter_mut().enumerate() {
            let mut constant = true;
            if ch < 3 {
                if colors.len() >= 2 {
                    let base = self
                        .color_point(0)?
                        .ok_or(GradientError::NullReference)?
                        .color()
                        .argb()[ch + 1];
                    for i in 1..colors.len() {
                        if !(self
                            .color_point(i as i32)?
                            .ok_or(GradientError::NullReference)?
                            .color()
                            .argb()[ch + 1]
                            - base)
                            .abs()
                            .lt(&1e-5)
                        {
                            constant = false;
                            break;
                        }
                    }
                }
            } else if alphas.len() >= 2 {
                let base = self
                    .alpha_point(0)?
                    .ok_or(GradientError::NullReference)?
                    .value();
                for i in 1..alphas.len() {
                    if !(self
                        .alpha_point(i as i32)?
                        .ok_or(GradientError::NullReference)?
                        .value()
                        - base)
                        .abs()
                        .lt(&1e-5)
                    {
                        constant = false;
                        break;
                    }
                }
            }
            out.resize(if constant { 1 } else { temp.len() }, 0.0);
        }
        if channels.iter().all(|v| v.len() == channels[0].len()) {
            for i in 0..channels[0].len() {
                let c = temp.value(i as i32)?.argb();
                for ch in 0..4 {
                    channels[ch][i] = if ch == 3 { 1.0 - c[0] } else { c[ch + 1] };
                }
            }
        } else {
            let c = temp.value(0)?.argb();
            for ch in 0..4 {
                channels[ch][0] = if ch == 3 { 1.0 - c[0] } else { c[ch + 1] };
            }
            for i in 1..temp.len() {
                let c = temp.value(i as i32)?.argb();
                for ch in 0..4 {
                    if channels[ch].len() > 1 {
                        channels[ch][i] = if ch == 3 { 1.0 - c[0] } else { c[ch + 1] };
                    }
                }
            }
        }
        Ok(channels)
    }
    pub fn load(&self, reader: &mut BinaryCursor<'_>) -> Result<(), GradientError> {
        self.clear_points()?;
        self.set_width(reader.read_i32()?);
        let colors = reader.read_i32()?;
        let alphas = reader.read_i32()?;
        for _ in 0..colors {
            let x = reader.read_i32()?;
            let mut c = [0.0; 4];
            for v in &mut c {
                *v = reader.read_f64()?;
            }
            self.append_color(Some(ColorMixingPoint::new(x, RgbColor::from_argb(c))))?;
        }
        for _ in 0..alphas {
            let x = reader.read_i32()?;
            let v = reader.read_f64()?;
            self.append_alpha(Some(AlphaMixingPoint::new(x, v)))?;
        }
        Ok(())
    }
    pub(crate) fn wire_record(&self) -> Result<GradientRecord, GradientError> {
        let colors = self.colors()?;
        let alphas = self.alphas()?;
        Ok(GradientRecord {
            width: self.width(),
            colors: (0..colors.len())
                .map(|index| {
                    let point = colors.entry(index as i32)?.cast_color()?;
                    Ok(crate::GradientColorPoint {
                        coordinate: point.coordinate(),
                        argb: point.color().argb(),
                    })
                })
                .collect::<Result<_, GradientError>>()?,
            transparency: (0..alphas.len())
                .map(|index| {
                    let point = alphas.entry(index as i32)?.cast_alpha()?;
                    Ok(crate::GradientTransparencyPoint {
                        coordinate: point.coordinate(),
                        value: point.value(),
                    })
                })
                .collect::<Result<_, GradientError>>()?,
        })
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), GradientError> {
        writer.write_i32(self.width())?;
        let colors = self.colors()?;
        writer.write_i32(colors.len() as i32)?;
        let alphas = self.alphas()?;
        writer.write_i32(alphas.len() as i32)?;
        for i in 0..colors.len() {
            let p = colors.entry(i as i32)?.cast_color()?;
            writer.write_i32(p.coordinate())?;
            for v in p.color().argb() {
                writer.write_f64(v)?;
            }
        }
        for i in 0..alphas.len() {
            let p = alphas.entry(i as i32)?.cast_alpha()?;
            writer.write_i32(p.coordinate())?;
            writer.write_f64(p.value())?;
        }
        Ok(())
    }
    pub fn to_base64(&self) -> Result<String, GradientError> {
        let mut w = BinaryWriter::default();
        self.save(&mut w)?;
        Ok(encode64(w.bytes()))
    }
    /// Original UTF-16 string units, including unpaired units. Invalid Base64
    /// or incomplete wire data is handled by this factory's fresh-default fallback.
    pub fn from_base64(text: Option<&[u16]>) -> Self {
        let g = Self::new();
        if let Some(bytes) = text.and_then(decode64)
            && let Ok(mut reader) = BinaryCursor::new(&bytes)
            && g.load(&mut reader).is_ok()
        {
            return g;
        }
        Self::new()
    }
}
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn encode64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(BASE64[(n >> 18) as usize] as char);
        out.push(BASE64[((n >> 12) & 63) as usize] as char);
        out.push(if c.len() > 1 {
            BASE64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            BASE64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
fn decode64(text: &[u16]) -> Option<Vec<u8>> {
    let input: Vec<u16> = text
        .iter()
        .copied()
        .filter(|c| !matches!(*c, 0x20 | 0x0d | 0x0a | 0x09))
        .collect();
    if !input.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::new();
    for (i, c) in input.chunks(4).enumerate() {
        let mut n = 0u32;
        let mut padding = 0;
        for (j, ch) in c.iter().enumerate() {
            let v = if *ch == u16::from(b'=') {
                if j < 2 || i + 1 != input.len() / 4 {
                    return None;
                }
                padding += 1;
                0
            } else {
                if padding > 0 {
                    return None;
                }
                BASE64.iter().position(|b| u16::from(*b) == *ch)? as u32
            };
            n = (n << 6) | v;
        }
        if padding > 2 {
            return None;
        }
        out.push((n >> 16) as u8);
        if padding < 2 {
            out.push((n >> 8) as u8)
        }
        if padding < 1 {
            out.push(n as u8)
        }
    }
    Some(out)
}

/// Scene-wide concrete Gradient copy context. Distinct Gradient parents can
/// share a list, point or cache; retain that sharing inside the isolated job.
#[derive(Default)]
pub(crate) struct GradientCapture {
    roots: Vec<(SharedGradient, SharedGradient)>,
    lists: Vec<(GradientPointList, GradientPointList)>,
    points: Vec<(GradientEntry, GradientEntry)>,
    caches: Vec<(GradientCache, GradientCache)>,
}
impl GradientCapture {
    fn list(&mut self, old: &GradientPointList) -> GradientPointList {
        if let Some((_, copy)) = self.lists.iter().find(|(v, _)| v.same_reference(old)) {
            return copy.clone();
        }
        let entries = guard(&old.0).clone();
        let copy = GradientPointList::new();
        for entry in entries {
            let next = if let Some((_, copy)) =
                self.points.iter().find(|(v, _)| v.same_reference(&entry))
            {
                copy.clone()
            } else {
                let next = match &entry {
                    GradientEntry::Null => GradientEntry::Null,
                    GradientEntry::Color(p) => {
                        let (x, c) = *guard(&p.0);
                        GradientEntry::Color(ColorMixingPoint::new(x, c))
                    }
                    GradientEntry::Alpha(p) => {
                        let (x, v) = *guard(&p.0);
                        GradientEntry::Alpha(AlphaMixingPoint::new(x, v))
                    }
                    GradientEntry::Integer(p) => GradientEntry::Integer(p.clone()),
                };
                self.points.push((entry, next.clone()));
                next
            };
            copy.push(next);
        }
        self.lists.push((old.clone(), copy.clone()));
        copy
    }
    fn cache(&mut self, old: &GradientCache) -> GradientCache {
        if let Some((_, copy)) = self.caches.iter().find(|(v, _)| v.same_reference(old)) {
            return copy.clone();
        }
        let copy = GradientCache::from_values(guard(&old.0).to_vec());
        self.caches.push((old.clone(), copy.clone()));
        copy
    }
    pub(crate) fn gradient(&mut self, old: &SharedGradient) -> SharedGradient {
        if let Some((_, copy)) = self.roots.iter().find(|(v, _)| v.same_reference(old)) {
            return copy.clone();
        }
        let (width, colors, alpha, cache) = {
            let s = guard(&old.0);
            (s.width, s.colors.clone(), s.alpha.clone(), s.cache.clone())
        };
        let colors = colors.as_ref().map(|l| self.list(l));
        let alpha = alpha.as_ref().map(|l| self.list(l));
        let cache = cache.as_ref().map(|c| self.cache(c));
        let copy = SharedGradient(Arc::new(Mutex::new(State {
            width,
            colors,
            alpha,
            cache,
        })));
        self.roots.push((old.clone(), copy.clone()));
        copy
    }
}

#[cfg(test)]
mod source_laws {
    use super::*;
    // Static original-body laws, not additional original-runtime goldens.
    #[test]
    fn repeated_point_mirror_and_occurrence_clone() {
        let g = SharedGradient::new();
        g.set_width(8);
        let p = ColorMixingPoint::new(2, RgbColor::default());
        g.append_color(Some(p.clone())).unwrap();
        g.append_color(Some(p.clone())).unwrap();
        g.mirror().unwrap();
        assert_eq!(p.coordinate(), 2);
        let copy = g.source_clone().unwrap();
        let a = copy.color_point(0).unwrap().unwrap();
        let b = copy.color_point(1).unwrap().unwrap();
        assert!(!a.same_reference(&b));
        assert!(!a.same_reference(&p));
        a.set_coordinate(7);
        assert_eq!(b.coordinate(), 2);
    }
    #[test]
    fn retained_point_has_no_parent_backedge() {
        let g = SharedGradient::new();
        let weak = Arc::downgrade(&g.0);
        let p = ColorMixingPoint::new(0, RgbColor::default());
        g.append_color(Some(p.clone())).unwrap();
        drop(g);
        assert!(weak.upgrade().is_none());
        p.set_coordinate(3);
        assert_eq!(p.coordinate(), 3);
    }
    #[test]
    fn aliased_lists_no_recursive_guard_and_partial_sort() {
        let g = SharedGradient::new();
        g.set_width(4);
        let l = g.color_points().unwrap();
        let a = ColorMixingPoint::new(3, RgbColor::default());
        let b = ColorMixingPoint::new(1, RgbColor::default());
        l.push(GradientEntry::Color(a));
        l.push(GradientEntry::Color(b.clone()));
        l.push(GradientEntry::Null);
        assert_eq!(g.rebuild(), Err(GradientError::NullReference));
        assert!(l.entry(0).unwrap().color().unwrap().same_reference(&b));
        l.clear();
        g.set_alpha_points(Some(l));
        g.rebuild().unwrap();
    }
    #[test]
    fn alpha_sort_precedes_null_cache_and_zero_width_skips_dereference() {
        let g = SharedGradient::new();
        g.set_width(0);
        let a = g.alpha_points().unwrap();
        let first = AlphaMixingPoint::new(4, 1.0);
        let second = AlphaMixingPoint::new(1, 0.0);
        a.push(GradientEntry::Alpha(first));
        a.push(GradientEntry::Alpha(second.clone()));
        assert!(g.cache().is_none());
        g.rebuild_alpha().unwrap();
        assert!(a.entry(0).unwrap().alpha().unwrap().same_reference(&second));
        assert!(g.cache().is_none());
    }
    #[test]
    fn partial_clear_load_keeps_old_cache_and_width() {
        let g = SharedGradient::new();
        g.resize_cache(4).unwrap();
        let cache = g.cache().unwrap();
        let old = g.color_points().unwrap();
        old.push(GradientEntry::Color(ColorMixingPoint::new(
            1,
            RgbColor::default(),
        )));
        g.set_alpha_points(None);
        assert_eq!(
            g.load(&mut BinaryCursor::new(&[]).unwrap()),
            Err(GradientError::NullReference)
        );
        assert!(old.is_empty());
        assert_eq!(g.width(), 4);
        assert!(cache.same_reference(&g.cache().unwrap()));
    }
    #[test]
    fn channels_postpublication_failure_keeps_temp_but_projection_failure_restores() {
        let g = SharedGradient::new();
        g.resize_cache(2).unwrap();
        let old = g.cache().unwrap();
        g.set_width(4);
        g.color_points().unwrap().push(GradientEntry::Null);
        assert_eq!(g.render_channels(), Err(GradientError::NullReference));
        assert!(!old.same_reference(&g.cache().unwrap()));
        assert_eq!(g.cache().unwrap().len(), 4);
        g.clear_points().unwrap();
        g.set_cache(Some(old.clone()));
        g.set_width(0);
        assert_eq!(g.render_channels(), Err(GradientError::Index));
        assert!(old.same_reference(&g.cache().unwrap()));
    }
    #[test]
    fn clipping_preserves_alpha_and_copy_flag_rules() {
        let p = ColorMixingPoint::new(
            0,
            RgbColor::from_argb([-2.0, f64::NAN, 2.0, -1.0]).grayscale(true),
        );
        let q = p.clipped_copy();
        assert_eq!(q.color().argb(), [-2.0, 0.0, 1.0, 0.0]);
        assert!(!q.color().is_grayscale());
        assert!(p.source_clone().color().is_grayscale());
    }
}
