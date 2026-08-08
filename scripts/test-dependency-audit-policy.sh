#!/usr/bin/env bash
set -Eeuo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

fail() {
  printf 'dependency audit policy test failed: %s\n' "$*" >&2
  exit 1
}

assert_equal() {
  local expected="$1"
  local actual="$2"
  local subject="$3"

  if [[ "$actual" != "$expected" ]]; then
    fail "$subject: expected '$expected', got '$actual'"
  fi
}

assert_contains() {
  local file="$1"
  local expected="$2"
  local subject="$3"

  if ! grep -Fqx -- "$expected" "$file"; then
    fail "$subject: missing exact line '$expected' in $file"
  fi
}

assert_not_contains_pattern() {
  local path="$1"
  local pattern="$2"
  local subject="$3"

  if grep -R -E -n -- "$pattern" "$path" >/dev/null; then
    fail "$subject: forbidden pattern '$pattern' found under $path"
  fi
}

locked_cargo_version() {
  local package="$1"
  local lock_file="${2:-Cargo.lock}"

  awk -v target="$package" '
    $0 == "name = \"" target "\"" {
      in_package = 1
      next
    }
    in_package && /^version = / {
      version = $0
      sub(/^version = "/, "", version)
      sub(/"$/, "", version)
      print version
      exit
    }
    in_package && /^\[\[package\]\]$/ {
      exit
    }
  ' "$lock_file"
}

cargo_package_dependency_count() {
  local package="$1"
  local dependency="$2"

  awk -v target="$package" -v dependency="$dependency" '
    $0 == "name = \"" target "\"" {
      in_package = 1
      next
    }
    in_package && /^\[\[package\]\]$/ {
      exit
    }
    in_package && $0 == " \"" dependency "\"," {
      count += 1
    }
    END {
      print count + 0
    }
  ' Cargo.lock
}

test_rust_crypto_backend_and_patched_versions() {
  # Arrange
  local jwt_manifest_line
  local rsa_package_count
  local aws_lc_dependency_count
  local rsa_dependency_count
  local crossbeam_epoch_version
  local anyhow_version

  # Act
  jwt_manifest_line="$(
    grep -F 'jsonwebtoken = ' Cargo.toml || true
  )"
  rsa_package_count="$(
    grep -Fxc 'name = "rsa"' Cargo.lock || true
  )"
  aws_lc_dependency_count="$(cargo_package_dependency_count jsonwebtoken aws-lc-rs)"
  rsa_dependency_count="$(cargo_package_dependency_count jsonwebtoken rsa)"
  crossbeam_epoch_version="$(locked_cargo_version crossbeam-epoch)"
  anyhow_version="$(locked_cargo_version anyhow)"

  # Assert
  assert_equal \
    'jsonwebtoken = { version = "10.4.0", default-features = false, features = ["aws_lc_rs"] }' \
    "$jwt_manifest_line" \
    "JWT backend selection"
  assert_equal "0" "$rsa_package_count" "deprecated RSA implementation removal"
  assert_equal "1" "$aws_lc_dependency_count" "jsonwebtoken AWS-LC dependency"
  assert_equal "0" "$rsa_dependency_count" "jsonwebtoken RustCrypto RSA dependency"
  assert_equal "0.9.20" "$crossbeam_epoch_version" "crossbeam-epoch advisory fix"
  assert_equal "1.0.104" "$anyhow_version" "anyhow unsoundness fix"
}

test_web_overrides_and_exception_scope() {
  # Arrange
  local gui_policy="apps/schemahub-gui/pnpm-workspace.yaml"
  local demo_policy="apps/schemahub-demo/pnpm-workspace.yaml"
  local ignored_advisories
  local gui_esbuild_lock_count
  local gui_postcss_lock_count
  local gui_nanoid_lock_count
  local gui_router_lock_count
  local gui_vite_lock_count
  local demo_postcss_lock_count
  local demo_nanoid_lock_count
  local demo_undici_lock_count
  local demo_sharp_lock_count

  # Act
  ignored_advisories="$(
    awk '
      /^auditConfig:$/ {
        in_audit_config = 1
        next
      }
      in_audit_config && /^[^[:space:]]/ {
        exit
      }
      in_audit_config && /^[[:space:]]+- / {
        advisory = $0
        sub(/^[[:space:]]+- /, "", advisory)
        print advisory
      }
    ' "$gui_policy"
  )"
  gui_esbuild_lock_count="$(
    grep -Fxc '  esbuild@0.28.1:' apps/schemahub-gui/pnpm-lock.yaml || true
  )"
  gui_postcss_lock_count="$(
    grep -Fxc '  postcss@8.5.23:' apps/schemahub-gui/pnpm-lock.yaml || true
  )"
  gui_nanoid_lock_count="$(
    grep -Fxc '  nanoid@3.3.17:' apps/schemahub-gui/pnpm-lock.yaml || true
  )"
  gui_router_lock_count="$(
    grep -Fxc '  react-router-dom@7.18.1:' apps/schemahub-gui/pnpm-lock.yaml || true
  )"
  gui_vite_lock_count="$(
    grep -Fxc '  vite@7.3.6:' apps/schemahub-gui/pnpm-lock.yaml || true
  )"
  demo_postcss_lock_count="$(
    grep -Fxc '  postcss@8.5.23:' apps/schemahub-demo/pnpm-lock.yaml || true
  )"
  demo_nanoid_lock_count="$(
    grep -Fxc '  nanoid@3.3.17:' apps/schemahub-demo/pnpm-lock.yaml || true
  )"
  demo_undici_lock_count="$(
    grep -Fxc '  undici@7.29.0:' apps/schemahub-demo/pnpm-lock.yaml || true
  )"
  demo_sharp_lock_count="$(
    grep -Fxc '  sharp@0.35.3:' apps/schemahub-demo/pnpm-lock.yaml || true
  )"

  # Assert
  assert_equal \
    "GHSA-qwww-vcr4-c8h2" \
    "$ignored_advisories" \
    "GUI audit exception allowlist"
  assert_contains "$gui_policy" "  esbuild: 0.28.1" "GUI esbuild override"
  assert_contains "$gui_policy" "  postcss: 8.5.23" "GUI PostCSS override"
  assert_contains "$gui_policy" "  nanoid: 3.3.17" "GUI nanoid override"
  assert_contains "$demo_policy" "  postcss: 8.5.23" "demo PostCSS override"
  assert_contains "$demo_policy" "  nanoid: 3.3.17" "demo nanoid override"
  assert_contains "$demo_policy" "  undici: 7.29.0" "demo undici override"
  assert_contains "$demo_policy" "  sharp: 0.35.3" "demo Sharp override"
  assert_equal "2" "$gui_esbuild_lock_count" "GUI esbuild lock resolution"
  assert_equal "2" "$gui_postcss_lock_count" "GUI PostCSS lock resolution"
  assert_equal "1" "$gui_nanoid_lock_count" "GUI nanoid lock resolution"
  assert_equal "1" "$gui_router_lock_count" "GUI React Router lock resolution"
  assert_equal "1" "$gui_vite_lock_count" "GUI Vite lock resolution"
  assert_equal "2" "$demo_postcss_lock_count" "demo PostCSS lock resolution"
  assert_equal "1" "$demo_nanoid_lock_count" "demo nanoid lock resolution"
  assert_equal "1" "$demo_undici_lock_count" "demo undici lock resolution"
  assert_equal "1" "$demo_sharp_lock_count" "demo Sharp lock resolution"
  assert_contains \
    apps/schemahub-gui/src/main.tsx \
    "import { BrowserRouter } from 'react-router-dom';" \
    "client-only GUI router import"
  assert_contains \
    apps/schemahub-gui/src/main.tsx \
    "        <BrowserRouter>" \
    "client-only GUI router"
  assert_not_contains_pattern \
    apps/schemahub-gui/src \
    'react-router(-dom)?/server|react-server-dom|unstable_(createCallServer|createServerReference|decodeAction|decodeFormState|decodeReply|RSC)' \
    "React Router RSC exception scope"
}

test_ci_executes_all_dependency_audits() {
  # Arrange
  local workflow=".github/workflows/ci.yml"
  local contract_test_count
  local rust_audit_contract_test_count
  local cargo_audit_installer_test_count
  local cargo_audit_install_count
  local cargo_audit_gate_count
  local cargo_audit_install_root_count
  local cargo_audit_binary_count
  local cargo_auditable_contract_test_count
  local cargo_auditable_verify_count
  local container_supply_chain_test_count
  local pnpm_audit_count

  # Act
  contract_test_count="$(
    grep -Fxc '        run: scripts/test-dependency-audit-policy.sh' "$workflow" || true
  )"
  rust_audit_contract_test_count="$(
    grep -Fxc '        run: scripts/test-audit-rust-dependencies.sh' \
      "$workflow" || true
  )"
  cargo_audit_installer_test_count="$(
    grep -Fxc '        run: scripts/test-install-cargo-audit.sh' \
      "$workflow" || true
  )"
  cargo_audit_install_count="$(
    grep -Fxc '        run: scripts/install-cargo-audit.sh' "$workflow" || true
  )"
  cargo_audit_gate_count="$(
    grep -Fxc '        run: scripts/audit-rust-dependencies.sh' \
      "$workflow" || true
  )"
  cargo_audit_install_root_count="$(
    grep -Fxc \
      "          SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT: \${{ runner.temp }}/schemahub-cargo-audit" \
      "$workflow" || true
  )"
  cargo_audit_binary_count="$(
    grep -Fxc \
      "          SCHEMAHUB_CARGO_AUDIT_BIN: \${{ runner.temp }}/schemahub-cargo-audit/bin/cargo-audit" \
      "$workflow" || true
  )"
  cargo_auditable_contract_test_count="$(
    grep -Fxc '        run: scripts/test-verify-cargo-auditable.sh' \
      "$workflow" || true
  )"
  cargo_auditable_verify_count="$(
    grep -Fxc '        run: scripts/verify-cargo-auditable.sh' \
      "$workflow" || true
  )"
  container_supply_chain_test_count="$(
    grep -Fxc '        run: scripts/test-container-supply-chain-policy.sh' \
      "$workflow" || true
  )"
  pnpm_audit_count="$(
    grep -Fxc '        run: pnpm audit --audit-level low' "$workflow" || true
  )"

  # Assert
  assert_equal "1" "$contract_test_count" "dependency policy contract CI step"
  assert_equal \
    "1" \
    "$rust_audit_contract_test_count" \
    "Rust dependency warning contract CI step"
  assert_equal \
    "1" \
    "$cargo_audit_installer_test_count" \
    "cargo-audit installer contract CI step"
  assert_equal "1" "$cargo_audit_install_count" "pinned cargo-audit install CI step"
  assert_equal "1" "$cargo_audit_gate_count" "Rust dependency audit gate CI step"
  assert_equal \
    "1" \
    "$cargo_audit_install_root_count" \
    "isolated cargo-audit installation root"
  assert_equal \
    "2" \
    "$cargo_audit_binary_count" \
    "exact cargo-audit consumer binaries"
  assert_equal \
    "1" \
    "$cargo_auditable_contract_test_count" \
    "cargo-auditable supply-chain contract CI step"
  assert_equal \
    "1" \
    "$cargo_auditable_verify_count" \
    "cargo-auditable supply-chain verification CI step"
  assert_equal \
    "1" \
    "$container_supply_chain_test_count" \
    "container supply-chain policy CI step"
  assert_equal "2" "$pnpm_audit_count" "web dependency audit CI steps"
}

test_auditor_supply_chain_is_reproducible() {
  # Arrange
  local installer="scripts/install-cargo-audit.sh"
  local auditor_lock="tools/cargo-audit/Cargo.lock"
  local actual_lock_sha256
  local stale_registry_install_count

  # Act
  actual_lock_sha256="$(sha256sum "$auditor_lock" | awk '{print $1}')"
  stale_registry_install_count="$(
    awk '
      /cargo install/ && /cargo-audit([[:space:]@]|$)/ && /--locked/ {
        count += 1
      }
      END {
        print count + 0
      }
    ' .github/workflows/*.yml
  )"

  # Assert
  assert_contains \
    "$installer" \
    'AUDITOR_VERSION="0.22.2"' \
    "pinned cargo-audit version"
  assert_contains \
    "$installer" \
    'AUDITOR_ARCHIVE_SHA256="700c2b240f7fd330c24b675fe429f73a5b676531fcc6300400b2b67f155ba12a"' \
    "pinned cargo-audit source archive"
  assert_contains \
    "$installer" \
    'AUDITOR_LOCK_SHA256="02b6d4858475e8028b9e35aa7e86de2b06ae42df9432c9a5e6037d01e0ed9947"' \
    "reviewed cargo-audit lock identity"
  assert_contains \
    "$installer" \
    "  --deny warnings" \
    "zero-warning cargo-audit self-audit"
  assert_equal \
    "02b6d4858475e8028b9e35aa7e86de2b06ae42df9432c9a5e6037d01e0ed9947" \
    "$actual_lock_sha256" \
    "reviewed cargo-audit lock checksum"
  assert_equal \
    "0.9.20" \
    "$(locked_cargo_version crossbeam-epoch "$auditor_lock")" \
    "cargo-audit crossbeam-epoch advisory fix"
  assert_equal \
    "0.11.16" \
    "$(locked_cargo_version quinn-proto "$auditor_lock")" \
    "cargo-audit quinn-proto advisory fix"
  assert_equal \
    "0.9.11" \
    "$(locked_cargo_version memmap2 "$auditor_lock")" \
    "cargo-audit memmap2 unsoundness fix"
  assert_equal \
    "" \
    "$(locked_cargo_version anyhow "$auditor_lock")" \
    "cargo-audit anyhow unsoundness removal"
  assert_equal \
    "0" \
    "$stale_registry_install_count" \
    "stale published cargo-audit lock installation"
}

test_release_build_tool_is_exact_and_isolated() {
  # Arrange
  local verifier="scripts/verify-cargo-auditable.sh"
  local workflow=".github/workflows/release.yml"
  local old_install_count

  # Act
  old_install_count="$(
    grep -Fxc \
      '          cargo install cargo-auditable --locked --version 0.7.5' \
      "$workflow" || true
  )"

  # Assert
  assert_contains \
    "$verifier" \
    'AUDITABLE_VERSION="0.7.5"' \
    "pinned cargo-auditable version"
  assert_contains \
    "$verifier" \
    'AUDITABLE_ARCHIVE_SHA256="cd121127b91d68074770a620544182345d7db56d03dcbd85316ab11e54a5b1bc"' \
    "pinned cargo-auditable source archive"
  assert_contains \
    "$verifier" \
    'AUDITABLE_LOCK_SHA256="3a49de28391ca0e99709a96c64cd8e24f8f96d622f5a8360c2fbd5d8e0d9965e"' \
    "pinned cargo-auditable lock"
  assert_contains \
    "$verifier" \
    "  --deny warnings" \
    "zero-warning cargo-auditable audit"
  assert_equal "0" "$old_install_count" "ambient cargo-auditable installation"
  assert_contains \
    "$workflow" \
    "          auditable_root=\"\$RUNNER_TEMP/schemahub-cargo-auditable\"" \
    "isolated cargo-auditable installation root"
  assert_contains \
    "$workflow" \
    "            --version 0.7.5 \\" \
    "exact cargo-auditable release version"
  assert_contains \
    "$workflow" \
    "            --force \\" \
    "forced exact cargo-auditable installation"
  assert_contains \
    "$workflow" \
    "            --root \"\$auditable_root\"" \
    "isolated cargo-auditable install"
  assert_contains \
    "$workflow" \
    "          \"\$auditable_bin\" auditable build --release --locked \\" \
    "exact cargo-auditable binary invocation"
}

test_rust_crypto_backend_and_patched_versions
test_web_overrides_and_exception_scope
test_ci_executes_all_dependency_audits
test_auditor_supply_chain_is_reproducible
test_release_build_tool_is_exact_and_isolated

printf '%s\n' 'dependency audit policy tests passed'
