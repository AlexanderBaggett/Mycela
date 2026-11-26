#!/bin/sh
set -eu

project_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cd "$project_dir"

target_dir=${CARGO_TARGET_DIR:-target}
cargo build --locked --release --package mycela-app --bin mycela-app --target-dir "$target_dir"

# Stage the executable beside its destination so replacing it is atomic.
staged=$(mktemp "$project_dir/.mycela-app.XXXXXXXX")
trap 'rm -f -- "$staged"' EXIT
install -m 755 -- "$target_dir/release/mycela-app" "$staged"
mv -fT -- "$staged" "$project_dir/mycela-app"

printf 'Built %s\n' "$project_dir/mycela-app"
