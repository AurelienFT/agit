## Summary

<!-- One sentence. What changes, in which crate. -->

## Details

<!-- Bullet list of what changed and why. -->

## How this was tested

<!-- Commands run, results, edge cases considered. -->

## Checklist

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Updated docs (`docs/`, `CLAUDE.md`, `README.md`) if behavior or vocabulary changed
- [ ] `.agit/agents.yaml` still valid (`cargo run -p agit-cli -- validate`)

<!--
If this PR was opened by an Agit agent, the body above is templated by
.github/workflows/agit-runner.yml. The same checks apply.
-->
