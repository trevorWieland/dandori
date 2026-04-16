default:
    @just --list

set shell := ["bash", "-euo", "pipefail", "-c"]

cargo := "SQLX_OFFLINE=true " + env("CARGO", "cargo")
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

db-migrate:
    @{{ cargo }} run -p dandori-migrate --quiet

build:
    @{{ cargo }} build --workspace

check:
    @{{ cargo }} check --workspace --all-targets

test *args:
    @{{ cargo }} nextest run --workspace --profile ci --no-tests=pass {{ args }}

phase1-gate:
    @{{ cargo }} nextest run -p dandori-store --profile ci --no-tests=pass
    @{{ cargo }} nextest run -p dandori-app-services --profile ci --no-tests=pass
    @{{ cargo }} nextest run -p dandori-mcp --profile ci --no-tests=pass

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
    @{{ cargo }} deny check --hide-inclusion-graph

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
    @{{ cargo }} run -p dandori-deps-check --quiet

check-sql-policy:
    #!/usr/bin/env bash
    set -euo pipefail
    # `sqlx::` is allowed only in repository modules that enforce compile-time
    # SQL verification via `sqlx::query!` / `query_as!`. Stringly-typed or
    # SeaORM-only modules are NOT on the allow-list; adding a new entry here
    # should be a deliberate policy decision, not a work-around.
    offenders=$(
      rg -n 'sqlx::' crates/dandori-store/src \
        --glob '!crates/dandori-store/src/repositories/common.rs' \
        --glob '!crates/dandori-store/src/repositories/issue.rs' \
        --glob '!crates/dandori-store/src/repositories/outbox.rs' \
        --glob '!crates/dandori-store/src/repositories/partition.rs' \
        --glob '!crates/dandori-store/src/pg_store.rs' || true
    )
    if [[ -n "$offenders" ]]; then
      echo "FAIL: sqlx usage is only allowed in sanctioned escape-hatch modules"
      echo "$offenders"
      exit 1
    fi

check-sqlx-offline:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d .sqlx ]]; then
      echo "FAIL: .sqlx metadata directory is missing"
      exit 1
    fi
    if ! find .sqlx -maxdepth 1 -name 'query-*.json' -print -quit | grep -q .; then
      echo "FAIL: .sqlx metadata must include query-*.json files"
      exit 1
    fi
    SQLX_OFFLINE=true {{ cargo }} check -p dandori-store --all-targets

check-no-leaks:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' EXIT
    {{ cargo }} nextest run --workspace --profile ci --no-tests=pass --status-level leak --final-status-level all 2>&1 | tee "$tmp"
    if rg -n '(?i)\bleak\b' "$tmp" >/dev/null 2>&1; then
      echo "FAIL: leaky tests detected in nextest output"
      exit 1
    fi

perf-gate:
    @{{ cargo }} test -p dandori-store --test phase1_perf_gate -- --nocapture
    @{{ cargo }} test -p dandori-app-services --test phase1_perf_gate -- --nocapture

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
    # sqlx is allowed in dandori-store (production boundary) and in
    # dandori-test-support (test-only crate whose sole consumers are
    # `[dev-dependencies]` in integration test directories — never linked
    # into production binaries).
    if rg -n 'sqlx::' crates/*/src \
         --glob '!crates/dandori-store/**' \
         --glob '!crates/dandori-test-support/**' \
         >/dev/null 2>&1; then
      echo "FAIL: sqlx usage outside dandori-store source boundary"
      rg -n 'sqlx::' crates/*/src \
        --glob '!crates/dandori-store/**' \
        --glob '!crates/dandori-test-support/**'
      exit 1
    fi

check-ci-parity:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! grep -Eq 'run:\s*just ci' .github/workflows/ci.yml; then
      echo "FAIL: workflow must run just ci"
      exit 1
    fi

ci: fmt lint check test phase1-gate perf-gate check-no-leaks coverage deny machete doc check-lines check-suppression check-deps check-sql-policy check-sqlx-offline check-thin-interface check-store-boundary check-ci-parity
    @echo "==> All CI checks passed"
