# Configure providers

Provider entries are an allowlist for discovery and wildcard expansion. Explicit repository arguments work without a provider or `gh`.

## GitHub

`init --github` creates the default entry:

```toml
[providers."github.com"]
preset = "github"
prefix = ""
```

Authenticate the GitHub CLI before using remote discovery:

```sh
gh auth login
lager list --remote --json
```

GitHub catalog requests use `gh`; Git transport remains native Git/SSH.

## Bitbucket Cloud

```toml
[providers."bitbucket.org"]
preset = "bitbucket-cloud"
auth = "bearer"
token_env = "BITBUCKET_TOKEN"
```

`auth` may be `anonymous`, `bearer`, or `basic`. For basic auth use `username_env` and `password_env`. `api_url` may point to an approved API endpoint, which is useful for tests or a compatible service.

## Bitbucket Data Center

```toml
[providers."git.example.com"]
preset = "bitbucket-data-center"
prefix = "company"
api_url = "https://git.example.com"
auth = "bearer"
token_env = "BITBUCKET_DC_TOKEN"
ssh_user = "git"
ssh_port = 7999
```

Data Center requires `api_url`; credentials are read only from the named environment variables. Never put tokens or passwords in TOML.

Use `--include-archived` with `list --remote`, `register`, `add`, or `ensure` when archived repositories should be candidates. Provider errors are reported on stderr. For a no-argument `add`, successful picker selections still run, while any provider catalog failure is retained and forces exit 1.
