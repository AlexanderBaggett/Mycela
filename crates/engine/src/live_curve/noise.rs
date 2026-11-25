//! fh/awm: actual computed retained arrays, published before element writes.
use super::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct CurveCachePoint {
    pub(super) x: f32,
    pub(super) y: f32,
}
impl CurveCachePoint {
    pub fn x(self) -> f32 {
        self.x
    }
    pub fn y(self) -> f32 {
        self.y
    }
}
#[derive(Clone, Debug)]
pub struct CurveCache(Arc<Mutex<Box<[CurveCachePoint]>>>);
impl CurveCache {
    pub(super) fn from_render_points(points: Vec<CurveCachePoint>) -> Self {
        Self(Arc::new(Mutex::new(points.into_boxed_slice())))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn points(&self) -> Vec<CurveCachePoint> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).to_vec()
    }
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(super) fn allocate<E>(length: i32) -> Result<Self, CurveError<E>> {
        let length = usize::try_from(length).map_err(|_| CurveError::Overflow)?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(length)
            .map_err(|_| CurveError::Allocation)?;
        points.resize(length, CurveCachePoint::default());
        Ok(Self(Arc::new(Mutex::new(points.into_boxed_slice()))))
    }
    pub(super) fn write<E>(
        &self,
        index: usize,
        point: CurveCachePoint,
    ) -> Result<(), CurveError<E>> {
        let mut points = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *points.get_mut(index).ok_or(CurveError::Index)? = point;
        Ok(())
    }
    fn interval<E>(&self, time: f32) -> Result<[CurveCachePoint; 2], CurveError<E>> {
        let points = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for pair in points.windows(2) {
            if pair[0].x <= time && time < pair[1].x {
                return Ok([pair[0], pair[1]]);
            }
        }
        let index = points.len().checked_sub(2).ok_or(CurveError::Index)?;
        Ok([points[index], points[index + 1]])
    }
}
#[derive(Clone, Default)]
pub(super) struct State {
    base: Option<CurveCache>,
    jitter: Option<CurveCache>,
}

// Native Framework64 conv.i4 profile: finite representable inputs truncate;
// NaN, infinities and overflow produce Int32.MinValue. The accepted wire
// capture includes30 separately qualified JIT probes. Source then performs
// Math.Abs(seed) or allocates the computed count, preserving their own errors.
use crate::framework_cast::integer;

impl<E> CurveLayerHandle<E> {
    pub fn new_noise() -> Result<Self, CurveError<E>> {
        let result = Self::structure(CurveLayerKind::Noise);
        result.bind_noise();
        result.rebuild_noise(false)?;
        Ok(result)
    }
    pub(super) fn bind_noise(&self) {
        let owner = self.0.core.id;
        for (slot, jitter) in [(0, false), (2, true), (3, true), (4, false), (5, false)] {
            let core = Arc::new(HandlerCore {
                callback: Box::new(move |emitter: &Root<E>, ()| {
                    CurveLayerHandle(emitter.at(owner)).rebuild_noise(jitter)
                }),
            });
            let control = self.shape_controls()[slot].clone();
            // Only the owner ID is captured. The actual emitting handle supplies
            // cohort ownership during invocation, including replaced old controls.
            control.add_changed(&CurveControlHandler(core));
        }
    }
    pub fn noise_caches(&self) -> Option<[Option<CurveCache>; 2]> {
        self.0.read(|state| match state {
            Object::Layer(layer) => layer
                .noise
                .as_ref()
                .map(|n| [n.base.clone(), n.jitter.clone()]),
            _ => unreachable!(),
        })
    }
    /// Source declared property setter. Replaced controls keep their bindings;
    /// Seed and Loop rebuild after the replacement has committed.
    pub fn replace_noise_control(
        &self,
        slot: usize,
        control: &CurveControlHandle<E>,
    ) -> Result<(), CurveError<E>> {
        if self.kind() != CurveLayerKind::Noise || slot >= 6 {
            return Err(CurveError::WrongControl);
        }
        let expected = match slot {
            1 | 2 => CurveControlKind::Modulated,
            5 => CurveControlKind::Toggle,
            _ => CurveControlKind::Scalar,
        };
        if control.kind() != expected {
            return Err(CurveError::WrongControl);
        }
        self.0.join(&control.0);
        self.0.write(|state| {
            if let Object::Layer(layer) = state {
                layer.extra[slot] = control.0.core.id
            }
        });
        if slot >= 4 {
            self.rebuild_noise(false)?
        }
        Ok(())
    }
    pub(super) fn rebuild_noise(&self, jitter_only: bool) -> Result<(), CurveError<E>> {
        let controls = self.shape_controls();
        let seed = integer(f64::from(controls[4].scalar()?));
        let mut rng = crate::seeded_random::SeededRandom::from_seed(seed)
            .map_err(|_| CurveError::Overflow)?;
        let count = if jitter_only {
            let frequency = libm::round(f64::from(controls[3].scalar()?));
            let base = libm::round(f64::from(controls[0].scalar()?));
            integer(frequency * base)
        } else {
            integer(libm::round(f64::from(controls[0].scalar()?)))
        };
        let cache = CurveCache::allocate(count.wrapping_add(1))?;
        self.0.write(|state| {
            if let Object::Layer(layer) = state {
                let n = layer.noise.as_mut().expect("Noise state");
                if jitter_only {
                    n.jitter = Some(cache.clone())
                } else {
                    n.base = Some(cache.clone())
                }
            }
        });
        let length = cache.len();
        for i in 0..length.saturating_sub(1) {
            let x = div(i as f32, (length - 1) as f32);
            let y = rng.sample() as f32;
            cache.write(
                i,
                CurveCachePoint {
                    x,
                    y: if jitter_only { sub(y, 0.5) } else { y },
                },
            )?;
        }
        let last = length.checked_sub(1).ok_or(CurveError::Index)?;
        // X commits before the Loop getter and the final Y write.
        cache.write(last, CurveCachePoint { x: 1.0, y: 0.0 })?;
        let y = if controls[5].toggle()? {
            cache.points()[0].y
        } else {
            let y = rng.sample() as f32;
            if jitter_only { sub(y, 0.5) } else { y }
        };
        cache.write(last, CurveCachePoint { x: 1.0, y })?;
        if !jitter_only {
            self.rebuild_noise(true)?
        }
        Ok(())
    }
    pub(super) fn sample_noise(&self, time: f32) -> Result<f32, CurveError<E>> {
        let caches = self.noise_caches().ok_or(CurveError::WrongControl)?;
        let [left, right] = caches[0]
            .as_ref()
            .ok_or(CurveError::NullReference)?
            .interval(time)?;
        let ratio = div(sub(time, left.x), sub(right.x, left.x));
        let controls = self.shape_controls();
        let blend = gain(ratio, mul(controls[1].evaluate(time)?, 100.0));
        let mut value = add(left.y, mul(sub(right.y, left.y), blend));
        let amplitude = controls[2].evaluate(time)?;
        value = add(div(amplitude, 2.0), mul(value, sub(1.0, amplitude)));
        if amplitude != 0.0 {
            let [left, right] = caches[1]
                .as_ref()
                .ok_or(CurveError::NullReference)?
                .interval(time)?;
            let low = mul(left.y, amplitude);
            let high = mul(right.y, amplitude);
            value = add(
                value,
                add(
                    low,
                    div(mul(sub(high, low), sub(time, left.x)), sub(right.x, left.x)),
                ),
            );
        }
        Ok(value)
    }
}

impl State {
    pub(super) fn detached_render_copy(&self) -> Self {
        fn copy(cache: &CurveCache) -> CurveCache {
            CurveCache(Arc::new(Mutex::new(cache.points().into_boxed_slice())))
        }
        Self {
            base: self.base.as_ref().map(copy),
            jitter: self.jitter.as_ref().map(copy),
        }
    }
}

impl State {
    pub(super) fn render_values(&self, out: &mut Vec<u64>) {
        for c in [&self.base, &self.jitter] {
            match c {
                Some(c) => {
                    let p = c.points();
                    out.push(p.len() as u64 + 1);
                    for p in p {
                        out.extend([p.x().to_bits() as u64, p.y().to_bits() as u64]);
                    }
                }
                None => out.push(0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // These30 observer JIT conversions are not extra Genetica function goldens.
    // Their runtime qualification is pinned separately from the full wire cases.
    #[test]
    fn native_conv_i4_matches_actual_qualified_primitive_observations() {
        use std::io::BufRead;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../port/evidence/curve-wire-native-wire-native-qualified-r1-original.jsonl",
        );
        let first = std::io::BufReader::new(std::fs::File::open(path).unwrap())
            .lines()
            .next()
            .unwrap()
            .unwrap();
        let header: serde_json::Value = serde_json::from_str(&first).unwrap();
        let observations = header["observer_cast_probes"]["observations"]
            .as_array()
            .unwrap();
        assert_eq!(observations.len(), 30);
        for row in observations {
            let bits = row["bits"].as_str().unwrap();
            let value = if row["type"] == "System.Single" {
                f64::from(f32::from_bits(u32::from_str_radix(bits, 16).unwrap()))
            } else {
                assert_eq!(row["type"], "System.Double");
                f64::from_bits(u64::from_str_radix(bits, 16).unwrap())
            };
            assert_eq!(row["outcome"]["kind"], "return");
            assert_eq!(
                i64::from(integer(value)),
                row["outcome"]["value"].as_i64().unwrap()
            );
        }
    }

    // Static source-body law: the committed nonfinite parameter flows through
    // conv.i4 into real count allocation/Math.Abs, before cache replacement.
    #[test]
    fn native_invalid_conversion_preserves_caches_and_typed_internal_failure() {
        for slot in [0, 3, 4] {
            let layer = CurveLayerHandle::<&'static str>::new_noise().unwrap();
            let control = layer.shape_controls()[slot].clone();
            let before = layer.noise_caches().unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let called = Arc::clone(&count);
            let handler = CurveControlHandler::new(move || {
                called.fetch_add(1, Ordering::SeqCst);
                Err("tail must be stopped")
            });
            control.add_changed(&handler);
            assert_eq!(control.set_scalar(f32::NAN), Err(CurveError::Overflow));
            assert!(control.scalar().unwrap().is_nan());
            let after = layer.noise_caches().unwrap();
            for i in 0..2 {
                assert!(
                    before[i]
                        .as_ref()
                        .unwrap()
                        .same_reference(after[i].as_ref().unwrap())
                );
            }
            assert_eq!(count.load(Ordering::SeqCst), 0);
            control.remove_changed(&handler);
        }
    }

    // Source-body laws supplement the original18 replay; these chosen inputs
    // are not additional runtime goldens or part of its numeric denominator.
    #[test]
    fn source_law_replaced_control_keeps_noise_receiver_then_reclaims_cohort() {
        let layer = CurveLayerHandle::<&'static str>::new_noise().unwrap();
        let weak = Arc::downgrade(&layer.0.core);
        let old = layer.shape_controls()[0].clone();
        let replacement = CurveLayerHandle::new_noise().unwrap().shape_controls()[0].clone();
        replacement.set_scalar(3.0).unwrap();
        layer.replace_noise_control(0, &replacement).unwrap();
        let before = layer.noise_caches().unwrap()[0].clone().unwrap();
        drop(layer);
        assert!(weak.upgrade().is_some());
        // The old emitter's new scalar is ignored by the owner after replacement.
        old.set_scalar(-1.0).unwrap();
        let owner = CurveLayerHandle(old.0.at(weak.upgrade().unwrap().id));
        let cache = owner.noise_caches().unwrap()[0].clone().unwrap();
        assert_eq!(cache.len(), 4);
        assert!(!cache.same_reference(&before));
        drop(owner);
        drop(replacement);
        drop(old);
        assert!(
            weak.upgrade().is_none(),
            "internal bound callbacks must not form a sponsor cycle"
        );
    }
    #[test]
    fn source_law_internal_errors_are_typed_and_partial_publication_is_retained() {
        let layer = CurveLayerHandle::<&'static str>::new_noise().unwrap();
        let controls = layer.shape_controls();
        let before = layer.noise_caches().unwrap();
        assert_eq!(controls[0].set_scalar(-1.0), Err(CurveError::Index));
        assert_eq!(controls[0].scalar().unwrap(), -1.0);
        let failed = layer.noise_caches().unwrap();
        assert!(failed[0].as_ref().unwrap().is_empty());
        assert!(
            !failed[0]
                .as_ref()
                .unwrap()
                .same_reference(before[0].as_ref().unwrap())
        );
        assert!(
            failed[1]
                .as_ref()
                .unwrap()
                .same_reference(before[1].as_ref().unwrap())
        );
        assert_eq!(
            controls[4].set_scalar(i32::MIN as f32),
            Err(CurveError::Overflow)
        );
        let seed_failed = layer.noise_caches().unwrap();
        for i in 0..2 {
            assert!(
                seed_failed[i]
                    .as_ref()
                    .unwrap()
                    .same_reference(failed[i].as_ref().unwrap())
            );
        }
        assert!(matches!(layer.source_clone(), Err(CurveError::Overflow)));
    }
    #[test]
    fn source_law_reentry_uses_captured_chain_after_commit_without_model_guards() {
        let layer = CurveLayerHandle::<&'static str>::new_noise().unwrap();
        let control = layer.shape_controls()[0].clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let c = control.clone();
        let l = layer.clone();
        let seen = Arc::clone(&calls);
        let entered = Arc::clone(&count);
        let a = CurveControlHandler::new(move || {
            let pass = entered.fetch_add(1, Ordering::SeqCst);
            seen.lock().unwrap().push((
                "a",
                c.scalar().unwrap().to_bits(),
                l.noise_caches().unwrap()[0].as_ref().unwrap().len(),
            ));
            if pass == 0 {
                c.set_scalar(3.0).unwrap();
            }
            Ok(())
        });
        let seen = Arc::clone(&calls);
        let b = CurveControlHandler::new(move || {
            seen.lock().unwrap().push(("b", 0, 0));
            Ok(())
        });
        control.add_changed(&a);
        control.add_changed(&b);
        let captured = control.changed_delegate().unwrap();
        control.set_scalar(2.0).unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                ("a", 2.0f32.to_bits(), 3),
                ("a", 3.0f32.to_bits(), 4),
                ("b", 0, 0),
                ("b", 0, 0)
            ]
        );
        control.remove_changed(&a);
        control.remove_changed(&b);
        assert_eq!(control.changed_delegate().unwrap().invocation_count(), 1);
        assert_eq!(captured.invocation_count(), 3);
        calls.lock().unwrap().clear();
        captured.invoke().unwrap();
        assert_eq!(calls.lock().unwrap().len(), 2);
        drop(captured);
        drop(a);
        drop(b);
    }
    #[test]
    fn source_law_user_error_wraps_once_after_real_rebuild() {
        let layer = CurveLayerHandle::<&'static str>::new_noise().unwrap();
        let control = layer.shape_controls()[0].clone();
        let handler = CurveControlHandler::new(|| Err("user"));
        control.add_changed(&handler);
        assert_eq!(control.set_scalar(2.0), Err(CurveError::Observer("user")));
        assert_eq!(layer.noise_caches().unwrap()[0].as_ref().unwrap().len(), 3);
        // A source error in the first internal binding prevents the user callback.
        assert_eq!(control.set_scalar(-1.0), Err(CurveError::Index));
        control.remove_changed(&handler);
    }
}
