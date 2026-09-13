# v1 scope and non-goals

v1 manages declarations, discovery, local availability, hooks, listing, and guarded removal on Linux and macOS for x86_64 and ARM64.

It does not track fleet branch/dirty/ahead status, pull or push repositories, traverse arbitrary dirty repositories, rebuild working trees, or provide a hosted service. GitHub, Bitbucket Cloud, and Bitbucket Data Center discovery are supported; clone authentication remains the responsibility of native Git and SSH. A future terminal UI can call the application services without changing the CLI contract.
