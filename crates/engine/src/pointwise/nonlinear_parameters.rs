//! Literal nonlinear/color node constructor declarations. Each creates the actual retained
//! parameter/Limits/range owners used by rendering, UI and live wire methods.
use super::*;
fn number_with<E>(
    name: &str,
    value: f64,
    definition: LimitsDefinition,
) -> Result<ParameterState<E>, ImageError> {
    let limits = SharedLimits::from_definition_with_profile(
        definition,
        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
    )
    .map_err(|_| ImageError::Overflow)?;
    Ok(ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current: value,
            previous: value,
            limits,
        },
    })
}
fn fraction(scale: u32) -> Result<DecimalBits, ImageError> {
    DecimalBits::new([1, 0, 0, scale << 16]).map_err(|_| ImageError::Argument)
}
pub(super) fn frequency<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let frequency = number_with(
        "Frequency",
        2.,
        LimitsDefinition {
            decimal_places: 4,
            step: LimitsStep::Derived(fraction(1)?),
            minimum: decimal(-1_000_000)?,
            maximum: decimal(1_000_000)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(RangeOptionsRecord::new(
                decimal(-10)?,
                decimal(10)?,
            ))),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(0)?,
                maximum: decimal(10)?,
                wrap_y: false,
            })),
        },
    )?;
    let phase = number_with(
        "Phase",
        0.,
        LimitsDefinition {
            decimal_places: 4,
            step: LimitsStep::Derived(fraction(2)?),
            minimum: decimal(-1)?,
            maximum: decimal(1)?,
            used: true,
            use_range: None,
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(0)?,
                maximum: decimal(1)?,
                wrap_y: true,
            })),
        },
    )?;
    Ok(vec![
        frequency,
        phase,
        choice("Modifier", &["None", "Saw Tooth", "Sinusoid"], 0),
    ])
}
pub(super) fn fractalize<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let iterations = number_with(
        "Iterations",
        5.,
        LimitsDefinition {
            decimal_places: 0,
            step: LimitsStep::Derived(decimal(1)?),
            minimum: decimal(0)?,
            maximum: decimal(20)?,
            used: true,
            use_range: None,
            curve_range: None,
        },
    )?;
    let mut range = RangeOptionsRecord::new(decimal(-3)?, decimal(3)?);
    range.multiplier = 0.01;
    let falloff = number_with(
        "Falloff",
        0.,
        LimitsDefinition {
            decimal_places: 3,
            step: LimitsStep::Derived(fraction(1)?),
            minimum: decimal(-1000)?,
            maximum: decimal(1000)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(range)),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(-3)?,
                maximum: decimal(3)?,
                wrap_y: false,
            })),
        },
    )?;
    Ok(vec![
        iterations,
        falloff,
        choice("Mode", &["Blend", "Overlap"], 0),
        choice(
            "Transform",
            &[
                "0 Degrees",
                "45 Degrees",
                "90 Degrees",
                "135 Degrees",
                "180 Degrees",
                "Alternate Ending",
                "Rollback",
                "Speedy Exit",
                "Tailspin",
                "Winding Ways",
            ],
            0,
        ),
    ])
}

pub(super) fn exposure<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let make = |name, value, min, max, lo, hi, multiplier, curve_lo, curve_hi| {
        let mut range = RangeOptionsRecord::new(decimal(lo)?, decimal(hi)?);
        range.multiplier = multiplier;
        number_with(
            name,
            value,
            LimitsDefinition {
                decimal_places: 3,
                step: LimitsStep::Derived(fraction(1)?),
                minimum: decimal(min)?,
                maximum: decimal(max)?,
                used: true,
                use_range: Some(SharedRangeOptionsRecord::new(range)),
                curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                    minimum: decimal(curve_lo)?,
                    maximum: decimal(curve_hi)?,
                    wrap_y: false,
                })),
            },
        )
    };
    let exposure = make("Exposure", 0., -20, 20, -20, 20, 0.1, -2, 2)?;
    let mut gamma_range = RangeOptionsRecord::new(
        DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?,
        decimal(100)?,
    );
    gamma_range.multiplier = 0.05;
    let gamma = number_with(
        "Gamma",
        1.,
        LimitsDefinition {
            decimal_places: 3,
            step: LimitsStep::Derived(fraction(1)?),
            minimum: DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?,
            maximum: decimal(100)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(gamma_range)),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: DecimalBits::new([2, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?,
                maximum: decimal(5)?,
                wrap_y: false,
            })),
        },
    )?;
    Ok(vec![exposure, gamma])
}
pub(super) fn selector<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let color = ParameterState {
        extras: ParameterExtras::with_ordinary_limits(),
        choice_memory: None,
        name: text("Color"),
        value: PointwiseParameterValue::Color(crate::ContextColorValue::unlinked([0.; 3])),
    };
    let mut transparency_range = RangeOptionsRecord::new(decimal(0)?, decimal(1)?);
    transparency_range.multiplier = 1.;
    transparency_range.flags = [false, true, true];
    let transparency = number_with(
        "Transparency",
        0.,
        LimitsDefinition {
            decimal_places: 3,
            step: LimitsStep::Derived(fraction(2)?),
            minimum: decimal(-1_000_000)?,
            maximum: decimal(1_000_000)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(transparency_range)),
            curve_range: None,
        },
    )?;
    let mut tolerance_range = RangeOptionsRecord::new(decimal(0)?, decimal(10)?);
    tolerance_range.multiplier = 0.07;
    let tolerance = number_with(
        "Tolerance",
        1.,
        LimitsDefinition {
            decimal_places: 3,
            step: LimitsStep::Derived(fraction(1)?),
            minimum: decimal(0)?,
            maximum: decimal(100)?,
            used: true,
            use_range: Some(SharedRangeOptionsRecord::new(tolerance_range)),
            curve_range: Some(SharedCurveRangeRecord::new(CurveRangeRecord {
                minimum: decimal(0)?,
                maximum: decimal(1)?,
                wrap_y: false,
            })),
        },
    )?;
    Ok(vec![color, transparency, tolerance])
}

// h's four-argument Limits constructor uses its real default ad4 child.
pub(super) fn match_color<E>() -> Result<Vec<ParameterState<E>>, ImageError> {
    let make = |name, value| {
        number_with(
            name,
            value,
            LimitsDefinition {
                decimal_places: 1,
                step: LimitsStep::Derived(decimal(1)?),
                minimum: decimal(0)?,
                maximum: decimal(100)?,
                used: true,
                use_range: None,
                curve_range: None,
            },
        )
    };
    Ok(vec![make("Strength", 90.)?, make("Conformity", 60.)?])
}
