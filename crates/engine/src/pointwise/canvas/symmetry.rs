//! Canvas combines the distance field with its reflected or rotated samples.
use crate::PointwiseError;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum CanvasSymmetry {
    #[default]
    None,
    Horizontal,
    Vertical,
    MainDiagonal,
    OppositeDiagonal,
    HalfTurn,
    Orthogonal,
    BothDiagonals,
    QuarterTurns,
    EightWay,
}

impl CanvasSymmetry {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Horizontal,
        Self::Vertical,
        Self::MainDiagonal,
        Self::OppositeDiagonal,
        Self::HalfTurn,
        Self::Orthogonal,
        Self::BothDiagonals,
        Self::QuarterTurns,
        Self::EightWay,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Horizontal => "Horizontal mirror",
            Self::Vertical => "Vertical mirror",
            Self::MainDiagonal => "Descending diagonal",
            Self::OppositeDiagonal => "Ascending diagonal",
            Self::HalfTurn => "180° rotation",
            Self::Orthogonal => "Horizontal and vertical",
            Self::BothDiagonals => "Both diagonals",
            Self::QuarterTurns => "90° rotations",
            Self::EightWay => "Eight-way",
        }
    }

    pub(super) fn from_index(index: i32) -> Option<Self> {
        Self::ALL.get(usize::try_from(index).ok()?).copied()
    }

    pub(super) fn apply<E>(
        self,
        field: &mut [f32],
        width: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        use Transform::*;
        let transforms: &[Transform] = match self {
            Self::None => &[],
            Self::Horizontal => &[FlipX],
            Self::Vertical => &[FlipY],
            Self::MainDiagonal => &[Transpose],
            Self::OppositeDiagonal => &[ReverseTranspose],
            Self::HalfTurn => &[RotateHalf],
            Self::Orthogonal => &[FlipX, FlipY],
            Self::BothDiagonals => &[Transpose, ReverseTranspose],
            Self::QuarterTurns => &[RotateQuarter, RotateHalf],
            Self::EightWay => &[FlipX, FlipY, RotateQuarter],
        };
        let last = width - 1;
        for transform in transforms {
            for y in 0..width {
                crate::pointwise::check_render_cancelled(cancelled)?;
                for x in 0..width {
                    let (u, v) = match transform {
                        FlipX => (last - x, y),
                        FlipY => (x, last - y),
                        Transpose => (y, x),
                        ReverseTranspose => (last - y, last - x),
                        RotateHalf => (last - x, last - y),
                        RotateQuarter => (y, last - x),
                    };
                    let value = field[v * width + u];
                    let pixel = &mut field[y * width + x];
                    if *pixel < value {
                        *pixel = value;
                    }
                }
            }
        }
        Ok(())
    }
}

enum Transform {
    FlipX,
    FlipY,
    Transpose,
    ReverseTranspose,
    RotateHalf,
    RotateQuarter,
}
