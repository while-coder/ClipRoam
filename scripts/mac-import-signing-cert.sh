#!/usr/bin/env bash
# 共用桌面发布工作流的固定入口。
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec bash "$ROOT/apps/app/src-tauri/native/macos/sign.sh" --import-ci "$@"
