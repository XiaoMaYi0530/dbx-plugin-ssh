## Agent handoff

- Task ID:
- Stage: `contract` / `implementation` / `review` / `integration`
- Base SHA:
- Allowed paths respected: yes / no
- Depends on PR:

## Verification

- [ ] repository and connection-form validation
- [ ] frontend typecheck/test/build
- [ ] `cargo fmt --manifest-path backend/Cargo.toml --check`
- [ ] `cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings`
- [ ] `cargo test --locked --manifest-path backend/Cargo.toml`
- [ ] SSH container smoke and relevant MCP smoke

## Security review

- [ ] No real credentials/private keys/production hosts used
- [ ] Authentication/secret/command-execution changes reviewed by a human
- [ ] No install or DBX restart performed by the agent

## Review notes

- Changed files:
- Risks or known limitations:
- Follow-up task:

