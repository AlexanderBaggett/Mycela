use gpui_kit::{AssetSource, SharedString, assets::Assets};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(CommandIcons, [Save, SaveAll, Download, ZoomIn, ZoomOut]);

struct MycelaAssets;

impl AssetSource for MycelaAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        match CommandIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut assets = Assets.list(path)?;
        assets.extend(CommandIcons.list(path)?);
        Ok(assets)
    }
}

fn main() {
    gpui_kit::application().with_assets(MycelaAssets).run(|cx| {
        gpui_kit::init(cx);
        if let Err(error) = mycela_app::open_desktop_window(cx) {
            eprintln!("Could not open Mycela: {error}");
            cx.quit();
        }
    });
}
