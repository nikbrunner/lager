#!/usr/bin/env bash
set -euo pipefail

project="$(cd "$(dirname "$0")/.." && pwd)"
binary="$project/target/debug/lager"
alias_name="${1:-inventory}"
case "$alias_name" in
  inventory|inv) ;;
  *) printf 'Usage: bash scripts/inventory-sandbox.sh [inventory|inv]\n' >&2; exit 2 ;;
esac
if [[ ! -x "$binary" ]]; then
  printf 'Build first, using your normal HOME: cargo build --locked\n' >&2
  exit 1
fi

git_binary="$(command -v git)"
sandbox="$(mktemp -d "${TMPDIR:-/tmp}/lager-inventory.XXXXXX")"
trap 'rm -rf "$sandbox"' EXIT
home="$sandbox/home"
root="$home/repos"
config="$sandbox/config.toml"
mkdir -p "$root"
runtime=(env -i "HOME=$home" "PATH=$PATH" "TERM=${TERM:-xterm-256color}"
  GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
  "HTTP_PROXY=http://127.0.0.1:1" "HTTPS_PROXY=http://127.0.0.1:1")
if [[ ${NO_COLOR+x} ]]; then
  runtime+=("NO_COLOR=$NO_COLOR")
fi

fixture() {
  local path="$root/$1"
  "${runtime[@]}" "$git_binary" init -qb main "$path"
  printf 'sandbox checkout\n' > "$path/README"
  "${runtime[@]}" "$git_binary" -C "$path" add README
  "${runtime[@]}" "$git_binary" -C "$path" -c user.name=Fixture \
    -c user.email=fixture@example.invalid commit -qm fixture
  if [[ -n "$2" ]]; then
    "${runtime[@]}" "$git_binary" -C "$path" remote add origin "$2"
  fi
}

fixture org/alpha git@github.com:org/alpha.git
fixture elsewhere/alpha git@github.com:org/alpha.git
fixture org/dirty git@github.com:org/dirty.git
printf 'local change\n' >> "$root/org/dirty/README"
fixture org/excluded git@github.com:org/excluded.git
fixture local/no-origin ''
fixture 'unicode/庫-café' git@github.com:org/unicode.git

cat > "$config" <<'TOML'
root = "repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/org/alpha"

[[repositories]]
url = "github.com/org/*"
exclude = ["alpha", "excluded", "escaped-warning-\n\t\u001b"]
TOML
for index in {01..30}; do
  printf '\n[[repositories]]\nurl = "github.com/pending/repo-%s"\n' "$index" >> "$config"
done

printf 'Disposable sandbox: %s\nConfig for edits in a second terminal: %s\n' "$sandbox" "$config"
printf 'The sandbox is removed when inventory exits. No real Lager config is used.\n'
status=0
"${runtime[@]}" "$binary" --config "$config" "$alias_name" || status=$?
printf 'Inventory exit status: %s\n' "$status"
exit "$status"
