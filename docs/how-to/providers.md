# Configure providers

Providers let Lager discover repositories for no-argument selection and expand wildcard declarations. Explicit repository arguments work without a provider.

Start with the [configuration reference](../reference/config.md) for every field and validation rule.

## GitHub

Create the default provider with `init --github`, or add it to TOML:

```toml
[providers."github.com"]
preset = "github"
```

Authenticate `gh` before remote discovery:

```sh
gh auth login
lager list --remote --json
```

GitHub catalog requests use `gh`; cloning continues to use native Git and your SSH credentials.

## Bitbucket Cloud

Use a bearer token from the environment:

```toml
[providers."bitbucket.org"]
preset = "bitbucket-cloud"
auth = "bearer"
token_env = "BITBUCKET_TOKEN"
```

For basic authentication, replace `token_env` with `username_env` and `password_env`. `auth` defaults to `anonymous` when omitted. `api_url` is optional for Cloud.

## Bitbucket Data Center

Data Center requires an HTTP(S) API URL:

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

Lager reads credentials from the named environment variables. Keep tokens and passwords out of TOML.

## Discover archived repositories

Archived repositories are excluded unless you ask for them:

```sh
lager list --remote --include-archived
lager register --include-archived
lager add --include-archived
lager ensure --include-archived
```

`list --include-archived` requires `--remote`. For a no-argument `add`, successful picker selections still run when another provider fails, but the command exits 1 to report that failure.
