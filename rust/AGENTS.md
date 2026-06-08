# Rust workspace guidance

When adding long text, JavaScript snippets/scripts, test workflow scripts, fixtures, templates, or other non-trivial embedded content, keep that content in dedicated asset/fixture files instead of embedding it inline in Rust string literals.

Use `include_str!(...)` or the existing asset/migration embedding pattern to include those files into the compiled binary. This keeps the Rust code focused on runtime wiring and makes embedded content easier to read, test, diff, and maintain.

Examples:

- Put QuickJS runtime helpers under `rust/engine/src/js_runtime/rquickjs_js/`.
- Put CLI help/LLM guidance under `rust/cli/assets/`.
- Put test JavaScript workflow scripts and larger fixtures in dedicated fixture/asset files near the tests that use them.
- Include them from Rust with `include_str!("<relative-asset-file>")`.
- Keep inline Rust strings only for very small snippets where a separate asset would make the code less clear.

When making user-visible CLI changes, also review and update `rust/cli/assets/llm.txt` so `smol-wf llm txt` stays accurate for coding agents.

The CLI also packages copies of the smol-workflows agent skills under `rust/cli/assets/skills/` because published Cargo crates cannot include files from the repository-level `harness/` tree. Treat `harness/plugins/smol-workflows/skills/` as the canonical source. After editing those harness skills, run:

```sh
./hack/sync-cli-skill-assets.sh
cargo test -p smol-workflow-cli --test cli packaged_skill_assets_match_harness_sources
```

Do not hand-copy the files. The sync test is intended to catch drift between the harness skills and the packaged CLI copies.

After making Rust workspace changes, run the relevant format, lint, and test commands before handing off. At minimum, prefer:

```sh
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

If the change also touches the TypeScript SDK or generated workflow-facing types, also run:

```sh
npm --prefix ts/sdk run build
```
