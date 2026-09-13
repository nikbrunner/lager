# Provider reference

Providers are configured under `[providers."host"]`. The host selects the discovery adapter; `preset` must be `github`, `bitbucket-cloud`, or `bitbucket-data-center`.

GitHub discovery uses authenticated `gh` and supports pagination. Bitbucket Cloud uses workspace and repository REST endpoints. Data Center uses repository/project REST endpoints and requires `api_url`. All adapters filter archived repositories unless `--include-archived` is set and follow pagination before returning candidates.

Authentication modes for Bitbucket are `anonymous`, `bearer`, and `basic`. Bearer reads `token_env`; basic reads `username_env` and `password_env`. SSH clone settings may override `ssh_user` and `ssh_port`. Secrets are never persisted by Lager. Bitbucket Cloud follows relative or absolute opaque pagination links only when they resolve to the configured HTTP(S) origin; scheme, host, port, and userinfo changes are rejected before credentials are attached or a request is sent. Discovery errors are separate from native Git errors and use exit code 1.

Configured repositories map to `root/prefix/remote-path`; the configured host is not inserted into the local path. Data Center removes a leading `~` only from the personal-project path segment. List, add, ensure, and hook use that same validated relative destination.
