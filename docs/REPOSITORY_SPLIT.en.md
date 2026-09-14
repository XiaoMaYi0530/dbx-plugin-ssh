# SSH standalone repository migration notes

This repository was split from `/Users/Jinpy/btroot/dbx-plugins/ssh` at source
commit `ce19beb4fa75cc79048e3ba8852c298d674fd9b3`. The initial import used
`git subtree split --prefix=ssh`, preserving the SSH directory history.

The split is self-contained: SSH-only frontend adapters live under
`shared/frontend/`, the connection-form contract check is under
`scripts/connection-forms/`, and the matching DBX sidecar SDK is vendored under
`shared/sdk/rust/dbx-plugin-sdk/`. No LDAP, Files, or Kafka implementation was
copied, and no `../shared` or `../host` path is required for the normal build.

`ci.yml` validates manifest/backend identity, runs the frontend and Rust tests,
executes the offline MCP stdio smoke, and builds a Linux candidate package.
`release.yml` delegates GitHub Release packaging to the official DBX reusable
workflow. It does not invent a remote repository, signing key, or secret.

The manifest currently uses explicit `https://github.com/TODO/` placeholders for
`source` and `homepage`. Replace them after the GitHub repository is created,
then publish only a matching `ssh-v<version>` tag. Live SSH, Docker, and DBX.app
host tests remain opt-in; unavailable environments must be reported as `SKIP`.
