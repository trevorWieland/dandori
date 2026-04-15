SQLx offline metadata placeholder.

This repository currently limits raw SQL to sanctioned store escape hatches.
If additional compile-time SQLx macros are introduced, refresh metadata with:

`cargo sqlx prepare --workspace`
