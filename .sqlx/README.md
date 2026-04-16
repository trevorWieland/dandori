SQLx offline metadata directory.

This directory must contain `query-*.json` artifacts generated from compile-time SQLx macros.

Refresh metadata (requires a PostgreSQL schema matching current migrations):

1. `export DANDORI_DATABASE_URL=postgres://...`
2. `cargo run -p dandori-migrate --quiet`
3. `cargo sqlx prepare --workspace -- --all-targets`

CI validates offline usage by checking for query metadata files and compiling store targets with `SQLX_OFFLINE=true`.
