//! Literal ok/yo and six concrete constructor declarations.
use super::*;
fn definition(
    places: i32,
    step: DecimalBits,
    min: i32,
    max: i32,
    range: Option<RangeOptionsRecord>,
    curve: Option<CurveRangeRecord>,
) -> Result<SharedLimits, ImageError> {
    SharedLimits::from_definition_with_profile(
        LimitsDefinition {
            decimal_places: places,
            step: LimitsStep::Derived(step),
            minimum: decimal(min)?,
            maximum: decimal(max)?,
            used: true,
            use_range: range.map(SharedRangeOptionsRecord::new),
            curve_range: curve.map(SharedCurveRangeRecord::new),
        },
        crate::LimitsArithmeticProfile::Framework64OleAut32TenFirst,
    )
    .map_err(|_| ImageError::Overflow)
}
fn range(a: i32, b: i32, places: i32) -> Result<RangeOptionsRecord, ImageError> {
    let mut r = RangeOptionsRecord::new(decimal(a)?, decimal(b)?);
    r.decimal_places = places;
    Ok(r)
}
fn curve(a: i32, b: i32, wrap: bool) -> Result<CurveRangeRecord, ImageError> {
    Ok(CurveRangeRecord {
        minimum: decimal(a)?,
        maximum: decimal(b)?,
        wrap_y: wrap,
    })
}
fn pair<E>(
    name: &str,
    initial: [f64; 2],
    limits: SharedLimits,
    mode: PointwiseCloneMode,
    size: bool,
) -> ParameterState<E> {
    ParameterState {
        extras: ParameterExtras {
            size: size.then(|| retained_spatial::SizeNormalization {
                normalized: if mode == PointwiseCloneMode::Original {
                    initial
                } else {
                    [0.; 2]
                },
                scale: 1.,
                flags: (mode == PointwiseCloneMode::Original).then(|| vec![false; 20]),
            }),
            ..ParameterExtras::default()
        },
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Pair {
            current: initial,
            previous: initial,
            limits,
        },
    }
}
fn number<E>(name: &str, value: f64, limits: SharedLimits) -> ParameterState<E> {
    ParameterState {
        extras: ParameterExtras::default(),
        choice_memory: None,
        name: text(name),
        value: PointwiseParameterValue::Number {
            current: value,
            previous: value,
            limits,
        },
    }
}
pub(super) fn size_parameter<E>(
    mode: PointwiseCloneMode,
) -> Result<ParameterState<E>, PointwiseError<E>> {
    Ok(pair(
        "Size",
        [1.; 2],
        definition(
            3,
            decimal(1)?,
            -1000000,
            1000000,
            Some(range(-10, 10, 0)?),
            Some(curve(0, 10, false)?),
        )?,
        mode,
        true,
    ))
}

pub(super) fn parameters<E>(
    kind: PointwiseKind,
    mode: PointwiseCloneMode,
) -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    if kind == PointwiseKind::Remap {
        return Ok(Vec::new());
    }
    let mut result = vec![size_parameter(mode)?];
    match kind {
        PointwiseKind::LegacyBricksBoards => {
            let tenth = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
            let hundredth =
                DecimalBits::new([1, 0, 0, 2 << 16]).map_err(|_| ImageError::Argument)?;
            result.extend([
                pair(
                    "Location",
                    [0.; 2],
                    definition(
                        3,
                        tenth,
                        -1_000_000,
                        1_000_000,
                        Some(range(-10, 10, 0)?),
                        Some(curve(0, 1, true)?),
                    )?,
                    mode,
                    false,
                ),
                pair(
                    "Block Density",
                    [8., 3.],
                    definition(0, decimal(1)?, 1, 1_000_000, Some(range(1, 20, 0)?), None)?,
                    mode,
                    false,
                ),
                number(
                    "Border Width",
                    0.03,
                    definition(3, hundredth, 0, 1, None, None)?,
                ),
                number(
                    "Shifts",
                    3.,
                    definition(
                        3,
                        decimal(1)?,
                        -1_000_000,
                        1_000_000,
                        Some(range(-30, 30, 0)?),
                        None,
                    )?,
                ),
                number(
                    "Width Variation",
                    0.,
                    definition(3, decimal(1)?, 0, 1_000_000, Some(range(0, 5, 0)?), None)?,
                ),
                number(
                    "Slide Variation",
                    0.,
                    definition(
                        3,
                        decimal(1)?,
                        -1_000_000,
                        1_000_000,
                        Some(RangeOptionsRecord::new(decimal(-5)?, decimal(5)?)),
                        None,
                    )?,
                ),
                choice("Fill Mode", &crate::pointwise::legacy_bricks::FILL_MODES, 0),
                choice("Orientation", &["Horizontal", "Vertical"], 0),
            ]);
        }
        PointwiseKind::Distort => {
            result.push(number(
                "Steps",
                1.,
                definition(0, decimal(1)?, 1, 100, None, Some(curve(1, 30, false)?))?,
            ));
            result.push(choice("Anti-Alias", &["Yes", "No"], 0));
        }
        PointwiseKind::FreeTransform | PointwiseKind::MoveRepeat => {
            let tenth = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
            result.push(pair(
                "Location",
                [0.; 2],
                definition(
                    3,
                    tenth,
                    -1000000,
                    1000000,
                    Some(range(-10, 10, 0)?),
                    Some(if kind == PointwiseKind::MoveRepeat {
                        curve(0, 1, true)?
                    } else {
                        curve(0, 10, false)?
                    }),
                )?,
                mode,
                false,
            ));
            if kind == PointwiseKind::FreeTransform {
                result.push(pair(
                    "Skew",
                    [0.; 2],
                    definition(
                        3,
                        tenth,
                        -1000000,
                        1000000,
                        Some(range(-10, 10, 0)?),
                        Some(curve(0, 10, false)?),
                    )?,
                    mode,
                    false,
                ));
                let mut r = range(-360, 360, 3)?;
                r.flags = [false, true, true];
                result.push(number(
                    "Rotation",
                    0.,
                    definition(
                        3,
                        decimal(1)?,
                        -1000000,
                        1000000,
                        Some(r),
                        Some(curve(0, 360, true)?),
                    )?,
                ));
                result.push(choice("Rotate Order", &["Beginning", "End"], 1));
                result.push(choice("Seamless", &["Yes", "No", "Repeating"], 0));
            }
        }
        PointwiseKind::RotateTile => result.push(number(
            "Amount",
            1.,
            definition(0, decimal(1)?, 0, 10000, Some(range(0, 10, 0)?), None)?,
        )),
        PointwiseKind::Kaleidoscope => {
            let current = PointwiseCustomObject::new_region_for_property();
            let previous = current.source_clone()?;
            result.push(ParameterState {
                extras: ParameterExtras::default(),
                choice_memory: None,
                name: text("Region"),
                value: PointwiseParameterValue::Region {
                    current: Some(current.raw()),
                    previous: Some(previous.raw()),
                },
            });
        }
        _ => return Err(PointwiseError::InvalidCast),
    }
    Ok(result)
}

/// ah8 extends yo/ok with zero input slots and six polygon properties.
pub(super) fn polygon_parameters<E>(
    mode: PointwiseCloneMode,
) -> Result<Vec<ParameterState<E>>, PointwiseError<E>> {
    let mut result = parameters(PointwiseKind::FreeTransform, mode)?;
    result.truncate(2); // inherited Size and Location, preserving their actual owners
    result.push(number(
        "Edges",
        3.0,
        definition(
            0,
            decimal(1)?,
            3,
            1000,
            Some(range(3, 30, 0)?),
            Some(curve(3, 50, false)?),
        )?,
    ));
    let mut angle_range = range(-360, 360, 3)?;
    angle_range.flags = [false, true, true];
    result.push(number(
        "Angle",
        0.0,
        definition(
            3,
            decimal(1)?,
            -1000000,
            1000000,
            Some(angle_range),
            Some(curve(0, 360, true)?),
        )?,
    ));
    let tenth = DecimalBits::new([1, 0, 0, 1 << 16]).map_err(|_| ImageError::Argument)?;
    let mut inner_range = range(-50, 50, 3)?;
    inner_range.flags = [false, true, true];
    inner_range.multiplier = 0.05;
    result.push(number(
        "Inner Radius",
        0.5,
        definition(
            3,
            tenth,
            -1000000,
            1000000,
            Some(inner_range),
            Some(curve(0, 1, false)?),
        )?,
    ));
    result.push(choice("Fit Mode", &["None", "Proportional", "Distort"], 0));
    result.push(choice("Repeat", &["No", "Yes"], 0));
    result.push(choice("Radius Changer", &["None", "Star", "Gear"], 0));
    Ok(result)
}
