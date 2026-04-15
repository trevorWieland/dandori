default:
    @just --list

set shell := ["bash", "-euo", "pipefail", "-c"]

cargo := env("CARGO", "cargo")
max_lines := "500"
toml_globs := "Cargo.toml bin/*/Cargo.toml crates/*/Cargo.toml .cargo/*.toml .config/*.toml rust-toolchain.toml clippy.toml taplo.toml deny.toml rustfmt.toml lefthook.yml"

bootstrap:
    #!/usr/bin/env bash
    set -euo pipefail

    if command -v cargo-binstall >/dev/null 2>&1; then
      cargo binstall --no-confirm just cargo-nextest cargo-deny cargo-llvm-cov cargo-machete taplo-cli || true
    else
      cargo install --locked just cargo-nextest cargo-deny cargo-llvm-cov cargo-machete taplo-cli || true
    fi

    if ! command -v lefthook >/dev/null 2>&1; then
      if curl -1sLf 'https://dl.cloudsmith.io/public/evilmartians/lefthook/setup.shell.sh' | bash >/dev/null 2>&1; then
        true
      elif command -v brew >/dev/null 2>&1; then
        brew install lefthook
      fi
    fi

    if command -v lefthook >/dev/null 2>&1; then
      lefthook install
    fi

build:
    @{{ cargo }} build --workspace

check:
    @{{ cargo }} check --workspace --all-targets

test *args:
    @{{ cargo }} nextest run --workspace --profile ci --no-tests=pass {{ args }}

phase1-gate:
    @{{ cargo }} nextest run -p dandori-store --profile ci --no-tests=pass
    @{{ cargo }} nextest run -p dandori-app-services --profile ci --no-tests=pass

coverage:
    @{{ cargo }} llvm-cov nextest --workspace --lcov --output-path lcov.info --no-tests=pass

lint:
    @RUSTFLAGS="-D warnings" {{ cargo }} clippy --workspace --all-targets -- -D warnings

fmt:
    @{{ cargo }} fmt --check
    @taplo fmt --check {{ toml_globs }}

fmt-fix:
    @{{ cargo }} fmt
    @taplo fmt {{ toml_globs }}

fix:
    @{{ cargo }} fmt
    @{{ cargo }} clippy --workspace --all-targets --fix --allow-dirty --allow-staged -- -D warnings

deny:
    @{{ cargo }} deny check

machete:
    @{{ cargo }} machete

doc:
    @RUSTDOCFLAGS="-D warnings" {{ cargo }} doc --workspace --no-deps

check-lines:
    #!/usr/bin/env bash
    set -euo pipefail
    failed=0
    while IFS= read -r -d '' file; do
      lines=$(wc -l < "$file")
      if [[ "$lines" -gt {{ max_lines }} ]]; then
        echo "FAIL: $file has $lines lines (max {{ max_lines }})"
        failed=1
      fi
    done < <(find crates/ bin/ -name '*.rs' -print0)
    if [[ "$failed" -eq 1 ]]; then exit 1; fi

check-suppression:
    #!/usr/bin/env bash
    set -euo pipefail
    found=0
    if grep -rn '#\[allow(' crates/ bin/ --include='*.rs' 2>/dev/null; then found=1; fi
    if grep -rn '#\[expect(' crates/ bin/ --include='*.rs' 2>/dev/null; then found=1; fi
    if grep -rn '#!\[allow(' crates/ bin/ --include='*.rs' 2>/dev/null; then found=1; fi
    if [[ "$found" -eq 1 ]]; then
      echo "FAIL: Inline lint suppression is prohibited"
      exit 1
    fi

check-deps:
    #!/usr/bin/env bash
    set -euo pipefail
    metadata=$({{ cargo }} metadata --format-version 1 --no-deps)
    failed=0

    foundation=("dandori-domain" "dandori-contract" "dandori-policy" "dandori-observability")
    capability=("dandori-store" "dandori-graph" "dandori-workflow" "dandori-sync-github" "dandori-orchestrator" "dandori-app-services")

    for f in "${foundation[@]}"; do
      deps=$(echo "$metadata" | jq -r ".packages[] | select(.name == \"$f\") | .dependencies[].name" 2>/dev/null || true)
      for c in "${capability[@]}"; do
        if echo "$deps" | grep -qx "$c"; then
          echo "FAIL: foundation crate '$f' depends on capability crate '$c'"
          failed=1
        fi
      done
    done

    transport=("dandori-api" "dandori-mcp" "dandori-worker")
    forbidden=("dandori-domain" "dandori-store" "dandori-orchestrator")

    for b in "${transport[@]}"; do
      deps=$(echo "$metadata" | jq -r ".packages[] | select(.name == \"$b\") | .dependencies[].name" 2>/dev/null || true)
      for f in "${forbidden[@]}"; do
        if echo "$deps" | grep -qx "$f"; then
          echo "FAIL: transport binary '$b' depends directly on '$f'"
          failed=1
        fi
      done
    done

    if [[ "$failed" -eq 1 ]]; then exit 1; fi

check-thin-interface:
    #!/usr/bin/env bash
    set -euo pipefail
    failed=0
    for pattern in 'sqlx::' 'dandori_store::' 'dandori_domain::'; do
      if rg -n "$pattern" bin/dandori-api/src bin/dandori-mcp/src bin/dandori-worker/src >/dev/null 2>&1; then
        echo "FAIL: transport layer contains forbidden pattern: $pattern"
        failed=1
      fi
    done
    if [[ "$failed" -eq 1 ]]; then exit 1; fi

check-store-boundary:
    #!/usr/bin/env bash
    set -euo pipefail
    if rg -n 'sqlx::' crates/*/src --glob '!crates/dandori-store/**' >/dev/null 2>&1; then
      echo "FAIL: sqlx usage outside dandori-store source boundary"
      rg -n 'sqlx::' crates/*/src --glob '!crates/dandori-store/**'
      exit 1
    fi

check-ci-parity:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! grep -Eq 'run:\s*just ci' .github/workflows/ci.yml; then
      echo "FAIL: workflow must run just ci"
      exit 1
    fi

ci: fmt lint check test phase1-gate coverage deny machete doc check-lines check-suppression check-deps check-thin-interface check-store-boundary check-ci-parity
    @echo "==> All CI checks passed"
