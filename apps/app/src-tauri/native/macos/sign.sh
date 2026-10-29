#!/usr/bin/env bash
# 创建/使用自签名证书 cliproam-dev 给 macOS 构建产物签名，让「辅助功能」权限跨重编译保持授权。
#
# 背景：macOS 的 TCC 权限绑定应用的代码签名 Designated Requirement。
# 未签名 / ad-hoc 的 app 每次重编译 cdhash 都变，会被当成新 app 反复弹窗。
# 用固定证书签名后，DR 变为 identifier + 证书哈希，重编译不变，只需授权一次。
#
# 用法：
#   apps/app/src-tauri/native/macos/sign.sh --setup    # 首次创建并安装长期自签名证书（无需 Apple 账号）
#   apps/app/src-tauri/native/macos/sign.sh --check    # 检查当前钥匙串是否有可用签名身份
#   apps/app/src-tauri/native/macos/sign.sh --dev      # 构建并启动固定签名开发包
#   apps/app/src-tauri/native/macos/sign.sh --import-ci # CI 导入签名证书
#   apps/app/src-tauri/native/macos/sign.sh --build   # 使用固定证书构建安装包
#   apps/app/src-tauri/native/macos/sign.sh <路径>      # 签名指定的 .app 或可执行文件
#
# `pnpm --filter @cliproam/app macos:build` 会让 Tauri 使用这张证书签名打包产物；
# 本脚本的直接签名模式用于已有二进制以及排查签名问题。
set -euo pipefail
umask 077

IDENTITY="${APPLE_SIGNING_IDENTITY:-cliproam-dev}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/apps/app/src-tauri/target}"
if [[ "$TARGET_DIR" != /* ]]; then TARGET_DIR="$ROOT/apps/app/$TARGET_DIR"; fi
WORK_DIR=""
cleanup() {
  if [ -n "$WORK_DIR" ]; then rm -rf "$WORK_DIR"; fi
}
trap cleanup EXIT

workspace() {
  WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cliproam-signing.XXXXXX")"
}

[ "$(uname -s)" = Darwin ] || { echo "仅支持 macOS 签名" >&2; exit 1; }
[ -n "$IDENTITY" ] && [ "$IDENTITY" != "-" ] || { echo "需要固定证书身份，不能使用 ad-hoc 签名" >&2; exit 1; }
SIGNING_DIR="${CLIPROAM_SIGNING_DIR:-$HOME/.config/cliproam/signing}"
P12_PATH="$SIGNING_DIR/$IDENTITY.p12"

has_identity() {
  security find-identity -v -p codesigning 2>/dev/null | grep -Fq "\"$IDENTITY\""
}

check_identity() {
  if ! has_identity; then
    echo "未找到可用的 macOS 签名身份: $IDENTITY" >&2
    echo "请先运行: apps/app/src-tauri/native/macos/sign.sh --setup" >&2
    return 1
  fi
  security find-identity -v -p codesigning | grep -F "\"$IDENTITY\""
}

setup_identity() {
  [ "$IDENTITY" = cliproam-dev ] || { echo "--setup 仅创建 cliproam-dev；其他证书请自行导入钥匙串" >&2; return 1; }
  if has_identity; then
    echo "签名身份已存在: $IDENTITY"
    check_identity
    return 0
  fi

  command -v openssl >/dev/null || { echo "缺少 openssl" >&2; return 1; }
  command -v security >/dev/null || { echo "缺少 macOS security 工具" >&2; return 1; }

  workspace
  mkdir -p "$SIGNING_DIR"
  chmod 700 "$SIGNING_DIR"
  local key_path="$WORK_DIR/identity.key"
  local cert_path="$WORK_DIR/identity.crt"

  # 已有证书失去信任时只修复信任，不创建同名的新证书。
  if security find-certificate -c "$IDENTITY" -p > "$cert_path" 2>/dev/null; then
    security add-trusted-cert -r trustRoot -p codeSign \
      -k "$HOME/Library/Keychains/login.keychain-db" "$cert_path"
    if has_identity; then
      check_identity
      return 0
    fi
    if [ ! -e "$P12_PATH" ]; then
      echo "已存在同名证书但无可用私钥，请恢复原始 P12，不能重新创建证书: $P12_PATH" >&2
      return 1
    fi
  fi

  if [ -z "${CLIPROAM_CERTIFICATE_PASSWORD:-}" ]; then
    read -r -s -p "设置 $IDENTITY.p12 的导出密码（用于 GitHub Secret）: " CLIPROAM_CERTIFICATE_PASSWORD
    echo
  fi
  if [ -z "$CLIPROAM_CERTIFICATE_PASSWORD" ]; then
    echo "证书导出密码不能为空" >&2
    return 1
  fi
  export CLIPROAM_CERTIFICATE_PASSWORD

  # 恢复原有 P12，避免重建证书导致所有旧授权失效。
  if [ ! -e "$P12_PATH" ]; then
    openssl req -x509 -newkey rsa:3072 -sha256 -days 3650 -nodes \
      -config /dev/stdin \
      -keyout "$key_path" \
      -out "$cert_path" <<'OPENSSL_CONFIG'
[req]
prompt = no
distinguished_name = subject
x509_extensions = codesign

[subject]
CN = cliproam-dev
O = ClipRoam local development

[codesign]
basicConstraints = critical, CA:true
keyUsage = critical, digitalSignature, keyCertSign
extendedKeyUsage = codeSigning
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always
OPENSSL_CONFIG
    openssl pkcs12 -export \
      -name "$IDENTITY" \
      -inkey "$key_path" \
      -in "$cert_path" \
      -out "$WORK_DIR/identity.p12" \
      -passout env:CLIPROAM_CERTIFICATE_PASSWORD
    mv "$WORK_DIR/identity.p12" "$P12_PATH"
    chmod 600 "$P12_PATH"
  fi

  security import "$P12_PATH" \
    -k "$HOME/Library/Keychains/login.keychain-db" \
    -P "$CLIPROAM_CERTIFICATE_PASSWORD" \
    -T /usr/bin/codesign
  security find-certificate -c "$IDENTITY" -p > "$cert_path"
  security add-trusted-cert -r trustRoot -p codeSign \
    -k "$HOME/Library/Keychains/login.keychain-db" "$cert_path"
  unset CLIPROAM_CERTIFICATE_PASSWORD
  check_identity

  echo
  echo "自签名证书已保存到: $P12_PATH"
  echo "请把它的 Base64 内容保存为 GitHub Secret APPLE_CERTIFICATE，"
  echo "并把刚才的导出密码保存为 APPLE_CERTIFICATE_PASSWORD。"
  echo "macOS 可运行: base64 -i '$P12_PATH' | pbcopy"
}

sign() {
  [ -e "$1" ] || { echo "签名目标不存在: $1" >&2; return 1; }
  check_identity >/dev/null
  codesign --force --deep -s "$IDENTITY" "$1"
  verify "$1"
  echo "已签名: $1"
}

verify() {
  [ -e "$1" ] || { echo "签名目标不存在: $1" >&2; return 1; }
  codesign --verify --deep --strict --verbose=2 "$1"
  local requirement
  requirement="$(codesign -d -r- "$1" 2>&1)"
  if ! printf '%s\n' "$requirement" | grep -q 'designated =>'; then
    echo "无法读取签名身份: $1" >&2
    return 1
  fi
  if printf '%s\n' "$requirement" | grep -q 'cdhash'; then
    echo "签名仍绑定构建哈希，无法跨构建保持授权: $1" >&2
    return 1
  fi
  printf '%s\n' "$requirement"
}

signed_build() {
  check_identity >/dev/null
  (
    cd "$ROOT/apps/app"
    APPLE_SIGNING_IDENTITY="$IDENTITY" pnpm exec tauri build "$@"
  )
}

signed_dev() {
  check_identity >/dev/null
  (
    cd "$ROOT/apps/app"
    APPLE_SIGNING_IDENTITY="$IDENTITY" pnpm exec tauri build --debug --bundles app
    verify "$TARGET_DIR/debug/bundle/macos/ClipRoam.app"
    open "$TARGET_DIR/debug/bundle/macos/ClipRoam.app"
  )
}

import_ci() {
  # Bash 3.2 的函数参数展开错误配合 EXIT trap 可能错误地返回成功。
  # 显式返回失败，确保缺少证书时 CI 不能继续发布。
  [ -n "${APPLE_CERTIFICATE:-}" ] || { echo "缺少 GitHub Secret APPLE_CERTIFICATE" >&2; return 1; }
  [ -n "${APPLE_CERTIFICATE_PASSWORD:-}" ] || { echo "缺少 GitHub Secret APPLE_CERTIFICATE_PASSWORD" >&2; return 1; }
  [ -n "${RUNNER_TEMP:-}" ] || { echo "--import-ci 仅在 CI 中运行" >&2; return 1; }
  [ -n "${GITHUB_ENV:-}" ] || { echo "缺少 GITHUB_ENV" >&2; return 1; }
  workspace
  local ci_keychain_password
  ci_keychain_password="$(openssl rand -hex 32)"
  local ci_keychain_path="$RUNNER_TEMP/cliproam-signing.keychain-db"
  local ci_p12_path="$WORK_DIR/identity.p12"
  local ci_cert_path="$WORK_DIR/identity.crt"

  printf '%s' "$APPLE_CERTIFICATE" | base64 -D > "$ci_p12_path"

  echo "正在创建临时签名钥匙串..."
  security create-keychain -p "$ci_keychain_password" "$ci_keychain_path"
  security set-keychain-settings -lut 21600 "$ci_keychain_path"
  security unlock-keychain -p "$ci_keychain_password" "$ci_keychain_path"

  echo "正在导入签名身份..."
  security import "$ci_p12_path" -k "$ci_keychain_path" \
    -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
  # 从已导入的钥匙串读取证书，避免 OpenSSL 3 拒绝旧 P12 中的 RC2 加密。
  security find-certificate -c "$IDENTITY" -p "$ci_keychain_path" > "$ci_cert_path"

  if [ "$IDENTITY" = cliproam-dev ]; then
    echo "正在信任自签名证书..."
    sudo -n security add-trusted-cert -d -r trustRoot -p codeSign \
      -k /Library/Keychains/System.keychain "$ci_cert_path"
  fi

  echo "正在授权 codesign 访问私钥..."
  security set-key-partition-list -S apple-tool:,apple:,codesign: \
    -s -k "$ci_keychain_password" "$ci_keychain_path"

  local keychain
  local signing_keychains=("$ci_keychain_path")
  while IFS= read -r keychain; do
    keychain="${keychain#*\"}"
    keychain="${keychain%\"*}"
    [ -z "$keychain" ] || signing_keychains+=("$keychain")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "${signing_keychains[@]}"

  if ! security find-identity -v -p codesigning "$ci_keychain_path" | grep -Fq "\"$IDENTITY\""; then
    echo "导入后仍未找到有效签名身份: $IDENTITY" >&2
    security find-identity -v -p codesigning "$ci_keychain_path" >&2
    return 1
  fi

  # 真实签名验证能同时发现缺失私钥、证书不受信任和钥匙串访问失败。
  cp /usr/bin/true "$WORK_DIR/signing-probe"
  codesign --force --sign "$IDENTITY" --keychain "$ci_keychain_path" "$WORK_DIR/signing-probe"
  verify "$WORK_DIR/signing-probe"
  echo "APPLE_SIGNING_IDENTITY=$IDENTITY" >> "$GITHUB_ENV"
  echo "已导入固定 macOS 签名身份: $IDENTITY"
}

case "${1:---help}" in
  --setup) setup_identity ;;
  --check) check_identity ;;
  --dev) signed_dev ;;
  --build) shift; signed_build "$@" ;;
  --import-ci) import_ci ;;
  --verify) [ "$#" = 2 ] || { echo "用法: sign.sh --verify <路径>" >&2; exit 1; }; verify "$2" ;;
  --help|-h)
    echo "Usage: bash sign.sh [--setup|--check|--dev|--build [Tauri args]|--import-ci|--verify <path>|<path>]"
    ;;
  --*) echo "未知参数: $1" >&2; exit 1 ;;
  *) [ "$#" = 1 ] || exit 1; sign "$1" ;;
esac
