use std::{fs, path::Path};

fn main() {
    let vendor = Path::new("vendor/ijg-jpeg");
    let bridge = "src/ijg_jpeg.c";
    println!("cargo:rerun-if-changed={}", vendor.display());
    println!("cargo:rerun-if-changed={bridge}");
    let mut sources: Vec<_> = fs::read_dir(vendor)
        .expect("read IJG source directory")
        .map(|entry| entry.expect("read IJG source entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
        .collect();
    sources.sort();
    cc::Build::new()
        .include(vendor)
        .files(sources)
        .file(bridge)
        .flag_if_supported("-fvisibility=hidden")
        .warnings(false)
        .compile("mycela_ijg_jpeg");
}
