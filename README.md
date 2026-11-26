# Mycela

Mycela is a work-in-progress Linux desktop editor for procedural textures, written in Rust with GPUI Kit.

Build a texture by connecting nodes for patterns, images, and filters. Adjust a node’s properties and see the result update in the live preview. Use the Shader node for custom WGSL, save editable graphs as `.mycela` projects, and render textures for export as still images, animation frames, or video.

The Rust workspace keeps the desktop app in `crates/app`, the graph and texture engine in `crates/engine`, and GPU rendering in `crates/renderer`.

Build and run from this directory on Linux with a recent Rust toolchain and a C compiler.

```sh
./build.sh
./mycela-app
```

The build keeps Cargo’s release executable in `target/release/` by default and puts a runnable copy beside this README.

Video export uses the `ffmpeg` executable from your system. Install FFmpeg and make sure it is on your PATH. Mycela lists the supported codecs available in that installation.
