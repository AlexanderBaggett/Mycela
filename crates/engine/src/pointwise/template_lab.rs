//! Fixed template Lab identities from Genetica's sa family.
use super::{PointwiseError, PointwiseFilter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateLabKind {
    CutTile,
    Fiber,
    Layer,
    Noise,
    SelectNoise,
    SelectPattern,
    Substance,
    Weather,
    Wood,
    SelectArea,
    SelectRings,
    Rings,
    Wrapper,
}

pub(super) struct LabTemplate {
    pub(super) name: &'static str,
    pub(super) inputs: &'static [&'static str],
    pub(super) contents: Option<&'static str>,
}

impl TemplateLabKind {
    const ALL: [Self; 13] = [
        Self::CutTile,
        Self::Fiber,
        Self::Layer,
        Self::Noise,
        Self::SelectNoise,
        Self::SelectPattern,
        Self::Substance,
        Self::Weather,
        Self::Wood,
        Self::SelectArea,
        Self::SelectRings,
        Self::Rings,
        Self::Wrapper,
    ];

    pub(super) fn from_identity(identity: &[u16]) -> Option<Self> {
        if identity.iter().copied().eq("Material Lab".encode_utf16()) {
            return Some(Self::Substance);
        }
        Self::ALL.into_iter().find(|kind| {
            identity
                .iter()
                .copied()
                .eq(kind.template().name.encode_utf16())
        })
    }

    pub(super) fn template(self) -> LabTemplate {
        let (name, inputs, contents): (_, &[_], _) = match self {
            Self::CutTile => (
                "Cut & Tile Lab",
                &["Material", "Pattern"],
                Some("Wgroups/Cut & Tile/Cut & Tile.wtx"),
            ),
            Self::Fiber => (
                "Fiber Lab",
                &["Background", "Noise"],
                Some("Wgroups/Fiber/Fiber.wtx"),
            ),
            Self::Layer => (
                "Layer Lab",
                &["First Material", "Second Material", "Noise"],
                Some("Wgroups/Layer/Layer.wtx"),
            ),
            Self::Noise => (
                "Noise Lab",
                &["Input Noise"],
                Some("Wgroups/Noise/Combine Noise.wtx"),
            ),
            Self::SelectNoise => (
                "Select Noise",
                &["Input Noise"],
                Some("Wgroups/Noise/Select Noise.wtx"),
            ),
            Self::SelectPattern => (
                "Select Pattern",
                &["Input Pattern"],
                Some("Wgroups/Cut & Tile/Select Pattern.wtx"),
            ),
            Self::Substance => (
                "Substance Lab",
                &["Noise"],
                Some("Wgroups/Material/Material.wtx"),
            ),
            Self::Weather => (
                "Weather Lab",
                &["Material", "Noise"],
                Some("Wgroups/Weathering/Multi-Weather.wtx"),
            ),
            Self::Wood => ("Wood Lab", &[], Some("Wgroups/Wood/Wood.wtx")),
            Self::SelectArea => (
                "Select Area",
                &["Input Pattern"],
                Some("Wgroups/Wood/Select Area.wtx"),
            ),
            Self::SelectRings => (
                "Select Rings",
                &["Input Pattern"],
                Some("Wgroups/Wood/Selection Types/Select Rings.wtx"),
            ),
            Self::Rings => ("Rings", &[], Some("Wgroups/Wood/Rings.wtx")),
            Self::Wrapper => ("Lab Wrapper Node", &[], None),
        };
        LabTemplate {
            name,
            inputs,
            contents,
        }
    }
}

pub(super) const SCALE: i32 = 3;
pub(super) const SEED: i32 = 4;

pub(super) fn scale_multiplier(value: f64) -> f64 {
    if value < 0. {
        -1. / (value - 1.)
    } else {
        value + 1.
    }
}

pub(super) fn scale_control_value(multiplier: f64) -> f64 {
    if multiplier < 1. {
        -1. / multiplier + 1.
    } else {
        multiplier - 1.
    }
}

pub(super) fn refresh_control<E: 'static>(
    lab: &PointwiseFilter<E>,
    index: i32,
) -> Result<(), PointwiseError<E>> {
    let Some(content) = lab.node().content(0)?.node() else {
        return Ok(());
    };
    let (scale, seed) = lab.lab_inheritance();
    match index {
        SCALE => content.set_graph_scale(scale * scale_multiplier(lab.parameter(SCALE)?.number())),
        SEED => content.set_graph_seed_modifier(super::parameter_ops::seed_modifier(
            lab.parameter(SEED)?.number(),
            seed,
            true,
        )),
        _ => Ok(()),
    }
}
