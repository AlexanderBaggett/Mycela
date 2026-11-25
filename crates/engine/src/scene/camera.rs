//! Retained ab7/kl camera and its actual apt/ai8 result owners.
//! Renderer snapshots read these roots; no GPUI entity or GPU ID owns camera state.
use super::{SceneTransform, SceneVector};
use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}
/// One mutable source apt result. Clone retains the same matrix object.
#[derive(Clone)]
pub struct CameraTransform(Arc<Mutex<SceneTransform>>);
impl CameraTransform {
    pub fn new(value: SceneTransform) -> Self {
        Self(Arc::new(Mutex::new(value)))
    }
    pub fn value(&self) -> SceneTransform {
        *lock(&self.0)
    }
    pub fn set_value(&self, value: SceneTransform) {
        *lock(&self.0) = value;
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl fmt::Debug for CameraTransform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CameraTransform")
            .field(&self.value())
            .finish()
    }
}
/// The distinct mutable source ai8 normal matrix, in row-major order.
#[derive(Clone)]
pub struct CameraNormalTransform(Arc<Mutex<[f64; 9]>>);
impl CameraNormalTransform {
    pub fn values(&self) -> [f64; 9] {
        *lock(&self.0)
    }
    pub fn set_values(&self, values: [f64; 9]) {
        *lock(&self.0) = values;
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
/// The source event argument distinguishes view and projection invalidation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraChange {
    view: bool,
    projection: bool,
}
impl CameraChange {
    pub const fn changes_view(self) -> bool {
        self.view
    }
    pub const fn changes_projection(self) -> bool {
        self.projection
    }
}
type Listener<E> = Arc<dyn Fn(&SceneCamera<E>, CameraChange) -> Result<(), E> + Send + Sync>;
pub struct CameraHandler<E>(Listener<E>);
impl<E> Clone for CameraHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> CameraHandler<E> {
    pub fn new(
        handler: impl Fn(&SceneCamera<E>, CameraChange) -> Result<(), E> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(handler))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
/// A concrete invocation list. Removal follows Delegate.Remove's last matching
/// subsequence rule, including duplicate handlers.
pub struct CameraDelegate<E>(Vec<CameraHandler<E>>);
impl<E> Clone for CameraDelegate<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> CameraDelegate<E> {
    pub fn from_handlers(handlers: &[CameraHandler<E>]) -> Self {
        Self(handlers.to_vec())
    }
    pub fn handlers(&self) -> Vec<CameraHandler<E>> {
        self.0.clone()
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum CameraError<E> {
    Callback(E),
    NullMatrixFactory,
}
impl<E: fmt::Display> fmt::Display for CameraError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Callback(error) => write!(f, "camera callback: {error}"),
            Self::NullMatrixFactory => f.write_str("camera matrix factory is null"),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for CameraError<E> {}
type Factory<E> =
    Arc<dyn Fn(&SceneCamera<E>) -> Result<Option<CameraTransform>, CameraError<E>> + Send + Sync>;
/// A retained delegate binding used by ab7's overridable matrix factories.
pub struct CameraMatrixFactory<E>(Factory<E>, u8);
impl<E> Clone for CameraMatrixFactory<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), self.1)
    }
}
impl<E> CameraMatrixFactory<E> {
    pub fn new(
        factory: impl Fn(&SceneCamera<E>) -> Result<Option<CameraTransform>, CameraError<E>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(factory), 0)
    }
    pub fn same_binding(&self, other: &Self) -> bool {
        (self.1 != 0 && self.1 == other.1) || Arc::ptr_eq(&self.0, &other.0)
    }
}
#[derive(Clone, Copy)]
struct Orbit {
    elevation: f64,
    azimuth: f64,
    distance: f64,
    dirty: bool,
}
struct CameraState<E> {
    position: SceneVector,
    target: SceneVector,
    up: SceneVector,
    frustum: [f64; 4],
    near: f64,
    far: f64,
    auto_view: bool,
    auto_projection: bool,
    view: Option<CameraTransform>,
    projection: Option<CameraTransform>,
    inverse: Option<CameraTransform>,
    normal: Option<CameraNormalTransform>,
    composed: Option<CameraTransform>,
    view_factory: Option<CameraMatrixFactory<E>>,
    projection_factory: Option<CameraMatrixFactory<E>>,
    listeners: Vec<CameraHandler<E>>,
    orbit: Option<Orbit>,
}
pub struct SceneCamera<E>(Arc<Mutex<CameraState<E>>>);
/// Weak callback route for hosts whose event binding must not retain the camera.
pub struct WeakSceneCamera<E>(Weak<Mutex<CameraState<E>>>);
impl<E> Clone for WeakSceneCamera<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> WeakSceneCamera<E> {
    pub fn upgrade(&self) -> Option<SceneCamera<E>> {
        self.0.upgrade().map(SceneCamera)
    }
}
impl<E> Clone for SceneCamera<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E: 'static> Default for SceneCamera<E> {
    fn default() -> Self {
        Self::new()
    }
}
impl<E: 'static> SceneCamera<E> {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(CameraState {
            position: SceneVector::default(),
            target: SceneVector::default(),
            up: SceneVector::default(),
            frustum: [0.; 4],
            near: 0.,
            far: 0.,
            auto_view: true,
            auto_projection: true,
            view: None,
            projection: None,
            inverse: None,
            normal: None,
            composed: None,
            view_factory: Some(CameraMatrixFactory(
                Arc::new(|camera| Ok(Some(CameraTransform::new(camera.default_view())))),
                1,
            )),
            projection_factory: Some(CameraMatrixFactory(
                Arc::new(|camera| Ok(Some(CameraTransform::new(camera.default_projection())))),
                2,
            )),
            listeners: Vec::new(),
            orbit: None,
        })))
    }
    pub fn orbit() -> Self {
        let camera = Self::new();
        lock(&camera.0).orbit = Some(Orbit {
            elevation: 0.,
            azimuth: 0.,
            distance: 0.,
            dirty: true,
        });
        camera
    }
    /// z8's retained preview camera, before ak1 supplies an actual tile frustum.
    pub fn source_preview() -> Self {
        let camera = Self::orbit();
        {
            let mut state = lock(&camera.0);
            state.orbit = Some(Orbit {
                elevation: 70.,
                azimuth: 158.,
                distance: 2.5,
                dirty: true,
            });
            state.near = 1.;
            state.far = 1024.;
        }
        camera
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn downgrade(&self) -> WeakSceneCamera<E> {
        WeakSceneCamera(Arc::downgrade(&self.0))
    }
    pub fn view_factory(&self) -> Option<CameraMatrixFactory<E>> {
        lock(&self.0).view_factory.clone()
    }
    pub fn projection_factory(&self) -> Option<CameraMatrixFactory<E>> {
        lock(&self.0).projection_factory.clone()
    }
    /// Source mt: new camera, copied value vectors/factories/flags, fresh caches
    /// and no event subscribers. kl's copy starts with its own dirty=true field.
    pub fn source_copy(&self) -> Result<Self, CameraError<E>> {
        let source = lock(&self.0);
        let view_factory = source
            .view_factory
            .as_ref()
            .ok_or(CameraError::NullMatrixFactory)?
            .clone();
        let projection_factory = source
            .projection_factory
            .as_ref()
            .ok_or(CameraError::NullMatrixFactory)?
            .clone();
        Ok(Self(Arc::new(Mutex::new(CameraState {
            position: source.position,
            target: source.target,
            up: source.up,
            frustum: source.frustum,
            near: source.near,
            far: source.far,
            auto_view: source.auto_view,
            auto_projection: source.auto_projection,
            view: None,
            projection: None,
            inverse: None,
            normal: None,
            composed: None,
            view_factory: Some(view_factory),
            projection_factory: Some(projection_factory),
            listeners: Vec::new(),
            orbit: source.orbit.map(|orbit| Orbit {
                dirty: true,
                ..orbit
            }),
        }))))
    }
    pub fn position(&self) -> SceneVector {
        let mut state = lock(&self.0);
        if let Some(orbit) = state.orbit.filter(|orbit| orbit.dirty) {
            state.position = orbit_position(orbit.elevation, orbit.azimuth, orbit.distance);
            // kl.mp deliberately does not clear its dirty field.
        }
        state.position
    }
    pub fn target(&self) -> SceneVector {
        lock(&self.0).target
    }
    pub fn up(&self) -> SceneVector {
        let state = lock(&self.0);
        if let Some(orbit) = state.orbit {
            let direction = unit(sub(state.position, state.target));
            if direction.values()[1].abs() > 0.9999 {
                return negate(orbit_position(orbit.elevation + 90., orbit.azimuth, 1.));
            }
            SceneVector::new(0., 1., 0.)
        } else {
            state.up
        }
    }
    pub fn frustum(&self) -> [f64; 4] {
        lock(&self.0).frustum
    }
    pub fn near(&self) -> f64 {
        lock(&self.0).near
    }
    pub fn far(&self) -> f64 {
        lock(&self.0).far
    }
    pub fn orbit_values(&self) -> Option<[f64; 3]> {
        lock(&self.0)
            .orbit
            .map(|orbit| [orbit.elevation, orbit.azimuth, orbit.distance])
    }
    pub fn is_view_automatic(&self) -> bool {
        lock(&self.0).auto_view
    }
    pub fn is_projection_automatic(&self) -> bool {
        lock(&self.0).auto_projection
    }
    pub fn set_view_automatic(&self, automatic: bool) {
        lock(&self.0).auto_view = automatic;
    }
    pub fn set_projection_automatic(&self, automatic: bool) {
        lock(&self.0).auto_projection = automatic;
    }
    pub fn add_changed(&self, delegate: &CameraDelegate<E>) {
        lock(&self.0).listeners.extend(delegate.0.iter().cloned());
    }
    pub fn remove_changed(&self, delegate: &CameraDelegate<E>) {
        let mut state = lock(&self.0);
        let count = delegate.0.len();
        if count == 0 || count > state.listeners.len() {
            return;
        }
        if let Some(start) = state.listeners.windows(count).rposition(|window| {
            window
                .iter()
                .zip(&delegate.0)
                .all(|(a, b)| a.same_reference(b))
        }) {
            state.listeners.drain(start..start + count);
        }
    }
    fn changed(&self, change: CameraChange) -> Result<(), CameraError<E>> {
        let listeners = {
            let mut state = lock(&self.0);
            if change.view {
                if state.auto_view {
                    state.view = None;
                }
                state.inverse = None;
                state.normal = None;
                state.composed = None;
            }
            if change.projection {
                if state.auto_projection {
                    state.projection = None;
                }
                state.composed = None;
            }
            state.listeners.clone()
        };
        for handler in listeners {
            (handler.0)(self, change).map_err(CameraError::Callback)?;
        }
        Ok(())
    }
    pub fn set_position(&self, position: SceneVector) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            if state.orbit.is_some() || state.position == position {
                return Ok(());
            }
            state.position = position;
            CameraChange {
                view: state.auto_view,
                projection: false,
            }
        };
        self.changed(change)
    }
    pub fn set_target(&self, target: SceneVector) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            if state.target == target {
                return Ok(());
            }
            state.target = target;
            CameraChange {
                view: state.auto_view,
                projection: false,
            }
        };
        self.changed(change)
    }
    pub fn set_up(&self, up: SceneVector) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            if state.orbit.is_some() || state.up == up {
                return Ok(());
            }
            state.up = up;
            CameraChange {
                view: state.auto_view,
                projection: false,
            }
        };
        self.changed(change)
    }
    pub fn set_frustum(&self, frustum: [f64; 4]) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            state.frustum = frustum;
            CameraChange {
                view: false,
                projection: state.auto_projection,
            }
        };
        self.changed(change)
    }
    pub fn set_frustum_bound(
        &self,
        bound: CameraFrustumBound,
        value: f64,
    ) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            let slot = &mut state.frustum[bound as usize];
            if *slot == value {
                return Ok(());
            }
            *slot = value;
            CameraChange {
                view: false,
                projection: state.auto_projection,
            }
        };
        self.changed(change)
    }
    pub fn set_near(&self, near: f64) -> Result<(), CameraError<E>> {
        self.set_depth(near, false)
    }
    pub fn set_far(&self, far: f64) -> Result<(), CameraError<E>> {
        self.set_depth(far, true)
    }
    fn set_depth(&self, value: f64, far: bool) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            let slot = if far { &mut state.far } else { &mut state.near };
            if *slot == value {
                return Ok(());
            }
            *slot = value;
            CameraChange {
                view: false,
                projection: state.auto_projection,
            }
        };
        self.changed(change)
    }
    pub fn set_orbit_component(
        &self,
        component: CameraOrbitComponent,
        value: f64,
    ) -> Result<bool, CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            let Some(orbit) = &mut state.orbit else {
                return Ok(false);
            };
            let slot = match component {
                CameraOrbitComponent::Elevation => &mut orbit.elevation,
                CameraOrbitComponent::Azimuth => &mut orbit.azimuth,
                CameraOrbitComponent::Distance => &mut orbit.distance,
            };
            if *slot == value {
                return Ok(true);
            }
            *slot = value;
            orbit.dirty = true;
            CameraChange {
                view: state.auto_view,
                projection: false,
            }
        };
        self.changed(change)?;
        Ok(true)
    }
    pub fn set_view(&self, view: Option<CameraTransform>) -> Result<(), CameraError<E>> {
        {
            let mut state = lock(&self.0);
            state.view = view;
            state.auto_view = false;
        }
        self.changed(CameraChange {
            view: true,
            projection: false,
        })
    }
    pub fn set_projection(
        &self,
        projection: Option<CameraTransform>,
    ) -> Result<(), CameraError<E>> {
        {
            let mut state = lock(&self.0);
            state.projection = projection;
            state.auto_projection = false;
        }
        self.changed(CameraChange {
            view: false,
            projection: true,
        })
    }
    pub fn set_view_factory(
        &self,
        factory: Option<CameraMatrixFactory<E>>,
    ) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            if same_factory(&state.view_factory, &factory) {
                return Ok(());
            }
            state.view_factory = factory;
            CameraChange {
                view: state.auto_view,
                projection: false,
            }
        };
        self.changed(change)
    }
    pub fn set_projection_factory(
        &self,
        factory: Option<CameraMatrixFactory<E>>,
    ) -> Result<(), CameraError<E>> {
        let change = {
            let mut state = lock(&self.0);
            if same_factory(&state.projection_factory, &factory) {
                return Ok(());
            }
            state.projection_factory = factory;
            CameraChange {
                view: false,
                projection: state.auto_projection,
            }
        };
        self.changed(change)
    }
    pub fn view(&self) -> Result<Option<CameraTransform>, CameraError<E>> {
        let factory = {
            let state = lock(&self.0);
            if let Some(view) = &state.view {
                return Ok(Some(view.clone()));
            }
            state.view_factory.clone()
        };
        let factory = factory.ok_or(CameraError::NullMatrixFactory)?;
        let result = (factory.0)(self)?;
        lock(&self.0).view = result.clone();
        Ok(result)
    }
    pub fn projection(&self) -> Result<Option<CameraTransform>, CameraError<E>> {
        let factory = {
            let state = lock(&self.0);
            if let Some(projection) = &state.projection {
                return Ok(Some(projection.clone()));
            }
            state.projection_factory.clone()
        };
        let factory = factory.ok_or(CameraError::NullMatrixFactory)?;
        let result = (factory.0)(self)?;
        lock(&self.0).projection = result.clone();
        Ok(result)
    }
    pub fn view_projection(&self) -> Result<Option<CameraTransform>, CameraError<E>> {
        if let Some(value) = &lock(&self.0).composed {
            return Ok(Some(value.clone()));
        }
        let view = self.view()?;
        let projection = self.projection()?;
        let result = view
            .zip(projection)
            .map(|(view, projection)| CameraTransform::new(view.value().then(projection.value())));
        lock(&self.0).composed = result.clone();
        Ok(result)
    }
    pub fn inverse_view(&self) -> Result<Option<CameraTransform>, CameraError<E>> {
        if let Some(value) = &lock(&self.0).inverse {
            return Ok(Some(value.clone()));
        }
        let result = self.view()?.map(|view| {
            CameraTransform::new(SceneTransform::from_values(inverse(
                view.value().values(),
                4,
            )))
        });
        lock(&self.0).inverse = result.clone();
        Ok(result)
    }
    pub fn normal(&self) -> Result<Option<CameraNormalTransform>, CameraError<E>> {
        if let Some(value) = &lock(&self.0).normal {
            return Ok(Some(value.clone()));
        }
        let result = self.view()?.map(|view| {
            let values = view.value().values();
            let mut rounded = [0.; 16];
            for row in 0..3 {
                for column in 0..3 {
                    rounded[row * 3 + column] = f64::from(values[row * 4 + column] as f32);
                }
            }
            let inverted = inverse(rounded, 3);
            let mut normal = [0.; 9];
            for row in 0..3 {
                for column in 0..3 {
                    normal[row * 3 + column] = inverted[column * 3 + row];
                }
            }
            CameraNormalTransform(Arc::new(Mutex::new(normal)))
        });
        lock(&self.0).normal = result.clone();
        Ok(result)
    }
    fn default_view(&self) -> SceneTransform {
        let position = self.position();
        let target = self.target();
        let up = self.up();
        SceneTransform::scale(SceneVector::new(-1., 1., 1.)).then(look_at(position, target, up))
    }
    fn default_projection(&self) -> SceneTransform {
        let state = lock(&self.0);
        let [left, right, bottom, top] = state.frustum.map(|value| value * 0.4);
        perspective(left, right, bottom, top, state.near, state.far)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraFrustumBound {
    Left,
    Right,
    Bottom,
    Top,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraOrbitComponent {
    Elevation,
    Azimuth,
    Distance,
}

fn same_factory<E>(
    left: &Option<CameraMatrixFactory<E>>,
    right: &Option<CameraMatrixFactory<E>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.same_binding(right),
        _ => false,
    }
}
fn orbit_position(elevation: f64, azimuth: f64, distance: f64) -> SceneVector {
    let angle = elevation / 180. * std::f64::consts::PI;
    let bearing = azimuth / 180. * std::f64::consts::PI;
    SceneVector::new(
        (-distance) * bearing.sin() * angle.sin(),
        distance * angle.cos(),
        distance * bearing.cos() * angle.sin(),
    )
}
fn sub(a: SceneVector, b: SceneVector) -> SceneVector {
    let a = a.values();
    let b = b.values();
    SceneVector::new(a[0] - b[0], a[1] - b[1], a[2] - b[2])
}
fn negate(a: SceneVector) -> SceneVector {
    let a = a.values();
    SceneVector::new(-a[0], -a[1], -a[2])
}
fn unit(a: SceneVector) -> SceneVector {
    let a = a.values();
    let factor = 1. / (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    SceneVector::new(a[0] * factor, a[1] * factor, a[2] * factor)
}
fn cross(a: SceneVector, b: SceneVector) -> SceneVector {
    let a = a.values();
    let b = b.values();
    SceneVector::new(
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )
}
pub(super) fn look_at(
    position: SceneVector,
    target: SceneVector,
    up: SceneVector,
) -> SceneTransform {
    let forward = unit(sub(position, target));
    let side = unit(cross(up, forward));
    let vertical = unit(cross(forward, side));
    let [x, y, z] = side.values();
    let [a, b, c] = vertical.values();
    let [u, v, w] = forward.values();
    SceneTransform::translation(negate(position)).then(SceneTransform::from_values([
        x, a, u, 0., y, b, v, 0., z, c, w, 0., 0., 0., 0., 1.,
    ]))
}
fn perspective(
    left: f64,
    right: f64,
    bottom: f64,
    top: f64,
    near: f64,
    far: f64,
) -> SceneTransform {
    SceneTransform::from_values([
        2. * near / (right - left),
        0.,
        0.,
        0.,
        0.,
        2. * near / (top - bottom),
        0.,
        0.,
        (right + left) / (right - left),
        (top + bottom) / (top - bottom),
        (-(far + near)) / (far - near),
        -1.,
        0.,
        0.,
        (-(2. * far * near)) / (far - near),
        0.,
    ])
}
fn minor(values: &[f64; 16], size: usize, row: usize, column: usize) -> [f64; 16] {
    let mut output = [0.; 16];
    let mut destination = 0;
    for r in 0..size {
        if r == row {
            continue;
        }
        for c in 0..size {
            if c == column {
                continue;
            }
            output[destination] = values[r * size + c];
            destination += 1;
        }
    }
    output
}
fn determinant(values: &[f64; 16], size: usize) -> f64 {
    if size == 2 {
        return values[0] * values[3] - values[1] * values[2];
    }
    let mut result = 0.;
    let mut sign = 1.;
    for column in 0..size {
        result += sign * values[column] * determinant(&minor(values, size, 0, column), size - 1);
        sign = 0. - sign;
    }
    result
}
fn inverse(values: [f64; 16], size: usize) -> [f64; 16] {
    let det = determinant(&values, size);
    if det == 0. {
        return values;
    }
    let factor = 1. / det;
    let mut output = [0.; 16];
    for row in 0..size {
        for column in 0..size {
            let sign = if (row + column) % 2 == 0 { 1. } else { -1. };
            output[column * size + row] =
                sign * determinant(&minor(&values, size, row, column), size - 1) * factor;
        }
    }
    output
}

#[cfg(test)]
mod laws;
