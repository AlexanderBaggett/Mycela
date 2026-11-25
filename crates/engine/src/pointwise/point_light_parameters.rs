//! n7 constructor properties, using real os and aob owners.
use super::*;
pub(super) fn numeric<E>(
    name: &str,
    initial: f64,
    places: i32,
    step: DecimalBits,
    minimum: i32,
    maximum: i32,
    range: Option<(i32, i32, f64, [bool; 3])>,
) -> Result<ParameterState<E>, PointwiseError<E>> {
    let lo = decimal(minimum)?;
    let hi = decimal(maximum)?;
    let use_range = match range {
        Some((a, b, m, flags)) => {
            let mut r = RangeOptionsRecord::new(decimal(a)?, decimal(b)?);
            r.multiplier = m;
            r.flags = flags;
            Some(SharedRangeOptionsRecord::new(r))
        }
        None => Some(SharedRangeOptionsRecord::new(RangeOptionsRecord::new(
            lo, hi,
        ))),
    };
    let limits = SharedLimits::from_definition_with_profile(
        LimitsDefinition {
            decimal_places: places,
            step: LimitsStep::Derived(step),
            minimum: lo,
            maximum: hi,
            used: true,
            use_range,
            curve_range: None,
        },
        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
    )
    .map_err(|_| ImageError::Overflow)?;
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current: initial,
            previous: initial,
            limits,
        },
    })
}
fn light_position_parameter<E>() -> Result<ParameterState<E>, PointwiseError<E>> {
    let light = PointwiseCustomObject::new_light_position();
    let previous = light.source_clone()?;
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text("Light Position"),
        value: PointwiseParameterValue::Matrix {
            current: Some(light.raw()),
            previous: Some(previous.raw()),
        },
    })
}

pub(super) fn parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let hundredth = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let one = decimal(1)?;
    Ok(vec![
        light_position_parameter()?,
        numeric(
            "Height Factor",
            0.05,
            4,
            hundredth,
            -1_000_000,
            1_000_000,
            Some((-3, 3, 0.1, [false, true, true])),
        )?,
        numeric("Shadow Strength", 0.0, 1, one, 0, 100, None)?,
        numeric("Global Illumination", 50.0, 1, one, 0, 100, None)?,
        numeric(
            "Ambient Occlusion",
            0.0,
            1,
            one,
            0,
            1000,
            Some((0, 200, 1.0, [false, true, true])),
        )?,
        numeric("Occlusion Radius", 10.0, 1, one, 0, 100, None)?,
        choice("Skip for Color Only", &["Yes", "No"], 0),
    ])
}

/// zg's hidden legacy record has its own property order and Light Range.
pub(super) fn illuminate_g2_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let hundredth = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    Ok(vec![
        numeric(
            "Height Factor",
            0.05,
            3,
            hundredth,
            -1_000_000,
            1_000_000,
            Some((-3, 3, 0.1, [false, true, true])),
        )?,
        light_position_parameter()?,
        choice("Light Range", &["Half", "Full"], 0),
        choice("Skip for Color Only", &["Yes", "No"], 0),
    ])
}

/// ar7 constructor order differs from n7 while sharing the aob owner.
pub(super) fn shadow_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let hundredth = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    Ok(vec![
        numeric(
            "Height Factor",
            0.2,
            4,
            hundredth,
            -1_000_000,
            1_000_000,
            Some((-3, 3, 0.1, [false, true, true])),
        )?,
        light_position_parameter()?,
        numeric("Shadow Strength", 70.0, 1, decimal(1)?, 0, 100, None)?,
        choice("Skip for Color Only", &["Yes", "No"], 0),
    ])
}

/// atr retains Strength and Quality in older GTX files.
pub(super) fn shadow_g2_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let hundredth = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let tenth = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
    Ok(vec![
        numeric(
            "Height Factor",
            0.2,
            3,
            hundredth,
            -1_000_000,
            1_000_000,
            Some((-3, 3, 0.1, [false, true, true])),
        )?,
        light_position_parameter()?,
        numeric(
            "Strength",
            0.5,
            3,
            hundredth,
            0,
            1_000_000,
            Some((0, 100, 0.1, [false, true, true])),
        )?,
        numeric(
            "Quality",
            1.0,
            3,
            tenth,
            0,
            10,
            Some((0, 3, 1.0, [false, false, false])),
        )?,
        choice("Skip for Color Only", &["Yes", "No"], 0),
    ])
}

/// ps constructor reuses the same source property types with additional gains.
pub(super) fn full_light_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let mut shared = parameters()?.into_iter();
    let position = shared.next().ok_or(PointwiseError::Index)?;
    let height = shared.next().ok_or(PointwiseError::Index)?;
    let shadow = shared.next().ok_or(PointwiseError::Index)?;
    let gi = shared.next().ok_or(PointwiseError::Index)?;
    let ao = shared.next().ok_or(PointwiseError::Index)?;
    let radius = shared.next().ok_or(PointwiseError::Index)?;
    let skip = shared.next().ok_or(PointwiseError::Index)?;
    let one = decimal(1)?;
    Ok(vec![
        position,
        height,
        numeric("Direct Light", 100.0, 1, one, 0, 1000, None)?,
        shadow,
        gi,
        ao,
        radius,
        numeric("Enviro Strength", 50.0, 1, one, 0, 1000, None)?,
        numeric("Enviro Attenuation", 100.0, 1, one, 0, 1000, None)?,
        skip,
    ])
}

/// ath constructor keeps the real aob position and six source property roots.
pub(super) fn atmosphere_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let point = light_position_parameter()?;
    let tenth = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
    let hundredth = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let mut density = numeric("Sample Density", 1.0, 3, tenth, 0, 5, None)?;
    if let PointwiseParameterValue::Number { limits, .. } = &mut density.value {
        let mut range = RangeOptionsRecord::new(tenth, decimal(3)?);
        range.flags = [false, false, false];
        *limits = SharedLimits::from_definition_with_profile(
            LimitsDefinition {
                decimal_places: 3,
                step: LimitsStep::Derived(tenth),
                minimum: tenth,
                maximum: decimal(5)?,
                used: true,
                use_range: Some(SharedRangeOptionsRecord::new(range)),
                curve_range: None,
            },
            crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
        )
        .map_err(|_| ImageError::Overflow)?;
    }
    let mut detail = numeric(
        "Detail Scale",
        1.0,
        3,
        tenth,
        0,
        1_000_000,
        Some((0, 10, 1.0, [false, true, false])),
    )?;
    if let PointwiseParameterValue::Number { limits, .. } = &mut detail.value {
        let mut range = RangeOptionsRecord::new(decimal(0)?, decimal(10)?);
        range.flags = [false, true, false];
        let definition = LimitsDefinition {
            decimal_places: 3,
            step: LimitsStep::Derived(tenth),
            minimum: decimal(0)?,
            maximum: decimal(1_000_000)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(range)),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(0)?,
                maximum: decimal(3)?,
                wrap_y: false,
            })),
        };
        *limits = SharedLimits::from_definition_with_profile(
            definition,
            crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
        )
        .map_err(|_| ImageError::Overflow)?;
    }
    Ok(vec![
        point,
        numeric("Thickness Factor", 1.0, 3, tenth, 0, 5, None)?,
        numeric("Shadow", 0.2, 3, hundredth, 0, 1, None)?,
        density,
        detail,
        numeric("Seed", 0.0, 0, decimal(1)?, 0, 1_000_000, None)?,
    ])
}

/// ah6 constructor order matches the retained property stream.
pub(super) fn hair_parameters<E>() -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let zero = decimal(0)?;
    let one = decimal(1)?;
    let tenth = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
    let hundredth = DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
    let limits = |step, minimum, maximum, use_range, curve: Option<(DecimalBits, DecimalBits)>| {
        SharedLimits::from_definition_with_profile(
            LimitsDefinition {
                decimal_places: 3,
                step: LimitsStep::Derived(step),
                minimum,
                maximum,
                used: true,
                use_range: Some(SharedRangeOptionsRecord::new(use_range)),
                curve_range: curve.map(|(minimum, maximum)| {
                    SharedCurveRangeRecord::new(CurveRangeRecord {
                        minimum,
                        maximum,
                        wrap_y: false,
                    })
                }),
            },
            crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
        )
        .map_err(|_| ImageError::Overflow)
    };
    let mut density_range = RangeOptionsRecord::new(zero, decimal(10)?);
    density_range.multiplier = 0.2;
    density_range.flags = [false, true, true];
    let mut sample_range = RangeOptionsRecord::new(tenth, decimal(5)?);
    sample_range.flags = [false, false, false];
    let number = |name, current, limits| ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current,
            previous: current,
            limits,
        },
    };
    Ok(vec![
        light_position_parameter()?,
        numeric("Hair Length", 0.5, 3, hundredth, 0, 1, None)?,
        number(
            "Hair Density",
            0.8,
            limits(
                tenth,
                zero,
                decimal(1_000_000)?,
                density_range,
                Some((zero, decimal(3)?)),
            )?,
        ),
        ParameterState {
            extras: ParameterExtras::default(),
            choice_memory: None,
            name: text("Hair Direction"),
            value: PointwiseParameterValue::Pair {
                current: [2., -3.],
                previous: [2., -3.],
                limits: limits(
                    one,
                    decimal(-100)?,
                    decimal(100)?,
                    RangeOptionsRecord::new(decimal(-8)?, decimal(8)?),
                    Some((decimal(-20)?, decimal(20)?)),
                )?,
            },
        },
        numeric("Shine", 1., 3, tenth, 0, 10, None)?,
        numeric("Light Scatter", 1., 3, tenth, 0, 10, None)?,
        number(
            "Height Factor",
            0.2,
            limits(
                hundredth,
                zero,
                one,
                RangeOptionsRecord::new(
                    zero,
                    DecimalBits::new([4, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
                ),
                None,
            )?,
        ),
        number(
            "Sample Density",
            2.,
            limits(tenth, tenth, decimal(5)?, sample_range, None)?,
        ),
    ])
}
