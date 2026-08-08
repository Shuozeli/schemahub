#!/usr/bin/env bash
set -Eeuo pipefail

: "${SCHEMAHUB_FAKE_CARGO_AUDIT_SCENARIO:?}"

if [[ "$#" -ne 2 ]] || [[ "$1" != "audit" ]]; then
  printf 'unexpected fake cargo-audit invocation\n' >&2
  exit 2
fi
shift

if [[ "$1" == "--version" ]]; then
  if [[ "${SCHEMAHUB_FAKE_CARGO_AUDIT_SCENARIO}" == "wrong-version" ]]; then
    printf '%s\n' 'cargo-audit-audit 0.22.1'
  else
    printf '%s\n' 'cargo-audit-audit 0.22.2'
  fi
  exit 0
fi

if [[ "$1" != "--json" ]]; then
  printf 'unexpected fake cargo-audit argument: %s\n' "$1" >&2
  exit 2
fi

case "${SCHEMAHUB_FAKE_CARGO_AUDIT_SCENARIO}" in
  accepted | wrong-version)
    printf '%s\n' '{"vulnerabilities":{"found":false,"count":0,"list":[]},"warnings":{"unmaintained":[{"kind":"unmaintained","package":{"name":"paste","version":"1.0.15","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":{"id":"RUSTSEC-2024-0436"}}],"yanked":[{"kind":"yanked","package":{"name":"spin","version":"0.9.8","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":null}]}}'
    ;;
  vulnerability)
    printf '%s\n' '{"vulnerabilities":{"found":true,"count":1,"list":[{"advisory":{"id":"RUSTSEC-2099-0001"},"package":{"name":"unsafe-crate","version":"1.0.0"}}]},"warnings":{"unmaintained":[{"kind":"unmaintained","package":{"name":"paste","version":"1.0.15","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":{"id":"RUSTSEC-2024-0436"}}],"yanked":[{"kind":"yanked","package":{"name":"spin","version":"0.9.8","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":null}]}}'
    ;;
  new-warning)
    printf '%s\n' '{"vulnerabilities":{"found":false,"count":0,"list":[]},"warnings":{"unmaintained":[{"kind":"unmaintained","package":{"name":"paste","version":"1.0.15","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":{"id":"RUSTSEC-2024-0436"}},{"kind":"unmaintained","package":{"name":"new-warning","version":"1.0.0","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":{"id":"RUSTSEC-2099-0002"}}],"yanked":[{"kind":"yanked","package":{"name":"spin","version":"0.9.8","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":null}]}}'
    ;;
  missing-warning)
    printf '%s\n' '{"vulnerabilities":{"found":false,"count":0,"list":[]},"warnings":{"unmaintained":[{"kind":"unmaintained","package":{"name":"paste","version":"1.0.15","source":"registry+https://github.com/rust-lang/crates.io-index"},"advisory":{"id":"RUSTSEC-2024-0436"}}],"yanked":[]}}'
    ;;
  malformed)
    printf '%s\n' '{'
    ;;
  command-failure)
    printf '%s\n' '{"vulnerabilities":{"found":false,"count":0,"list":[]},"warnings":{}}'
    exit 7
    ;;
  *)
    printf 'unknown fake cargo-audit scenario: %s\n' \
      "${SCHEMAHUB_FAKE_CARGO_AUDIT_SCENARIO}" >&2
    exit 2
    ;;
esac
