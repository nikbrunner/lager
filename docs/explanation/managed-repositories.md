# How Lager manages repositories

Lager separates declarations from local state. TOML declarations describe what should be available; `add` creates one local checkout, and `ensure` reconciles every effective declaration. `list` compares declarations with disk without changing either.

A repository identity is normalized from its host and path for duplicate, destination, and origin checks. Stored URL text remains intact. Clone destinations are created and opened relative to held directory capabilities without following symlinks; native Git clones `URL` into `.` after changing the child to the held destination. Renaming or replacing configured-root pathnames cannot redirect the clone, while credentials, prompts, progress, and errors remain native Git behavior. Failed fresh destinations are cleaned descriptor-relatively.
