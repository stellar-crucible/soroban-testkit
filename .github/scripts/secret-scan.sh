#!/usr/bin/env bash
# Scans the lines a diff adds for credentials. Pattern-based: it looks for the
# shapes real providers issue, plus the shapes that hide them (credentials
# embedded in a URL, a key assigned to a literal).
#
# This is deliberately not an entropy heuristic. A Rust repository's Cargo.lock
# is full of 64-character hex checksums, and an entropy check reports every one
# of them. GitHub's own secret scanning, enabled on this repository, does
# provider-pattern matching with validity checks and covers the same ground with
# a maintained ruleset; this script covers the pull request diff before it lands.
#
# usage: secret-scan.sh
#
# It takes no arguments. The range, the mode and the override label all come
# from the GitHub event payload, which keeps the workflow free of ${{ }}
# interpolation inside a shell script.
set -uo pipefail

RED=$'\033[31m'; DIM=$'\033[2m'; BOLD=$'\033[1m'; OFF=$'\033[0m'

EVENT="${GITHUB_EVENT_NAME:-}"
PAYLOAD="${GITHUB_EVENT_PATH:-}"

MODE=report
OVERRIDE=false
BASE=""
HEAD="${GITHUB_SHA:-}"

if [ "$EVENT" = pull_request ] && [ -s "$PAYLOAD" ]; then
  BASE="$(jq -r '.pull_request.base.sha // empty' "$PAYLOAD")"
  HEAD="$(jq -r '.pull_request.head.sha // empty' "$PAYLOAD")"
  MODE=enforce
  if jq -e '[.pull_request.labels[]?.name] | index("security-override")' "$PAYLOAD" >/dev/null 2>&1; then
    OVERRIDE=true
  fi
elif [ -s "$PAYLOAD" ]; then
  BASE="$(jq -r '.before // empty' "$PAYLOAD")"
fi

if ! git cat-file -e "${BASE}^{commit}" 2>/dev/null; then
  BASE="$(git rev-parse "${HEAD}^" 2>/dev/null || git rev-list --max-parents=0 "$HEAD" | tail -1)"
  MODE=report
fi

hit_file=(); hit_kind=(); hit_evi=()

clip() { printf '%s' "$1" | tr -d '\r' | cut -c1-160; }

# A finding that is obviously a placeholder is not a finding. Documentation in
# this repository shows `export CARGO_REGISTRY_TOKEN=<paste your token>` and
# that has to stay legal.
PLACEHOLDER='(<[^>]{0,40}>|\$\{|\$\(|YOUR[_-]|REPLACE|EXAMPLE|PLACEHOLDER|xxxx|XXXX|\.\.\.|redacted|\*{4,})'

echo "Secret scan"
echo "${DIM}range  $BASE..$HEAD${OFF}"
echo "${DIM}mode   $MODE   override $OVERRIDE${OFF}"
echo

if [ "$BASE" = "$HEAD" ]; then
  echo "No change between the two commits; nothing to inspect."
  exit 0
fi

ADDED="$(mktemp)"
trap 'rm -f "$ADDED"' EXIT
git diff --unified=0 "$BASE" "$HEAD" | awk '
  /^\+\+\+[[:space:]]/ { f = substr($2, 3); next }
  /^\+/                { print f "\t" substr($0, 2) }
' > "$ADDED"

scan() { # scan KIND REGEX
  local kind="$1" re="$2" f line
  while IFS=$'\t' read -r f line; do
    [ -z "${f-}" ] && continue
    printf '%s' "$line" | grep -qE -e "$PLACEHOLDER" && continue
    hit_file+=("$f"); hit_kind+=("$kind"); hit_evi+=("$(clip "$line")")
  done < <(grep -E -e "$re" "$ADDED" || true)
}

scan 'GitHub token'                '\bgh[pousr]_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{59,}\b'
scan 'AWS access key id'           '\b(AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b'
scan 'AWS secret access key'       '\baws_secret_access_key\b[^A-Za-z0-9]{0,4}[A-Za-z0-9/+=]{40}'
scan 'Google API key'              '\bAIza[0-9A-Za-z_-]{35}\b'
scan 'Slack token'                 '\bxox[baprs]-[A-Za-z0-9-]{10,}\b'
scan 'OpenAI or Anthropic key'     '\bsk-(ant-)?[A-Za-z0-9_-]{20,}\b'
scan 'Stripe live key'             '\b(sk|rk)_live_[A-Za-z0-9]{20,}\b'
scan 'GitLab personal token'       '\bglpat-[A-Za-z0-9_-]{20,}\b'
scan 'npm token'                   '\bnpm_[A-Za-z0-9]{36}\b'
scan 'Hugging Face token'          '\bhf_[A-Za-z0-9]{34}\b'
scan 'DigitalOcean token'          '\bdop_v1_[a-f0-9]{64}\b'
scan 'Shopify token'               '\bshp(at|ca|ss)_[A-Za-z0-9]{20,}\b'
scan 'Twilio API key'              '\bSK[0-9a-fA-F]{32}\b'
scan 'JWT'                         '\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b'
scan 'private key block'           '-----BEGIN[ A-Z]*PRIVATE KEY'
scan 'credentials in a URL'        '://[^/@[:space:]]+:[^/@[:space:]]+@'
scan 'secret assigned to a literal' '(api[_-]?key|apikey|secret|token|passw(or)?d|credential|private[_-]?key)["'"'"']?[[:space:]]*[:=][[:space:]]*["'"'"'][A-Za-z0-9/+=_.~-]{16,}["'"'"']'
scan 'crates.io publish token'      'CARGO_REGISTRY_TOKEN[[:space:]]*=[[:space:]]*[^[:space:]<$]'

if [ "${#hit_file[@]}" -eq 0 ]; then
  echo "${BOLD}No credential shapes in the added lines.${OFF}"
  exit 0
fi

echo "${RED}${BOLD}Found (${#hit_file[@]})${OFF}"
for i in "${!hit_file[@]}"; do
  printf '  %sx%s %s — %s\n' "$RED" "$OFF" "${hit_file[$i]}" "${hit_kind[$i]}"
  printf '     %s> %s%s\n' "$DIM" "${hit_evi[$i]}" "$OFF"
done

if [ "$MODE" = report ]; then
  echo
  echo "Report mode: push to the default branch, findings printed rather than enforced."
  exit 0
fi
if [ "$OVERRIDE" = true ]; then
  echo
  echo "security-override is set on this pull request, so these findings are recorded but not enforced."
  exit 0
fi

cat <<'TEXT'

If one of these is a real credential, it is already exposed: the pull request
head is public the moment it is pushed. Rotate it at the provider, then remove
it from the branch. Do not force-push over the commit and assume it is gone —
the pull request keeps a record of every head SHA it ever had.

If it is a test fixture, an obviously fake value, or a placeholder the filter
above did not recognise, say so on the pull request. A maintainer applies the
security-override label. Do not obfuscate the string to get past the pattern.
TEXT
exit 1
