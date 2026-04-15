# Contributing to Dandori

## Workflow

1. Create a branch from `main`
2. Make focused changes
3. Run `just ci`
4. Open a PR
5. Address review feedback and merge via squash

## Quality Rules

- `unsafe` is forbidden
- Warnings are treated as errors in CI
- No inline lint suppression (`#[allow]`, `#[expect]`) in source files
- No `unwrap()`, `panic!`, `todo!`, `dbg!`, `println!`, or `eprintln!` in committed code
- Thin binaries, shared business logic in library crates

## Commit Style

Use Conventional Commits:

- `feat(scope): ...`
- `fix(scope): ...`
- `refactor(scope): ...`
- `test(scope): ...`
- `docs(scope): ...`
- `chore(scope): ...`
