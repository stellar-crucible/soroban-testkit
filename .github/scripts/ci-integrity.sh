#!/usr/bin/env bash
# Audits the pipeline itself. The Malware guard inspects the code a pull request
# brings; this inspects the thing that decides whether that code is allowed to
# merge, because a pull request that can weaken its own gate does not need to
# evade it.
#
# Line-based rather than a real YAML parse. Every rule below is a shape that can
# be recognised without a parser, and the ones that need block context track
# indentation. It audits the workflow files as they would be after the change,
# not the diff, because a weakened pipeline is a property of the resulting state.
#
# usage: ci-integrity.sh
#
# It takes no arguments. The mode and the override label come from the GitHub
# event payload, which keeps the workflow free of ${{ }} interpolation inside a
# shell script.
set -uo pipefail

RED=$'\033[31m'; YEL=$'\033[33m'; DIM=$'\033[2m'; BOLD=$'\033[1m'; OFF=$'\033[0m'

MODE=report
OVERRIDE=false

if [ "${GITHUB_EVENT_NAME:-}" = pull_request ]; then
  MODE=enforce
  if jq -e '[.pull_request.labels[]?.name] | index("security-override")' \
        "${GITHUB_EVENT_PATH:-/dev/null}" >/dev/null 2>&1; then
    OVERRIDE=true
  fi
fi

fail=(); warn=()

clip() { printf '%s' "$1" | tr -d '\r' | cut -c1-160; }

# Third-party actions this repository already depends on. Adding one means
# editing this list, and this file lives under .github/scripts, which the
# Malware guard refuses to let a pull request change without the
# security-override label.
ALLOWED_THIRD_PARTY=(
  'dtolnay/rust-toolchain@(stable|beta|nightly|[0-9]+\.[0-9]+(\.[0-9]+)?)'
  'Swatinem/rust-cache@v[0-9]+'
  'EmbarkStudios/cargo-deny-action@v[0-9]+'
)

shopt -s nullglob
files=(.github/workflows/*.yml .github/workflows/*.yaml)
shopt -u nullglob

echo "CI integrity"
echo "${DIM}mode   $MODE   override $OVERRIDE${OFF}"
echo "${DIM}files  ${#files[@]} workflow(s)${OFF}"
echo

if [ "${#files[@]}" -eq 0 ]; then
  echo "${RED}${BOLD}No workflow files found.${OFF} Either the working directory is wrong or the pipeline has been deleted."
  exit 1
fi

for f in "${files[@]}"; do
  # --- Triggers that hand a pull request more than it should have ----------

  # pull_request_target runs the base branch's workflow with a write token and
  # access to secrets. Combined with checking out the head ref it is the classic
  # way to let an untrusted pull request exfiltrate both.
  if grep -qE '^[[:space:]]*pull_request_target:' "$f"; then
    fail+=("$f: uses the pull_request_target trigger, which grants a write token and secrets to a pull request from a fork")
  fi

  # workflow_run completes in the context of the base branch, so a job it
  # triggers inherits write permissions from an untrusted workflow's output.
  if grep -qE '^[[:space:]]*workflow_run:' "$f"; then
    fail+=("$f: uses the workflow_run trigger, whose jobs run with base-branch permissions driven by an untrusted workflow")
  fi

  # --- Least privilege ----------------------------------------------------

  if ! grep -qE '^permissions:' "$f"; then
    fail+=("$f: has no top-level permissions block, so it inherits the repository default instead of asking for what it needs")
  fi

  if grep -qE '^[[:space:]]*pull_request:' "$f"; then
    while IFS= read -r scope; do
      [ -n "$scope" ] || continue
      fail+=("$f: grants $scope on a workflow triggered by pull_request, which runs code from a fork")
    done < <(grep -oE '^[[:space:]]*(contents|packages|actions|id-token|deployments|administration):[[:space:]]*write' "$f" \
               | sed -E 's/^[[:space:]]*//; s/[[:space:]]*:[[:space:]]*write$/: write/' | sort -u)

    while IFS= read -r scope; do
      [ -n "$scope" ] || continue
      warn+=("$f: grants $scope on a pull_request workflow. Forks get a read-only token regardless, so this only takes effect for branches pushed to this repository")
    done < <(grep -oE '^[[:space:]]*(pull-requests|issues):[[:space:]]*write' "$f" \
               | sed -E 's/^[[:space:]]*//; s/[[:space:]]*:[[:space:]]*write$/: write/' | sort -u)
  fi

  # --- Secrets ------------------------------------------------------------

  # This repository stores no secrets. Referencing one from a workflow that a
  # pull request can trigger is how a stored secret gets printed.
  while IFS= read -r line; do
    [ -n "$line" ] && fail+=("$f: references a repository secret — $line")
  done < <(grep -nE '\$\{\{[[:space:]]*secrets\.' "$f" | head -5)

  # --- Script injection ---------------------------------------------------

  # github.event.* fields include text a contributor typed: a pull request
  # title, an issue body, a branch name. Interpolated into a run: script they
  # are shell source, not data. Assign them to env: and read the variable.
  while IFS= read -r line; do
    [ -n "$line" ] && fail+=("$f: interpolates an event field into a shell script — $(clip "$line")")
  done < <(awk '
    function indented(l,   m) { match(l, /^[ ]*/); return RLENGTH }
    {
      l = $0; i = indented(l)
      if (l ~ /^[ ]*run:/) {
        inrun = 1; runind = i
        if (l ~ /\$\{\{[[:space:]]*github\.event\./) print FNR ": " l
        next
      }
      if (inrun && l ~ /[^ ]/ && i <= runind) inrun = 0
      if (inrun && l ~ /\$\{\{[[:space:]]*github\.event\./) print FNR ": " l
    }
  ' "$f")

  # --- Action provenance --------------------------------------------------

  while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    case "$ref" in
      docker://*)
        fail+=("$f: runs $ref, a container image whose contents no review of this repository can see")
        continue ;;
      ./*|/*)
        fail+=("$f: uses the local action $ref, which is code in this repository running with the workflow's token")
        continue ;;
    esac
    if printf '%s' "$ref" | grep -qE '^actions/[^@]+@v[0-9]+$'; then
      continue   # first-party, semver tag
    fi
    if printf '%s' "$ref" | grep -qE '^[^@/]+/[^@/]+@[0-9a-f]{40}$'; then
      continue   # pinned to a commit
    fi
    ok=false
    for allowed in "${ALLOWED_THIRD_PARTY[@]}"; do
      if printf '%s' "$ref" | grep -qE "^$allowed$"; then ok=true; break; fi
    done
    if [ "$ok" = false ]; then
      fail+=("$f: uses $ref, a mutable third-party ref. Pin it to a 40-character commit SHA, or add it to ALLOWED_THIRD_PARTY in this script with a reason")
    fi
  done < <(grep -hoE '^[[:space:]]*uses:[[:space:]]*[^[:space:]#]+' "$f" | sed -E 's/^[[:space:]]*uses:[[:space:]]*//')

done

if [ "${#fail[@]}" -gt 0 ]; then
  echo "${RED}${BOLD}Blocked (${#fail[@]})${OFF}"
  for m in "${fail[@]}"; do printf '  %sx%s %s\n' "$RED" "$OFF" "$m"; done
  echo
fi
if [ "${#warn[@]}" -gt 0 ]; then
  echo "${YEL}${BOLD}Review (${#warn[@]})${OFF}"
  for m in "${warn[@]}"; do printf '  %s!%s %s\n' "$YEL" "$OFF" "$m"; done
  echo
fi
if [ "${#fail[@]}" -eq 0 ] && [ "${#warn[@]}" -eq 0 ]; then
  echo "${BOLD}Nothing to report.${OFF} Every workflow declares least-privilege permissions, no untrusted trigger, no secret reference, no event field interpolated into shell source, and every action ref is first-party, pinned to a commit, or on the allowlist above."
fi

[ "${#fail[@]}" -eq 0 ] && exit 0

if [ "$MODE" = report ]; then
  echo
  echo "${YEL}Report mode: push to the default branch, findings printed rather than enforced.${OFF}"
  exit 0
fi
if [ "$OVERRIDE" = true ]; then
  echo
  echo "${YEL}${BOLD}security-override is set on this pull request, so these findings are recorded but not enforced.${OFF}"
  exit 0
fi

cat <<'TEXT'

This job audits the pipeline rather than the code. A pull request that can edit
the workflow inspecting it can simply delete the inspection, so these findings
are enforced even when the change looks harmless.

If you are adding a legitimate CI step: declare the narrowest permissions the
step needs, pass any event data through env rather than interpolating it into a
run block, and pin third-party actions to a commit SHA. If you are changing the
pipeline on purpose, describe it on the pull request and a maintainer applies
the security-override label.
TEXT
exit 1
