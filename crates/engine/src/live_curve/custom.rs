//! hy: retained immutable boxed point values, working list and computed cache.
use super::*;
use crate::BinaryCursor;
mod spline;
static REBUILD: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Default)]
struct Point {
    x: f32,
    y: f32,
}
#[derive(Clone, Copy, Debug)]
struct PointValue {
    position: Point,
    incoming: i32,
    outgoing: i32,
}
/// Retains an actual boxed Custom point. Editing replaces a selected box;
/// clones and callers retaining this handle keep the old immutable value.
#[derive(Clone, Debug)]
pub struct CustomPointHandle(Arc<PointValue>);
impl CustomPointHandle {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn position(&self) -> [f32; 2] {
        [self.0.position.x, self.0.position.y]
    }
    pub fn incoming_tangent(&self) -> i32 {
        self.0.incoming
    }
    pub fn outgoing_tangent(&self) -> i32 {
        self.0.outgoing
    }
    fn new(value: PointValue) -> Self {
        Self(Arc::new(value))
    }
}
/// Retains the actual raw or working list independently of a layer handle.
/// Entries retain box identity; the returned Vec is a readonly projection.
#[derive(Clone, Debug, Default)]
pub struct CustomPointList(Arc<Mutex<Vec<CustomPointHandle>>>);
impl CustomPointList {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn entries(&self) -> Vec<CustomPointHandle> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn replace(&self, entries: Vec<CustomPointHandle>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = entries;
    }
    fn push(&self, value: PointValue) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(CustomPointHandle::new(value));
    }
    fn set<E>(&self, ix: i32, value: PointValue) -> Result<CustomPointHandle, CurveError<E>> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let ix = usize::try_from(ix).map_err(|_| CurveError::ArgumentIndex)?;
        let point = CustomPointHandle::new(value);
        *entries.get_mut(ix).ok_or(CurveError::ArgumentIndex)? = point.clone();
        Ok(point)
    }
}
#[derive(Clone)]
pub(super) struct State {
    raw: CustomPointList,
    selected: i32,
    looped: bool,
    cache: Option<CurveCache>,
    work: CustomPointList,
    dirty: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            raw: CustomPointList::default(),
            selected: -1,
            looped: true,
            cache: None,
            work: CustomPointList::default(),
            dirty: true,
        }
    }
}

// Current recorded cold rebuilds and tangent-resolution inputs have unique,
// finite X. Their unique final permutation is independent of Framework's
// runtime-selected sort branch. Equal/NaN ordering on a cold cache is not
// established by warm-load observations and remains an explicit capability gap.
fn order<E>(entries: &mut [CustomPointHandle]) -> Result<(), CurveError<E>> {
    for i in 0..entries.len() {
        if !entries[i].0.position.x.is_finite() {
            return Err(CurveError::UnsupportedCapability);
        }
        for j in 0..i {
            if entries[i].0.position.x == entries[j].0.position.x {
                return Err(CurveError::UnsupportedCapability);
            }
        }
    }
    // Matches retained Framework helper SorterObjectArray.InsertionSort06006885:
    // shift while Compare(value,old)<0. The source runtime branch was not
    // observed; unique finite X has one final permutation under either branch.
    for i in 1..entries.len() {
        let value = entries[i].clone();
        let mut j = i;
        while j > 0 && value.0.position.x < entries[j - 1].0.position.x {
            entries[j] = entries[j - 1].clone();
            j -= 1;
        }
        entries[j] = value;
    }
    Ok(())
}
impl<E> CurveLayerHandle<E> {
    pub fn new_custom() -> Self {
        Self::structure(CurveLayerKind::Custom)
    }
    fn with_custom<T>(&self, f: impl FnOnce(&State) -> T) -> Result<T, CurveError<E>> {
        self.0.read(|s| match s {
            Object::Layer(l) => l.custom.as_ref().map(f).ok_or(CurveError::WrongControl),
            _ => unreachable!(),
        })
    }
    fn edit_custom<T>(&self, f: impl FnOnce(&mut State) -> T) -> Result<T, CurveError<E>> {
        self.0.write(|s| match s {
            Object::Layer(l) => l.custom.as_mut().map(f).ok_or(CurveError::WrongControl),
            _ => unreachable!(),
        })
    }
    /// hy.d(): fresh caller-owned coordinate values, never retained source boxes.
    pub fn custom_point_values(&self) -> Result<Vec<[f32; 2]>, CurveError<E>> {
        Ok(self
            .custom_points()?
            .entries()
            .iter()
            .map(CustomPointHandle::position)
            .collect())
    }
    pub fn custom_points(&self) -> Result<CustomPointList, CurveError<E>> {
        self.with_custom(|s| s.raw.clone())
    }
    pub fn custom_working_points(&self) -> Result<CustomPointList, CurveError<E>> {
        self.with_custom(|s| s.work.clone())
    }
    pub fn custom_selected_index(&self) -> Result<i32, CurveError<E>> {
        self.with_custom(|s| s.selected)
    }
    pub fn is_custom_looped(&self) -> Result<bool, CurveError<E>> {
        self.with_custom(|s| s.looped)
    }
    pub fn is_custom_dirty(&self) -> Result<bool, CurveError<E>> {
        self.with_custom(|s| s.dirty)
    }
    pub fn custom_cache(&self) -> Result<Option<CurveCache>, CurveError<E>> {
        self.with_custom(|s| s.cache.clone())
    }
    pub fn set_custom_looped(&self, value: bool) -> Result<(), CurveError<E>> {
        self.edit_custom(|s| {
            s.looped = value;
            s.dirty = true
        })
    }
    /// Source hy.a(RectangleF, PointF): choose a point inside the hit rectangle.
    /// Its original nearest-point calculation deliberately uses X twice.
    pub fn select_custom_point(
        &self,
        rect: [f32; 4],
        target: [f32; 2],
    ) -> Result<bool, CurveError<E>> {
        let points = self.custom_points()?.entries();
        let mut candidates = Vec::new();
        for (index, point) in points.iter().enumerate() {
            let [x, y] = point.position();
            if x >= rect[0]
                && x < add(rect[0], rect[2])
                && y >= rect[1]
                && y < add(rect[1], rect[3])
            {
                candidates.push((index, x));
            }
        }
        if candidates.is_empty() {
            return Ok(false);
        }
        let selected = if candidates.len() == 1 {
            candidates[0].0 as i32
        } else {
            let mut minimum = f32::MAX;
            let mut selected = -1;
            for (index, x) in candidates {
                let dx = sub(x, target[0]);
                let distance = libm::sqrt(f64::from(add(mul(dx, dx), mul(dx, dx)))) as f32;
                if distance < minimum {
                    minimum = distance;
                    selected = index as i32;
                }
            }
            selected
        };
        self.edit_custom(|s| s.selected = selected)?;
        Ok(true)
    }

    /// Select a retained point from a list control, including coincident points.
    pub fn select_custom_point_handle(
        &self,
        point: &CustomPointHandle,
    ) -> Result<(), CurveError<E>> {
        self.edit_custom(|state| {
            let index = state
                .raw
                .entries()
                .iter()
                .position(|entry| entry.same_reference(point))
                .ok_or(CurveError::ArgumentIndex)?;
            state.selected = i32::try_from(index).map_err(|_| CurveError::Overflow)?;
            Ok(())
        })?
    }

    /// Source hy.g: remove the selected raw point, then clear selection and invalidate cache.
    pub fn remove_selected_custom_point(&self) -> Result<(), CurveError<E>> {
        let selected = self.custom_selected_index()?;
        if selected == -1 {
            return Ok(());
        }
        let raw = self.custom_points()?;
        {
            let mut entries = raw.0.lock().unwrap_or_else(|e| e.into_inner());
            let index = usize::try_from(selected).map_err(|_| CurveError::ArgumentIndex)?;
            if index >= entries.len() {
                return Err(CurveError::ArgumentIndex);
            }
            entries.remove(index);
        }
        self.edit_custom(|s| {
            s.selected = -1;
            s.dirty = true;
        })
    }
    /// Point Y uses the source percent units, before working-list normalization.
    pub fn add_custom_point(&self, x: f32, y: f32) -> Result<(), CurveError<E>> {
        let raw = self.custom_points()?;
        let mut position = Point { x, y };
        for entry in raw.entries() {
            if entry.0.position.x == position.x {
                position.x = add(position.x, 0.00001)
            }
        }
        if position.x >= 1.0 {
            position.x = 0.99999;
        }
        let mut entries = raw.entries();
        order::<E>(&mut entries)?;
        let mut incoming = 1;
        let mut outgoing = 1;
        let mut left = f32::MIN;
        let mut right = f32::MAX;
        for entry in &entries {
            let point = &entry.0;
            if left < point.position.x && point.position.x < position.x {
                left = point.position.x;
                incoming = point.outgoing;
            }
            if position.x < point.position.x && point.position.x < right {
                right = point.position.x;
                outgoing = point.incoming;
            }
        }
        if self.is_custom_looped()?
            && !entries.is_empty()
            && (position.x < entries[0].0.position.x
                || position.x > entries[entries.len() - 1].0.position.x)
        {
            incoming = entries[entries.len() - 1].0.outgoing;
            outgoing = entries[0].0.incoming;
        }
        let selected = i32::try_from(raw.entries().len()).map_err(|_| CurveError::Overflow)?;
        raw.push(PointValue {
            position,
            incoming,
            outgoing,
        });
        self.edit_custom(|s| {
            s.selected = selected;
            s.dirty = true
        })
    }
    // Preserve the source ordered branches and their NaN behavior.
    #[allow(clippy::manual_clamp)]
    pub fn move_selected_custom_point(&self, x: f32, y: f32) -> Result<(), CurveError<E>> {
        let selected = self.custom_selected_index()?;
        if selected == -1 {
            return Ok(());
        }
        let raw = self.custom_points()?;
        let entries = raw.entries();
        let previous = *entries
            .get(usize::try_from(selected).map_err(|_| CurveError::ArgumentIndex)?)
            .ok_or(CurveError::ArgumentIndex)?
            .0;
        let mut position = Point { x, y };
        for entry in entries {
            let point = entry.0.position;
            // PointF != compares both coordinates using ordinary Single equality.
            if sub(point.x, position.x).abs() < 0.001
                && (point.x != previous.position.x || point.y != previous.position.y)
            {
                position.x = add(position.x, 0.00001);
            }
        }
        if position.x > 1.0 {
            position.x = 1.0
        }
        if position.x < 0.0 {
            position.x = 0.0
        }
        if position.y > 1000.0 {
            position.y = 1000.0
        }
        if position.y < -1000.0 {
            position.y = -1000.0
        }
        raw.set(
            selected,
            PointValue {
                position,
                ..previous
            },
        )?;
        self.edit_custom(|s| s.dirty = true)
    }
    fn set_custom_tangent(&self, value: i32, incoming: bool) -> Result<(), CurveError<E>> {
        let selected = self.custom_selected_index()?;
        if selected == -1 {
            return Ok(());
        }
        let raw = self.custom_points()?;
        let entries = raw.entries();
        let mut point = *entries
            .get(usize::try_from(selected).map_err(|_| CurveError::ArgumentIndex)?)
            .ok_or(CurveError::ArgumentIndex)?
            .0;
        if incoming {
            point.incoming = value
        } else {
            point.outgoing = value
        }
        raw.set(selected, point)?;
        self.edit_custom(|s| s.dirty = true)
    }
    pub fn set_custom_incoming_tangent(&self, value: i32) -> Result<(), CurveError<E>> {
        self.set_custom_tangent(value, true)
    }
    pub fn set_custom_outgoing_tangent(&self, value: i32) -> Result<(), CurveError<E>> {
        self.set_custom_tangent(value, false)
    }
    pub(super) fn copy_custom(&self, original: &Self) -> Result<(), CurveError<E>> {
        let points = original.custom_points()?.entries();
        let looped = original.is_custom_looped()?;
        self.custom_points()?.replace(points);
        self.edit_custom(|s| s.looped = looped)
    }
    // Negating RangeInclusive::contains would reject source-accepted NaNs.
    #[allow(clippy::manual_range_contains)]
    pub(super) fn load_custom(&self, reader: &mut BinaryCursor<'_>) -> Result<(), CurveError<E>> {
        let looped = reader.read_bool()?;
        self.edit_custom(|s| s.looped = looped)?;
        let count = reader.read_i32()?;
        if count < 0 {
            return Err(CurveError::InvalidData);
        }
        let raw = self.custom_points()?;
        raw.replace(Vec::new());
        for _ in 0..count {
            let x = reader.read_f64()? as f32;
            let y = reader.read_f64()? as f32;
            let incoming = reader.read_i32()?;
            let outgoing = reader.read_i32()?;
            if x < 0.0
                || x > 1.0
                || y < -1000.0
                || y > 1000.0
                || !(0..=1).contains(&incoming)
                || !(0..=1).contains(&outgoing)
            {
                return Err(CurveError::InvalidData);
            }
            raw.push(PointValue {
                position: Point { x, y },
                incoming,
                outgoing,
            });
        }
        // Actual Load intentionally leaves selection, work, cache and dirty alone.
        Ok(())
    }
    pub(super) fn rebuild_custom(&self) -> Result<(), CurveError<E>> {
        let raw = self.custom_points()?;
        let work = self.custom_working_points()?;
        work.replace(raw.entries());
        let mut entries = work.entries();
        order::<E>(&mut entries)?;
        work.replace(entries.clone());
        let looped = self.is_custom_looped()?;
        if entries.len() > 1 && looped {
            let n = entries.len();
            let mut last = *entries[n - 1].0;
            let previous = *entries[n - 2].0;
            let first = *entries[0].0;
            let mut second = *entries[1].0;
            if last.position.x == previous.position.x {
                last.position.x = add(last.position.x, 0.00001)
            }
            let mut before_last = last;
            before_last.position.x = sub(-sub(1.0, before_last.position.x), 0.00001);
            let mut before_previous = previous;
            before_previous.position.x = sub(-sub(1.0, before_previous.position.x), 0.00001);
            if first.position.x == second.position.x {
                second.position.x = add(second.position.x, 0.00001)
            }
            let mut after_first = first;
            after_first.position.x = add(add(1.0, after_first.position.x), 0.00001);
            let mut after_second = second;
            after_second.position.x = add(add(1.0, after_second.position.x), 0.00001);
            for point in [before_last, before_previous, after_first, after_second] {
                entries.push(CustomPointHandle::new(point));
            }
            work.replace(entries.clone());
            order::<E>(&mut entries)?;
            work.replace(entries.clone());
        }
        let mut points = Vec::new();
        for (ix, entry) in entries.iter_mut().enumerate() {
            let mut point = *entry.0;
            point.position.y = div(point.position.y, 100.0);
            *entry = work.set(ix as i32, point)?;
            points.push(point.position);
        }
        let computed = spline::cache(&points, &entries, looped)?;
        let cache =
            CurveCache::allocate(i32::try_from(computed.len()).map_err(|_| CurveError::Overflow)?)?;
        for (ix, point) in computed.into_iter().enumerate() {
            cache.write(
                ix,
                CurveCachePoint {
                    x: point.x,
                    y: point.y,
                },
            )?;
        }
        self.edit_custom(|s| {
            s.cache = Some(cache);
            s.dirty = false
        })
    }
    /// Sample the Custom shape before layer frequency, amplitude and other
    /// controls. The resulting Y is normalized from the stored point percent.
    pub fn sample_custom(&self, time: f32) -> Result<f32, CurveError<E>> {
        if self.is_custom_dirty()? {
            let _guard = REBUILD.lock().unwrap_or_else(|e| e.into_inner());
            self.rebuild_custom()?;
        }
        let points = self
            .custom_cache()?
            .ok_or(CurveError::NullReference)?
            .points();
        for pair in points.windows(2) {
            if pair[0].x() <= time && time <= pair[1].x() {
                return Ok(add(
                    pair[0].y(),
                    div(
                        mul(sub(pair[1].y(), pair[0].y()), sub(time, pair[0].x())),
                        sub(pair[1].x(), pair[0].x()),
                    ),
                ));
            }
        }
        Ok(0.5)
    }
}

// Current-format Save reads the actual raw point list, never the sampled cache.
impl<E> CurveLayerHandle<E> {
    pub(super) fn save_custom(
        &self,
        writer: &mut crate::BinaryWriter,
    ) -> Result<(), CurveError<E>> {
        writer.write_bool(self.is_custom_looped()?)?;
        let entries = self.custom_points()?.entries();
        writer.write_i32(i32::try_from(entries.len()).map_err(|_| CurveError::Overflow)?)?;
        for entry in entries {
            let [x, y] = entry.position();
            writer.write_f64(f64::from(x))?;
            writer.write_f64(f64::from(y))?;
            writer.write_i32(entry.incoming_tangent())?;
            writer.write_i32(entry.outgoing_tangent())?;
        }
        Ok(())
    }
}

impl State {
    pub(super) fn detached_render_copy(&self) -> Self {
        fn list(old: &CustomPointList) -> CustomPointList {
            // Point boxes are immutable values; only the list needs new authority.
            CustomPointList(Arc::new(Mutex::new(old.entries())))
        }
        Self {
            raw: list(&self.raw),
            selected: self.selected,
            looped: self.looped,
            cache: self
                .cache
                .as_ref()
                .map(|c| CurveCache::from_render_points(c.points())),
            work: list(&self.work),
            dirty: self.dirty,
        }
    }
}

impl State {
    pub(super) fn render_values(&self, out: &mut Vec<u64>) {
        out.extend([self.selected as u64, self.looped as u64, self.dirty as u64]);
        let mut boxes: Vec<CustomPointHandle> = Vec::new();
        for list in [&self.raw, &self.work] {
            let entries = list.entries();
            out.push(entries.len() as u64);
            for p in entries {
                let id = if let Some(i) = boxes.iter().position(|q| q.same_reference(&p)) {
                    i
                } else {
                    boxes.push(p.clone());
                    boxes.len() - 1
                };
                let pos = p.position();
                out.extend([
                    id as u64,
                    pos[0].to_bits() as u64,
                    pos[1].to_bits() as u64,
                    p.incoming_tangent() as u64,
                    p.outgoing_tangent() as u64,
                ]);
            }
        }
        match &self.cache {
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

#[cfg(test)]
mod tests {
    use super::*;
    // These source-body laws are additional static tests, not additional
    // original numerical goldens or part of the16-case runtime denominator.
    #[test]
    fn source_law_clone_shares_point_boxes_but_edit_replaces_only_selected_box() {
        let layer = CurveLayerHandle::<()>::new_custom();
        layer.add_custom_point(0.2, 25.0).unwrap();
        layer.add_custom_point(0.8, 75.0).unwrap();
        layer.sample(0.5).unwrap();
        let raw = layer.custom_points().unwrap();
        let before = raw.entries();
        let cache = layer.custom_cache().unwrap().unwrap();
        let copy = layer.source_clone().unwrap();
        let copied = copy.custom_points().unwrap();
        assert!(!raw.same_reference(&copied));
        assert!(
            !layer
                .custom_working_points()
                .unwrap()
                .same_reference(&copy.custom_working_points().unwrap())
        );
        assert_eq!(copy.custom_selected_index().unwrap(), -1);
        assert!(copy.is_custom_dirty().unwrap());
        assert!(copy.custom_cache().unwrap().is_none());
        for (a, b) in before.iter().zip(copied.entries()) {
            assert!(a.same_reference(&b));
        }
        layer.move_selected_custom_point(0.6, 35.0).unwrap();
        let edited = raw.entries();
        assert!(edited[0].same_reference(&before[0]));
        assert!(!edited[1].same_reference(&before[1]));
        assert!(copied.entries()[1].same_reference(&before[1]));
        assert_eq!(before[1].position(), [0.8, 75.0]);
        assert!(
            layer
                .custom_cache()
                .unwrap()
                .unwrap()
                .same_reference(&cache)
        );
        layer.sample(0.5).unwrap();
        assert!(
            !layer
                .custom_cache()
                .unwrap()
                .unwrap()
                .same_reference(&cache)
        );
        copy.sample(0.5).unwrap();
        assert!(!copy.custom_cache().unwrap().unwrap().same_reference(&cache));
    }
    #[test]
    fn source_law_work_normalizes_new_boxes_and_loop_setter_dirties_equal_value() {
        let layer = CurveLayerHandle::<()>::new_custom();
        layer.add_custom_point(0.4, 25.0).unwrap();
        let raw = layer.custom_points().unwrap().entries()[0].clone();
        layer.sample(0.5).unwrap();
        let work = layer.custom_working_points().unwrap();
        let normalized = work.entries()[0].clone();
        assert!(!normalized.same_reference(&raw));
        assert_eq!(normalized.position(), [0.4, 0.25]);
        assert_eq!(raw.position(), [0.4, 25.0]);
        let cache = layer.custom_cache().unwrap().unwrap();
        assert!(!layer.is_custom_dirty().unwrap());
        layer.set_custom_looped(true).unwrap();
        assert!(layer.is_custom_dirty().unwrap());
        assert!(
            layer
                .custom_cache()
                .unwrap()
                .unwrap()
                .same_reference(&cache)
        );
        layer.sample(0.5).unwrap();
        assert!(work.same_reference(&layer.custom_working_points().unwrap()));
        assert!(!normalized.same_reference(&work.entries()[0]));
    }
    #[test]
    fn source_law_no_selection_mutators_are_noops_and_point_handles_need_no_owner() {
        let layer = CurveLayerHandle::<()>::new_custom();
        let weak = Arc::downgrade(&layer.0.core);
        let raw = layer.custom_points().unwrap();
        layer
            .move_selected_custom_point(f32::NAN, f32::INFINITY)
            .unwrap();
        layer.set_custom_incoming_tangent(i32::MIN).unwrap();
        layer.set_custom_outgoing_tangent(i32::MAX).unwrap();
        assert!(raw.entries().is_empty());
        assert_eq!(layer.custom_selected_index().unwrap(), -1);
        layer.add_custom_point(0.5, 50.0).unwrap();
        let point = raw.entries()[0].clone();
        drop(layer);
        assert!(
            weak.upgrade().is_none(),
            "point/list values have no back pointer to their former owner"
        );
        assert_eq!(point.position(), [0.5, 50.0]);
        assert!(raw.entries()[0].same_reference(&point));
    }
    #[test]
    fn list_selection_identifies_coincident_points_individually() {
        let curve = CurveLayerHandle::<()>::new_custom();
        curve.add_custom_point(1., 40.).unwrap();
        curve.add_custom_point(1., 40.).unwrap();
        let points = curve.custom_points().unwrap().entries();
        assert_eq!(points[0].position(), points[1].position());
        curve.select_custom_point_handle(&points[1]).unwrap();
        assert_eq!(curve.custom_selected_index().unwrap(), 1);
        curve.move_selected_custom_point(0.5, 20.).unwrap();
        assert_eq!(
            curve.custom_point_values().unwrap(),
            [points[0].position(), [0.5, 20.]]
        );
        assert!(curve.select_custom_point_handle(&points[1]).is_err());
    }

    #[test]
    fn selected_custom_point_hit_testing_and_removal_keep_real_point_boxes() {
        let layer = CurveLayerHandle::<()>::new_custom();
        layer.add_custom_point(0.2, 10.).unwrap();
        layer.add_custom_point(0.8, 90.).unwrap();
        let list = layer.custom_points().unwrap();
        let held = list.entries();
        assert!(
            layer
                .select_custom_point([0., 0., 1., 100.], [0.22, 90.])
                .unwrap()
        );
        assert_eq!(
            layer.custom_selected_index().unwrap(),
            0,
            "source distance uses X twice"
        );
        assert!(
            !layer
                .select_custom_point([0., 0., 0.1, 1.], [0., 0.])
                .unwrap()
        );
        assert_eq!(layer.custom_selected_index().unwrap(), 0);
        layer.remove_selected_custom_point().unwrap();
        assert_eq!(layer.custom_selected_index().unwrap(), -1);
        assert!(layer.is_custom_dirty().unwrap());
        assert_eq!(list.entries().len(), 1);
        assert!(list.entries()[0].same_reference(&held[1]));
        assert_eq!(held[0].position(), [0.2, 10.]);
    }
}
