//! The vector document embedded after the Canvas node's ordinary graph record.
mod adjust;
mod area;
mod blend;
mod distance;
mod environment;
mod environment_adjustment;
mod gradient;
mod height_map;
mod hierarchy;
mod named;
mod noise;
mod raster;
mod selection;
mod shape;
mod style;
mod symmetry;
mod synthesis;
#[cfg(test)]
mod visibility_tests;

mod grid;
pub(crate) use environment_adjustment::EnvironmentPaintEdit;
pub use grid::{CanvasGridKind, CanvasSnapGrid};
pub use hierarchy::{CanvasItem, CanvasItemPlacement};
pub(super) use raster::{render, render_style};
pub use selection::{
    CanvasAlignment, CanvasHandleDrag, CanvasHandleModifiers, CanvasHandlePreview,
    CanvasSelectionEdit, CanvasSelectionField, CanvasSelectionTransform, CanvasShapeEditor,
    CanvasShapeField, CanvasShapeValues, CanvasTransformFrame, CanvasTransformHandle,
    CanvasTransformOrientation,
};
pub use shape::{
    CanvasPath, CanvasPathEndpoint, CanvasPathHandle, CanvasPathPoint, CanvasPathSegment,
    CanvasPointKind,
};
pub(crate) use style::SynthesisPaint;
pub(crate) use synthesis::TextureEdit;

use super::{NodeWireError, PointwiseError, PointwiseFilter, node_wire::NodeWireReadContext};
use crate::{BinaryCursor, BinaryError, BinaryWriter};
use sha2::{Digest, Sha256};
use shape::CanvasShape;
use std::sync::{Arc, Weak};
pub(super) use style::CanvasStyle;
use style::ShapeArea;
pub use style::{
    StyleControl, StyleControlKind, StyleCurve, StyleEdit, StyleNoiseScope, StylePaintKind,
    StyleSection,
};
pub use symmetry::CanvasSymmetry;
pub use synthesis::{
    SynthesisCatalogValue, SynthesisColorField, SynthesisColorTarget, SynthesisCropHandle,
    SynthesisEdit, SynthesisField, SynthesisGeometry, SynthesisLayerKind, SynthesisNumberField,
    SynthesisPairField, SynthesisPatchResizeHandle, SynthesisPatchShape, SynthesisPatchTransform,
    SynthesisPatchTransformBounds, SynthesisPatchTransformField, SynthesisPlacementClipboard,
    SynthesisPlacementField, SynthesisPlacementGeometry, SynthesisPlacementTransform,
    SynthesisRecord,
};
pub(super) use synthesis::{SynthesisTexture, render_synthesis};

#[derive(Clone, Debug)]
pub(super) struct CanvasDocument {
    layers: Vec<CanvasLayer>,
    design_plan: CanvasDesignPlan,
    grid_density: [i32; 2],
    grid_kind: i32,
}

impl Default for CanvasDocument {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            design_plan: CanvasDesignPlan::default(),
            grid_density: [-1, -1],
            grid_kind: 0,
        }
    }
}

impl CanvasDocument {
    pub(super) fn preview_environment(
        &self,
        identity: usize,
        pass: usize,
        map: &Arc<crate::EnvironmentRecord>,
    ) -> Option<Arc<Self>> {
        let mut canvas = self.clone();
        let layer = canvas
            .layers
            .iter_mut()
            .find(|layer| Arc::as_ptr(&layer.identity) as usize == identity)?;
        layer.style = layer.style.preview_environment(pass, map)?;
        Some(Arc::new(canvas))
    }
}

/// The external file reference saved by Canvas. The guide is editor-only.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CanvasDesignPlan {
    path: Vec<u16>,
    hash: Vec<u16>,
}

impl CanvasDesignPlan {
    pub fn new(path: &str, image_bytes: &[u8]) -> Self {
        Self {
            path: path.encode_utf16().collect(),
            hash: format!("{:X}", Sha256::digest(image_bytes))
                .encode_utf16()
                .collect(),
        }
    }

    pub fn path(&self) -> Option<String> {
        (!self.path.is_empty()).then(|| String::from_utf16_lossy(&self.path))
    }

    pub fn matches_image(&self, image_bytes: &[u8]) -> bool {
        self.hash
            .iter()
            .copied()
            .eq(format!("{:X}", Sha256::digest(image_bytes)).encode_utf16())
    }
}

#[derive(Clone, Debug)]
struct CanvasLayer {
    identity: Arc<()>,
    header: ItemHeader,
    shapes: Vec<CanvasShape>,
    symmetry: CanvasSymmetry,
    scale: f32,
    input: i32,
    style: Arc<CanvasStyle>,
}

#[derive(Clone, Debug)]
struct ItemHeader {
    visibility: i32,
    name: Vec<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum CanvasPrimitive {
    Rectangle = 3,
    ClippedRectangle,
    RoundedRectangle,
    Ellipse,
    Pie,
    IsoscelesTriangle,
    RightTriangle,
    Parallelogram,
    Trapezoid,
    Polygon,
    Star,
    CurvedStar,
    Gear,
    Flower,
    Arrow,
    DoubleArrow,
    NotchedArrow,
    Chevron,
}

impl CanvasPrimitive {
    pub const ALL: [Self; 18] = [
        Self::Rectangle,
        Self::ClippedRectangle,
        Self::RoundedRectangle,
        Self::Ellipse,
        Self::Pie,
        Self::IsoscelesTriangle,
        Self::RightTriangle,
        Self::Parallelogram,
        Self::Trapezoid,
        Self::Polygon,
        Self::Star,
        Self::CurvedStar,
        Self::Gear,
        Self::Flower,
        Self::Arrow,
        Self::DoubleArrow,
        Self::NotchedArrow,
        Self::Chevron,
    ];

    fn from_tag(tag: i32) -> Option<Self> {
        Self::ALL.iter().copied().find(|shape| *shape as i32 == tag)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::ClippedRectangle => "Clipped rectangle",
            Self::RoundedRectangle => "Rounded rectangle",
            Self::Ellipse => "Ellipse",
            Self::Pie => "Pie",
            Self::IsoscelesTriangle => "Isosceles triangle",
            Self::RightTriangle => "Right triangle",
            Self::Parallelogram => "Parallelogram",
            Self::Trapezoid => "Trapezoid",
            Self::Polygon => "Polygon",
            Self::Star => "Star",
            Self::CurvedStar => "Curved star",
            Self::Gear => "Gear",
            Self::Flower => "Flower",
            Self::Arrow => "Arrow",
            Self::DoubleArrow => "Double arrow",
            Self::NotchedArrow => "Notched arrow",
            Self::Chevron => "Chevron",
        }
    }

    fn default_controls(self) -> &'static [(&'static str, f32)] {
        match self {
            Self::Rectangle | Self::Ellipse | Self::IsoscelesTriangle | Self::RightTriangle => &[],
            Self::ClippedRectangle => &[("Radius", 25.), ("Clip", 100.)],
            Self::RoundedRectangle => &[("Radius", 25.), ("Direction", 100.)],
            Self::Pie => &[("Start Angle", 90.), ("End Angle", 0.)],
            Self::Parallelogram => &[("Slant", 25.)],
            Self::Trapezoid | Self::Chevron => &[("Slant", 50.)],
            Self::Polygon => &[("Sides", 8.), ("Rounding", 0.)],
            Self::Star => &[("Points", 8.), ("Inner Radius", 50.), ("Twist", 0.)],
            Self::CurvedStar => &[
                ("Points", 8.),
                ("Direction", 100.),
                ("Strength", 50.),
                ("Twist", 0.),
            ],
            Self::Gear => &[
                ("Teeth", 20.),
                ("Inner Radius", 90.),
                ("Middle Radius", 50.),
                ("Bevel", 50.),
            ],
            Self::Flower => &[
                ("Petals", 8.),
                ("Inner Radius", 25.),
                ("Inner Tension", 0.),
                ("Outer Tension", 50.),
            ],
            Self::Arrow => &[("Head Width", 50.), ("Thickness", 50.)],
            Self::DoubleArrow | Self::NotchedArrow => &[("Head Width", 50.), ("Tickness", 50.)],
        }
    }

    /// The closed outline of a newly authored primitive in Canvas tile units.
    pub fn outline(self, bounds: CanvasShapeBounds) -> Result<Vec<[f64; 2]>, PointwiseError> {
        CanvasShape::primitive(self, bounds).primitive_outline()
    }
}

/// Canvas uses a 100 × 100 tile. Signed extents represent mirrored geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasShapeBounds {
    center: [f32; 2],
    size: [f32; 2],
}

impl CanvasShapeBounds {
    const CENTERED: Self = Self {
        center: [50.; 2],
        size: [60.; 2],
    };

    pub fn new(center: [f32; 2], size: [f32; 2]) -> Option<Self> {
        if center
            .into_iter()
            .chain(size)
            .any(|value| !value.is_finite())
        {
            return None;
        }
        Some(Self {
            center,
            size: size.map(|extent| if extent.abs() < 0.01 { 0.01 } else { extent }),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasAreaField {
    OffsetX,
    OffsetY,
    Thickness,
    HardFeather,
    SoftFeather,
    OutlineThickness,
}

impl CanvasAreaField {
    pub const ALL: [Self; 6] = [
        Self::OffsetX,
        Self::OffsetY,
        Self::Thickness,
        Self::HardFeather,
        Self::SoftFeather,
        Self::OutlineThickness,
    ];

    pub const fn bounds(self) -> [f64; 2] {
        match self {
            Self::HardFeather | Self::SoftFeather => [0., 100.],
            _ => [-100., 100.],
        }
    }
}

/// The scalar and toggle settings of an enabled Canvas shape mask.
#[derive(Clone, Copy, Debug)]
pub struct CanvasAreaSettings {
    numbers: [f64; 6],
    outline: bool,
}

impl CanvasAreaSettings {
    pub fn number(&self, field: CanvasAreaField) -> f64 {
        self.numbers[field as usize]
    }

    pub fn set_number(&mut self, field: CanvasAreaField, value: f64) -> bool {
        let [min, max] = field.bounds();
        if !(min..=max).contains(&value) {
            return false;
        }
        self.numbers[field as usize] = value;
        true
    }

    pub fn outline(&self) -> bool {
        self.outline
    }

    pub fn set_outline(&mut self, outline: bool) {
        self.outline = outline;
    }

    fn same_values(&self, other: &Self) -> bool {
        self.outline == other.outline
            && self.numbers.map(f64::to_bits) == other.numbers.map(f64::to_bits)
    }
}

impl From<&ShapeArea> for CanvasAreaSettings {
    fn from(area: &ShapeArea) -> Self {
        Self {
            numbers: [
                area.offset[0],
                area.offset[1],
                area.thickness,
                area.hard_feather,
                area.soft_feather,
                area.outline_thickness,
            ],
            outline: area.outline,
        }
    }
}

/// An editable top-level Canvas layer. The handle follows the layer through
/// edits and fails if a command targets a layer that has since been removed.
pub struct CanvasLayerEditor<E> {
    owner: PointwiseFilter<E>,
    identity: Arc<()>,
    label: String,
}

impl<E> Clone for CanvasLayerEditor<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            identity: self.identity.clone(),
            label: self.label.clone(),
        }
    }
}

impl<E: 'static> CanvasLayerEditor<E> {
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.identity) as usize
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.owner.same_reference(&other.owner) && Arc::ptr_eq(&self.identity, &other.identity)
    }

    pub fn name(&self) -> Option<String> {
        let (canvas, index) = self.current_layer().ok()?;
        Some(String::from_utf16_lossy(&canvas.layers[index].header.name))
    }

    pub fn is_visible(&self) -> Option<bool> {
        let (canvas, index) = self.current_layer().ok()?;
        Some(canvas.layers[index].header.container_visible())
    }

    pub fn scale(&self) -> Option<f32> {
        let (canvas, index) = self.current_layer().ok()?;
        Some(canvas.layers[index].scale)
    }

    pub fn input_index(&self) -> Option<usize> {
        let (canvas, index) = self.current_layer().ok()?;
        usize::try_from(canvas.layers[index].input).ok()
    }

    pub fn symmetry(&self) -> Option<CanvasSymmetry> {
        let (canvas, index) = self.current_layer().ok()?;
        Some(canvas.layers[index].symmetry)
    }

    pub fn edit_symmetry(&self, symmetry: CanvasSymmetry) -> Result<bool, PointwiseError<E>> {
        self.edit_settings("Change layer symmetry", |layer| {
            let changed = layer.symmetry != symmetry;
            layer.symmetry = symmetry;
            Ok(changed)
        })
    }

    pub fn edit_properties(&self, name: &str, scale: f32) -> Result<bool, PointwiseError<E>> {
        self.edit_settings("Edit Canvas layer properties", |layer| {
            let scaled = layer.scale.to_bits() != scale.to_bits();
            if scaled && !(-20. ..=20.).contains(&scale) {
                return Err(PointwiseError::Argument);
            }
            let renamed = String::from_utf16_lossy(&layer.header.name) != name;
            if renamed {
                layer.header.name = name.encode_utf16().collect();
            }
            layer.scale = scale;
            Ok(renamed || scaled)
        })
    }

    pub fn edit_visibility(&self, visible: bool) -> Result<bool, PointwiseError<E>> {
        self.edit_settings("Change layer visibility", |layer| {
            let changed = layer.header.container_visible() != visible;
            if changed {
                layer.header.set_visible(visible);
            }
            Ok(changed)
        })
    }

    pub fn edit_input(&self, index: usize) -> Result<bool, PointwiseError<E>> {
        if index >= self.owner.node().reference_inputs()?.len() {
            return Err(PointwiseError::Index);
        }
        let index = i32::try_from(index).map_err(|_| PointwiseError::Index)?;
        self.edit_settings("Change layer input texture", |layer| {
            let changed = layer.input != index;
            layer.input = index;
            Ok(changed)
        })
    }

    pub fn edit_move(&self, offset: isize) -> Result<bool, PointwiseError<E>> {
        let (before, index) = self.current_layer()?;
        let Some(target) = index
            .checked_add_signed(offset)
            .filter(|&target| target < before.layers.len() && target != index)
        else {
            return Ok(false);
        };
        let mut after = (*before).clone();
        let layer = after.layers.remove(index);
        after.layers.insert(target, layer);
        self.owner
            .commit_canvas_edit(before, after, "Move Canvas layer")?;
        Ok(true)
    }

    pub fn edit_remove(&self) -> Result<(), PointwiseError<E>> {
        let (before, index) = self.current_layer()?;
        let mut after = (*before).clone();
        after.layers.remove(index);
        self.owner
            .commit_canvas_edit(before, after, "Delete Canvas layer")
    }

    pub fn edit_duplicate(&self) -> Result<Self, PointwiseError<E>> {
        let (before, index) = self.current_layer()?;
        let mut after = (*before).clone();
        let mut duplicate = before.layers[index].clone();
        duplicate.assign_new_identity();
        let editor = self.owner.canvas_layer_editor(index, &duplicate);
        after.layers.insert(index, duplicate);
        self.owner
            .commit_canvas_edit(before, after, "Duplicate Canvas layer")?;
        Ok(editor)
    }

    fn current_layer(&self) -> Result<(Arc<CanvasDocument>, usize), PointwiseError<E>> {
        let canvas = self
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        let index = self
            .index_in(&canvas)
            .ok_or(PointwiseError::InvalidOperation)?;
        Ok((canvas, index))
    }

    fn edit_settings(
        &self,
        label: &str,
        apply: impl FnOnce(&mut CanvasLayer) -> Result<bool, PointwiseError<E>>,
    ) -> Result<bool, PointwiseError<E>> {
        let (before, index) = self.current_layer()?;
        let mut after = (*before).clone();
        if !apply(&mut after.layers[index])? {
            return Ok(false);
        }
        self.owner.commit_canvas_edit(before, after, label)?;
        Ok(true)
    }

    pub fn style_edit(&self) -> Option<StyleEdit> {
        let canvas = self.owner.canvas_document()?;
        let layer = &canvas.layers[self.index_in(&canvas)?];
        Some(StyleEdit::new(layer.style.clone()))
    }

    pub fn style_input_node(&self) -> Option<crate::PointwiseNode<E>> {
        let canvas = self.owner.canvas_document()?;
        let layer = &canvas.layers[self.index_in(&canvas)?];
        self.owner
            .node()
            .reference_inputs()
            .ok()?
            .get(layer.input as usize)?
            .node()
    }

    pub fn edit_style(&self, edit: StyleEdit, label: &str) -> Result<bool, PointwiseError<E>> {
        let Some((expected, style)) = edit.into_replacement() else {
            return Ok(false);
        };
        let before = self
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        let index = self
            .index_in(&before)
            .ok_or(PointwiseError::InvalidOperation)?;
        if !Arc::ptr_eq(&before.layers[index].style, &expected) {
            return Err(PointwiseError::InvalidOperation);
        }
        let mut after = (*before).clone();
        after.layers[index].style = style;
        self.owner.commit_canvas_edit(before, after, label)?;
        Ok(true)
    }

    pub fn edit_insert_shape(
        &self,
        primitive: CanvasPrimitive,
        bounds: CanvasShapeBounds,
    ) -> Result<(), PointwiseError<E>> {
        self.insert_item(CanvasShape::primitive(primitive, bounds), primitive.name())
    }

    pub fn edit_insert_path(&self, path: CanvasPath) -> Result<(), PointwiseError<E>> {
        self.insert_item(CanvasShape::from_canvas_path(path)?, "Curve")
    }

    fn insert_item(&self, item: CanvasShape, label: &str) -> Result<(), PointwiseError<E>> {
        let before = self
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        let index = self
            .index_in(&before)
            .ok_or(PointwiseError::InvalidOperation)?;
        let mut after = (*before).clone();
        after.layers[index].shapes.insert(0, item);
        self.owner
            .commit_canvas_edit(before, after, &format!("Add {label}"))
    }

    fn index_in(&self, canvas: &CanvasDocument) -> Option<usize> {
        canvas
            .layers
            .iter()
            .position(|layer| Arc::ptr_eq(&self.identity, &layer.identity))
    }
}

/// An Area pass in a retained Canvas layer. The handle follows the layer across history edits.
pub struct CanvasAreaEditor<E> {
    layer: CanvasLayerEditor<E>,
    pass: usize,
    label: String,
}

impl<E: 'static> Clone for CanvasAreaEditor<E> {
    fn clone(&self) -> Self {
        Self {
            layer: self.layer.clone(),
            pass: self.pass,
            label: self.label.clone(),
        }
    }
}

impl<E: 'static> CanvasAreaEditor<E> {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.layer.same_reference(&other.layer) && self.pass == other.pass
    }

    pub fn layer(&self) -> CanvasLayerEditor<E> {
        self.layer.clone()
    }

    pub fn settings(&self) -> Result<CanvasAreaSettings, PointwiseError<E>> {
        let document = self
            .layer
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        let area = self
            .layer
            .index_in(&document)
            .and_then(|index| {
                document.layers[index]
                    .style
                    .passes
                    .get(self.pass)?
                    .area
                    .as_ref()
            })
            .ok_or(PointwiseError::InvalidOperation)?;
        Ok(CanvasAreaSettings::from(area))
    }

    /// Apply one history edit if the settings still match the snapshot loaded by the editor.
    pub fn edit(
        &self,
        expected: CanvasAreaSettings,
        proposed: CanvasAreaSettings,
    ) -> Result<(), PointwiseError<E>> {
        let before = self
            .layer
            .owner
            .canvas_document()
            .ok_or(PointwiseError::InvalidCast)?;
        let index = self
            .layer
            .index_in(&before)
            .ok_or(PointwiseError::InvalidOperation)?;
        let mut after = (*before).clone();
        let area = Arc::make_mut(&mut after.layers[index].style)
            .passes
            .get_mut(self.pass)
            .and_then(|pass| pass.area.as_mut())
            .ok_or(PointwiseError::InvalidOperation)?;
        if !CanvasAreaSettings::from(&*area).same_values(&expected) {
            return Err(PointwiseError::InvalidOperation);
        }
        if expected.same_values(&proposed) {
            return Ok(());
        }
        for field in CanvasAreaField::ALL {
            let value = proposed.number(field);
            if value.to_bits() != expected.number(field).to_bits() {
                let [min, max] = field.bounds();
                if !(min..=max).contains(&value) {
                    return Err(PointwiseError::InvalidOperation);
                }
            }
        }
        let [
            x,
            y,
            thickness,
            hard_feather,
            soft_feather,
            outline_thickness,
        ] = proposed.numbers;
        area.offset = [x, y];
        area.thickness = thickness;
        area.hard_feather = hard_feather;
        area.soft_feather = soft_feather;
        area.outline_thickness = outline_thickness;
        area.outline = proposed.outline;
        self.layer
            .owner
            .commit_canvas_edit(before, after, "Edit Canvas Area")
    }
}

impl ItemHeader {
    fn new(name: &str) -> Self {
        Self {
            visibility: 1,
            name: name.encode_utf16().collect(),
        }
    }
}

impl<E: 'static> PointwiseFilter<E> {
    pub fn canvas_design_plan(&self) -> Option<CanvasDesignPlan> {
        let canvas = self.canvas_document()?;
        (!canvas.design_plan.path.is_empty()).then(|| canvas.design_plan.clone())
    }

    pub fn edit_canvas_design_plan(
        &self,
        plan: Option<CanvasDesignPlan>,
    ) -> Result<bool, PointwiseError<E>> {
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        let label = if plan.is_some() {
            "Set Canvas design plan"
        } else {
            "Clear Canvas design plan"
        };
        let plan = plan.unwrap_or_default();
        if before.design_plan == plan {
            return Ok(false);
        }
        let mut after = (*before).clone();
        after.design_plan = plan;
        self.commit_canvas_edit(before, after, label)?;
        Ok(true)
    }

    pub fn canvas_layer_editors(&self) -> Vec<CanvasLayerEditor<E>> {
        let Some(canvas) = self.canvas_document() else {
            return Vec::new();
        };
        canvas
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| self.canvas_layer_editor(index, layer))
            .collect()
    }

    pub fn edit_add_canvas_layer(&self) -> Result<CanvasLayerEditor<E>, PointwiseError<E>> {
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        let mut after = (*before).clone();
        let layer = CanvasLayer::new("", None);
        let editor = self.canvas_layer_editor(0, &layer);
        after.layers.insert(0, layer);
        self.commit_canvas_edit(before, after, "Add Canvas layer")?;
        Ok(editor)
    }

    fn canvas_layer_editor(&self, index: usize, layer: &CanvasLayer) -> CanvasLayerEditor<E> {
        let name = String::from_utf16_lossy(&layer.header.name);
        let label = if name.is_empty() {
            format!("Layer {}", index + 1)
        } else {
            format!("Layer {} · {name}", index + 1)
        };
        CanvasLayerEditor {
            owner: self.clone(),
            identity: layer.identity.clone(),
            label,
        }
    }

    pub fn canvas_area_editors(&self) -> Vec<CanvasAreaEditor<E>> {
        let Some(canvas) = self.canvas_document() else {
            return Vec::new();
        };
        let mut editors = Vec::new();
        for (index, layer) in canvas.layers.iter().enumerate() {
            let editor = self.canvas_layer_editor(index, layer);
            for (pass, style) in layer.style.passes.iter().enumerate() {
                if style.area.is_some() {
                    editors.push(CanvasAreaEditor {
                        layer: editor.clone(),
                        pass,
                        label: if layer.style.passes.len() == 1 {
                            editor.label().to_owned()
                        } else {
                            format!("{} · Pass {}", editor.label(), pass + 1)
                        },
                    });
                }
            }
        }
        editors
    }

    /// Add a centered shape using a solid color or an active image input.
    /// Input indices are zero-based, in the node's visible input order.
    pub fn edit_add_canvas_shape(
        &self,
        primitive: CanvasPrimitive,
        input: Option<usize>,
    ) -> Result<(), super::PointwiseError<E>> {
        self.edit_insert_canvas_shape(primitive, CanvasShapeBounds::CENTERED, input)
    }

    /// Insert one primitive at the supplied bounds as an undoable Canvas edit.
    pub fn edit_insert_canvas_shape(
        &self,
        primitive: CanvasPrimitive,
        bounds: CanvasShapeBounds,
        input: Option<usize>,
    ) -> Result<(), PointwiseError<E>> {
        self.insert_canvas_item(
            CanvasShape::primitive(primitive, bounds),
            primitive.name(),
            input,
        )
    }

    pub fn edit_insert_canvas_path(
        &self,
        path: CanvasPath,
        input: Option<usize>,
    ) -> Result<(), PointwiseError<E>> {
        self.insert_canvas_item(CanvasShape::from_canvas_path(path)?, "Curve", input)
    }

    fn insert_canvas_item(
        &self,
        item: CanvasShape,
        label: &str,
        input: Option<usize>,
    ) -> Result<(), PointwiseError<E>> {
        let before = self
            .canvas_document()
            .ok_or(super::PointwiseError::InvalidCast)?;
        if let Some(index) = input
            && index >= self.node().reference_inputs()?.len()
        {
            return Err(super::PointwiseError::Index);
        }
        let mut after = (*before).clone();
        let mut layer = CanvasLayer::new(label, input);
        layer.shapes.push(item);
        after.layers.insert(0, layer);
        self.commit_canvas_edit(before, after, &format!("Add {label}"))
    }

    fn commit_canvas_edit(
        &self,
        before: Arc<CanvasDocument>,
        after: CanvasDocument,
        label: &str,
    ) -> Result<(), PointwiseError<E>> {
        let after = Arc::new(after);
        let node = self.node();
        let slot = node.texture_node_logic().and_then(|logic| logic.slot());
        let history = slot.as_ref().and_then(|slot| slot.history());
        if !self.exchange_canvas_document(&before, after.clone()) {
            return Err(super::PointwiseError::InvalidOperation);
        }
        let record = if let Some((slot, history)) = slot.zip(history) {
            let sequence = history.edit_sequence();
            let result = history.record_graph_edit(
                slot.history_target(),
                label.encode_utf16().collect(),
                Box::new(CanvasEdit {
                    owner: Arc::downgrade(&node.core()),
                    before: before.clone(),
                    after: after.clone(),
                }),
            );
            if let Err(error) = result {
                if history.edit_sequence() == sequence
                    && self.exchange_canvas_document(&after, before)
                {
                    return Err(super::PointwiseError::History(Box::new(error)));
                }
                Err(super::PointwiseError::History(Box::new(error)))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        record.and(node.notify_property_change_with(crate::ContextChangeArgument::shared_empty()))
    }
}

struct CanvasEdit<E> {
    owner: Weak<super::graph::NodeCore<E>>,
    before: Arc<CanvasDocument>,
    after: Arc<CanvasDocument>,
}

impl<E: 'static> crate::history::HistoryGraphEdit<E> for CanvasEdit<E> {
    fn apply(&self, direction: crate::HistoryDirection) -> Result<(), crate::HistoryError<E>> {
        let owner = self
            .owner
            .upgrade()
            .ok_or(crate::HistoryError::BranchChanged)?;
        let sponsor =
            super::region::Sponsor::for_node(&owner).ok_or(crate::HistoryError::BranchChanged)?;
        let node = super::PointwiseNode {
            id: owner.id,
            sponsor,
        };
        let filter = node.filter().ok_or(crate::HistoryError::BranchChanged)?;
        let (expected, restored) = match direction {
            crate::HistoryDirection::Undo => (&self.after, &self.before),
            crate::HistoryDirection::Redo => (&self.before, &self.after),
        };
        if !filter.exchange_canvas_document(expected, restored.clone()) {
            return Err(crate::HistoryError::BranchChanged);
        }
        node.notify_property_change_with(crate::ContextChangeArgument::shared_empty())
            .map_err(|error| crate::HistoryError::Property(Box::new(error)))
    }
}

impl ItemHeader {
    const HIDDEN: i32 = 2;

    fn read<E>(
        reader: &mut BinaryCursor<'_>,
        context: Option<&NodeWireReadContext<'_, E>>,
    ) -> Result<Self, BinaryError> {
        let mut visibility = reader.read_i32()?;
        if visibility == 0
            && context.is_some_and(NodeWireReadContext::uses_legacy_canvas_visibility)
        {
            visibility = Self::HIDDEN;
        }
        let name = reader.read_string()?;
        Ok(Self { visibility, name })
    }

    fn write(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        writer.write_i32(self.visibility)?;
        writer.write_utf16(&self.name)
    }

    fn visible(&self) -> bool {
        self.visibility == 1
    }

    fn container_visible(&self) -> bool {
        self.visibility == 0 || self.visible()
    }

    fn set_visible(&mut self, visible: bool) {
        self.visibility = if visible { 1 } else { Self::HIDDEN };
    }
}

impl CanvasDocument {
    pub(super) fn uses_environment_library(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| layer.style.uses_environment_library())
    }

    pub(super) fn read<E: 'static>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
        mut context: Option<&mut NodeWireReadContext<'_, E>>,
    ) -> Result<Self, NodeWireError<E>> {
        let layers = read_list(reader, 21, |reader| {
            CanvasLayer::read(reader, version, 0, context.as_deref_mut())
        })?;
        let mut document = Self {
            layers,
            ..Self::default()
        };
        if version > 3.18 {
            document.design_plan.path = reader.read_string()?;
            document.design_plan.hash = reader.read_string()?;
        }
        if version > 3.19 {
            document.grid_density = [reader.read_i32()?, reader.read_i32()?];
            document.grid_kind = reader.read_i32()?;
        }
        Ok(document)
    }

    pub(super) fn write<E>(
        &self,
        writer: &mut BinaryWriter,
        gtx: bool,
    ) -> Result<(), NodeWireError<E>> {
        write_list(writer, &self.layers, |layer, writer| {
            layer.write(writer, gtx)
        })?;
        writer.write_utf16(&self.design_plan.path)?;
        writer.write_utf16(&self.design_plan.hash)?;
        writer.write_i32(self.grid_density[0])?;
        writer.write_i32(self.grid_density[1])?;
        writer.write_i32(self.grid_kind)?;
        Ok(())
    }

    pub(super) fn needs_input(&self, index: usize) -> bool {
        self.layers
            .iter()
            .any(|layer| layer.input as usize == index && layer.style.needs_input())
    }
}

impl<E: 'static> super::PointwiseNode<E> {
    pub(in crate::pointwise) fn uses_image_slot(&self, index: usize) -> bool {
        if self.kind() == Some(super::PointwiseKind::Synthesis) {
            return self
                .filter()
                .and_then(|filter| filter.synthesis_texture())
                .is_some_and(|texture| texture.uses_input());
        }
        if self.kind() != Some(super::PointwiseKind::Canvas) {
            return true;
        }
        self.filter().is_some_and(|filter| {
            filter
                .parameter(0)
                .is_ok_and(|count| index < count.number() as usize)
                && filter
                    .canvas_document()
                    .is_some_and(|canvas| canvas.needs_input(index))
        })
    }
}

impl CanvasLayer {
    fn new(name: &str, input: Option<usize>) -> Self {
        Self {
            identity: Arc::new(()),
            header: ItemHeader::new(name),
            shapes: Vec::new(),
            symmetry: CanvasSymmetry::None,
            scale: 0.,
            input: input.unwrap_or(0) as i32,
            style: Arc::new(input.map_or_else(CanvasStyle::default, |_| CanvasStyle::from_input())),
        }
    }

    fn assign_new_identity(&mut self) {
        self.identity = Arc::new(());
        for shape in &mut self.shapes {
            shape.assign_new_identity();
        }
    }

    fn read<E: 'static>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
        depth: usize,
        mut context: Option<&mut NodeWireReadContext<'_, E>>,
    ) -> Result<Self, NodeWireError<E>> {
        let header = ItemHeader::read(reader, context.as_deref())?;
        let shapes = shape::read_items(reader, version, depth, context.as_deref_mut())?;
        let symmetry = CanvasSymmetry::from_index(reader.read_i32()?).ok_or(BinaryError::Index)?;
        Ok(Self {
            identity: Arc::new(()),
            header,
            shapes,
            symmetry,
            scale: reader.read_f32()?,
            input: reader.read_i32()?,
            style: Arc::new(CanvasStyle::read(reader, version, context)?),
        })
    }

    fn write<E>(&self, writer: &mut BinaryWriter, gtx: bool) -> Result<(), NodeWireError<E>> {
        self.header.write(writer)?;
        write_list(writer, &self.shapes, |shape, writer| {
            shape.write(writer, gtx)
        })?;
        writer.write_i32(self.symmetry as i32)?;
        writer.write_f32(self.scale)?;
        writer.write_i32(self.input)?;
        self.style.write(writer)
    }
}

fn read_list<T, E>(
    reader: &mut BinaryCursor<'_>,
    minimum_size: i32,
    mut read: impl FnMut(&mut BinaryCursor<'_>) -> Result<T, NodeWireError<E>>,
) -> Result<Vec<T>, NodeWireError<E>> {
    let count = reader.read_i32()?;
    if count < 0 {
        return Err(BinaryError::NegativeLength.into());
    }
    if count > (reader.length()? - reader.position()?) / minimum_size {
        return Err(BinaryError::End.into());
    }
    let mut values = Vec::new();
    // Parse before reserving so a bad item does not allocate the advertised count.
    for _ in 0..count {
        let value = read(reader)?;
        values.try_reserve(1).map_err(|_| BinaryError::Allocation)?;
        values.push(value);
    }
    Ok(values)
}

fn write_list<T, E>(
    writer: &mut BinaryWriter,
    values: &[T],
    mut write: impl FnMut(&T, &mut BinaryWriter) -> Result<(), NodeWireError<E>>,
) -> Result<(), NodeWireError<E>> {
    writer.write_i32(i32::try_from(values.len()).map_err(|_| BinaryError::Count)?)?;
    for value in values {
        write(value, writer)?;
    }
    Ok(())
}

fn read_floats<const N: usize>(reader: &mut BinaryCursor<'_>) -> Result<[f32; N], BinaryError> {
    let mut values = [0.; N];
    for value in &mut values {
        *value = reader.read_f32()?;
    }
    Ok(values)
}

fn write_floats(writer: &mut BinaryWriter, values: &[f32]) -> Result<(), BinaryError> {
    for &value in values {
        writer.write_f32(value)?;
    }
    Ok(())
}
