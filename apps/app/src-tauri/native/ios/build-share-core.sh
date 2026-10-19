#!/bin/bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../../../../.." && pwd)"
manifest="$repo_root/apps/app/src-tauri/share/core/Cargo.toml"
output="$PROJECT_DIR/share-core/$CONFIGURATION$EFFECTIVE_PLATFORM_NAME"
build_dir="$PROJECT_DIR/share-core/target"
mkdir -p "$output"

case "$PLATFORM_NAME" in
  iphoneos) targets=(aarch64-apple-ios) ;;
  iphonesimulator)
    targets=()
    for arch in $ARCHS; do
      case "$arch" in
        arm64) targets+=(aarch64-apple-ios-sim) ;;
        x86_64) targets+=(x86_64-apple-ios) ;;
        *) echo "Unsupported simulator architecture: $arch" >&2; exit 1 ;;
      esac
    done ;;
  *) echo "Unsupported iOS platform: $PLATFORM_NAME" >&2; exit 1 ;;
esac

archives=()
export PATH="$HOME/.cargo/bin:$PATH"
for target in "${targets[@]}"; do
  rustup target add "$target"
  cargo build --manifest-path "$manifest" --target "$target" --target-dir "$build_dir" --release --locked
  archives+=("$build_dir/$target/release/libcliproam_share_upload.a")
done
if [ "${#archives[@]}" -eq 1 ]; then
  cp "${archives[0]}" "$output/libcliproam_share_upload.a"
else
  lipo -create "${archives[@]}" -output "$output/libcliproam_share_upload.a"
fi
