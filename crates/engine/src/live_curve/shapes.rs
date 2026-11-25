//! Source-specific analytic shape mathematics, separated from retained roots.
use super::gain;
// nq.a06001827: the logarithm result is narrowed THEN unary-negated.
pub(super) fn arc(time: f32, control: f32) -> f32 {
    let mut curve = (control * 0.01_f32) * 2.0_f32 - 1.0_f32;
    curve = if curve > 0.999_f32 {
        10.0_f32
    } else if curve >= 0.0_f32 {
        log_base_two(1.0_f64 / (1.0_f64 - f64::from(curve))) as f32
    } else if curve >= -0.999_f32 {
        -(log_base_two(1.0_f64 / (1.0_f64 + f64::from(curve))) as f32)
    } else {
        -10.0_f32
    };
    let positive = f64::from(curve) >= 0.0_f64;
    let mut value = time;
    let mut previous = value;
    let mut iterations = f64::from(curve.abs()) + 1.0_f64;
    while iterations > 1.0_f64 {
        previous = value;
        value = if positive {
            let offset = f64::from(value) - 1.0_f64;
            libm::sqrt(1.0_f64 - offset * offset) as f32
        } else {
            // Source multiplies value*value as f32 BEFORE widening.
            1.0_f32 - libm::sqrt(1.0_f64 - f64::from(value * value)) as f32
        };
        iterations -= 1.0_f64;
    }
    if iterations > 0.0_f64 {
        let fraction = iterations as f32;
        let remainder = 1.0_f32 - fraction;
        value = previous * remainder + value * fraction;
    }
    value
}

// aeq.a060019e5, called both by the shape and actual normalization initializer.
fn bell_density(mut time: f32, width: f32) -> f32 {
    time = ((1.0_f32 + 2.0_f32 * width) * (time - 0.5_f32)) / 3.0_f32;
    let exponent = (-18.0_f32 * time) * time;
    ((libm::exp(f64::from(exponent)) * 6.0_f64) / libm::sqrt(std::f64::consts::PI * 2.0_f64)) as f32
}
pub(super) fn bell(mut time: f32, mut skew: f32, mut width: f32) -> f32 {
    // Caller must evaluate actual Skew control first and Width second at original
    // shape input, before either transform. The closure/getters are not cached.
    if skew != 0.5_f32 {
        skew = 0.998_f32 * skew + 0.001_f32;
        time = 0.5_f32 * time + 0.5_f32;
        time = gain(time, (1.0_f32 - skew) * 100.0_f32);
        time = (time - 0.5_f32) * 2.0_f32;
    }
    if width != 0.5_f32 {
        width = 0.998_f32 * width + 0.001_f32;
        time = gain(time, (1.0_f32 - width) * 100.0_f32);
    }
    // Real algorithm-derived constants, never captured expected output bits.
    static NORMALIZATION: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();
    let &(minimum, peak) = NORMALIZATION.get_or_init(|| {
        (
            bell_density(0.0_f32, 1.0_f32),
            bell_density(0.5_f32, 1.0_f32),
        )
    });
    (bell_density(time, 1.0_f32) - minimum) / (peak - minimum)
}
pub(super) fn steps<E>(
    time: f32,
    count: f32,
    smoothness: impl FnOnce() -> Result<f32, E>,
) -> Result<f32, E> {
    let count = libm::round(f64::from(count)) as f32;
    let step = 1.0_f32 / count;
    let mut bucket = 0_i32;
    let mut boundary = 1_i32;
    // Do not turn the second ordered < into >=: NaN would change its branch.
    while (boundary as f32) * step < 1.0_f32 {
        if time < (boundary as f32) * step {
            break;
        }
        bucket = bucket.wrapping_add(1);
        boundary = boundary.wrapping_add(1);
    }
    let local = if time < 1.0_f32 {
        (time - (bucket as f32) * step) / step
    } else {
        1.0_f32
    };
    let percent = 100.0_f32 - 50.0_f32 * smoothness()?;
    let value = gain(local, percent);
    Ok((bucket as f32) * step + value * step)
}
// gain must retain the existing source-ordered rational function. The complete
// Bell060019e4 and Steps06001d08 CIL bodies are byte-identical97b1e4fd... .
// Original Framework Math.Log(double,double)0600103d, base fixed2:
// NaN(a) returns a before the internal Log calls; otherwise unary Log(a),
// unary Log(2), then division. Native Log exactness remains runtime-scoped.
fn log_base_two(value: f64) -> f64 {
    if value.is_nan() {
        value
    } else {
        libm::log(value) / libm::log(2.0)
    }
}
