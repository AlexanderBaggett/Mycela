//! Presentation only. Theme colors sit behind the unchanged engine raster.
use gpui_kit::component::ActiveTheme;
use gpui_kit::{Bounds, IntoElement, Pixels, Styled, canvas, fill, point, size};

pub(crate) fn checkerboard() -> impl IntoElement {
    checkerboard_for(1.)
}

pub(crate) fn checkerboard_for(aspect: f32) -> impl IntoElement {
    canvas(
        |_, window, cx| (window.rem_size(), cx.theme().background, cx.theme().muted),
        move |bounds, (rem, light, dark), window, _| {
            // Resolved paint geometry matches ObjectFit::Contain's
            // raster. Tiles follow interface zoom; very large windows cap
            // paint work at 64 × 64 cells without retaining layout state.
            let width = bounds.size.width.min(bounds.size.height * aspect);
            let height = bounds.size.height.min(bounds.size.width / aspect);
            let tile = rem.max(width.max(height) / 64.);
            if tile <= Pixels::ZERO {
                return;
            }
            let origin = point(
                bounds.origin.x + (bounds.size.width - width) / 2.,
                bounds.origin.y + (bounds.size.height - height) / 2.,
            );
            for row in 0..64usize {
                let y = tile * row;
                if y >= height {
                    break;
                }
                for column in 0..64usize {
                    let x = tile * column;
                    if x >= width {
                        break;
                    }
                    window.paint_quad(fill(
                        Bounds::new(
                            origin + point(x, y),
                            size(tile.min(width - x), tile.min(height - y)),
                        ),
                        if (row + column) % 2 == 0 { light } else { dark },
                    ));
                }
            }
        },
    )
    .absolute()
    .size_full()
}
