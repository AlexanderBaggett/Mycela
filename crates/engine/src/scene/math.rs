//! TexBase ae4/apt/c7 and Vector3F arithmetic at the primitive mesh seam.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneVector {
    values: [f64; 3],
}
impl SceneVector {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { values: [x, y, z] }
    }
    pub const fn values(self) -> [f64; 3] {
        self.values
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct MeshVector {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) z: f32,
}
impl MeshVector {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub const fn values(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    pub(super) fn add(a: Self, b: Self) -> Self {
        Self::new(a.x + b.x, a.y + b.y, a.z + b.z)
    }
    pub(super) fn sub(a: Self, b: Self) -> Self {
        Self::new(a.x - b.x, a.y - b.y, a.z - b.z)
    }
    pub(super) fn scale(a: f32, b: Self) -> Self {
        Self::new(a * b.x, a * b.y, a * b.z)
    }
    pub(super) fn dot(self, b: Self) -> f32 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    pub(super) fn length(self) -> f32 {
        (f64::from(self.x * self.x + self.y * self.y + self.z * self.z)).sqrt() as f32
    }
    pub(super) fn normalize(&mut self) {
        let n = self.length();
        if n != 0. {
            let s = 1. / n;
            self.x *= s;
            self.y *= s;
            self.z *= s;
        }
    }
    // Source g() deliberately differs from b() for zero vectors.
    pub(super) fn unit(self) -> Self {
        let n = 1. / self.length();
        Self::new(self.x * n, self.y * n, self.z * n)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct MeshUv {
    u: f32,
    v: f32,
}
impl MeshUv {
    pub const fn new(u: f32, v: f32) -> Self {
        Self { u, v }
    }
    pub const fn values(self) -> [f32; 2] {
        [self.u, self.v]
    }
    pub(super) fn u(self) -> f32 {
        self.u
    }
    pub(super) fn v(self) -> f32 {
        self.v
    }
}
/// Source apt's sixteen-double row-major homogeneous transform, separate from
/// the six-double 2D image matrix. Values are retained without normalization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneTransform {
    values: [f64; 16],
}
impl Default for SceneTransform {
    fn default() -> Self {
        Self::from_values([
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ])
    }
}
impl SceneTransform {
    pub const fn from_values(values: [f64; 16]) -> Self {
        Self { values }
    }
    pub const fn values(self) -> [f64; 16] {
        self.values
    }
    pub fn scale(v: SceneVector) -> Self {
        let [x, y, z] = v.values;
        Self::from_values([x, 0., 0., 0., 0., y, 0., 0., 0., 0., z, 0., 0., 0., 0., 1.])
    }
    pub fn translation(v: SceneVector) -> Self {
        let mut a = Self::default();
        a.values[12..15].copy_from_slice(&v.values);
        a
    }
    pub fn then(self, b: Self) -> Self {
        let mut c = [0.; 16];
        for i in 0..4 {
            for j in 0..4 {
                c[i * 4 + j] = self.values[i * 4] * b.values[j]
                    + self.values[i * 4 + 1] * b.values[4 + j]
                    + self.values[i * 4 + 2] * b.values[8 + j]
                    + self.values[i * 4 + 3] * b.values[12 + j];
            }
        }
        Self::from_values(c)
    }
    fn axis(angle: f64, v: SceneVector) -> Self {
        let [x, y, z] = v.values;
        let c = angle.cos();
        let s = angle.sin();
        let q = 1. - c;
        Self::from_values([
            c + x * x * q,
            x * y * q - z * s,
            x * z * q + y * s,
            0.,
            y * x * q + z * s,
            c + y * y * q,
            y * z * q - x * s,
            0.,
            z * x * q - y * s,
            z * y * q + x * s,
            c + z * z * q,
            0.,
            0.,
            0.,
            0.,
            1.,
        ])
    }
    pub fn orientation(v: SceneVector) -> Self {
        let [h, p, r] = v.values.map(|x| x / 180. * std::f64::consts::PI);
        let sh = h.sin();
        let ch = h.cos();
        let cp = p.cos();
        let sp = p.sin();
        Self::default()
            .then(Self::from_values([
                ch, 0., -sh, 0., 0., 1., 0., 0., sh, 0., ch, 0., 0., 0., 0., 1.,
            ]))
            .then(Self::axis(p, SceneVector::new(ch, 0., -sh)))
            .then(Self::axis(r, SceneVector::new(sh * cp, sp, ch * cp)))
    }
    pub fn from_components(
        size: SceneVector,
        location: SceneVector,
        orientation: SceneVector,
    ) -> Self {
        Self::default()
            .then(Self::scale(size))
            .then(Self::orientation(orientation))
            .then(Self::translation(location))
    }
    pub fn transform_homogeneous(self, p: [f64; 4]) -> [f64; 4] {
        let a = self.values;
        std::array::from_fn(|j| a[j] * p[0] + a[4 + j] * p[1] + a[8 + j] * p[2] + a[12 + j] * p[3])
    }
    pub(super) fn point(self, p: MeshVector) -> MeshVector {
        let a = self.values;
        let x = f64::from(p.x);
        let y = f64::from(p.y);
        let z = f64::from(p.z);
        let mut v = [
            a[0] * x + a[4] * y + a[8] * z + a[12],
            a[1] * x + a[5] * y + a[9] * z + a[13],
            a[2] * x + a[6] * y + a[10] * z + a[14],
        ];
        let w = a[3] * x + a[7] * y + a[11] * z + a[15];
        let scale = 1. / w;
        for x in &mut v {
            *x *= scale;
        }
        MeshVector::new(v[0] as f32, v[1] as f32, v[2] as f32)
    }
    pub(super) fn direction(mut self, p: MeshVector) -> MeshVector {
        self.values[12..15].fill(0.);
        let mut out = self.point(p);
        out.normalize();
        out
    }
}
