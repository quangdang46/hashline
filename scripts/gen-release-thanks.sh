#!/usr/bin/env bash
# gen-release-thanks.sh — generate the "Thanks:" credit line for a
# CHANGELOG release section.
#
# Scans commits in a range (default: last semver tag..HEAD) for:
#   - pull-request references (#NNN in subjects/bodies, "Merge pull request #NNN")
#   - GitHub commit authors (resolved via the API)
# then prints a ready-to-paste line in the established style:
#
#   Thanks: @cleg (Pavlo Dmytriiev) for #127.
#
# Excludes: the repository owner, the gh-authenticated user (the maintainer
# cutting the release), bots, and PRs already credited in CHANGELOG.md.
#
# Usage:
#   scripts/gen-release-thanks.sh [range]     # e.g. v0.9.22..HEAD
#
# Requires: git, gh (authenticated), bash. Read-only; safe to re-run.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

RANGE="${1:-}"
if [ -z "$RANGE" ]; then
  LAST_TAG="$(git tag --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)"
  if [ -z "$LAST_TAG" ]; then
    echo "error: no semver tag found; pass a range explicitly" >&2
    exit 1
  fi
  RANGE="${LAST_TAG}..HEAD"
fi

command -v gh >/dev/null || { echo "error: gh CLI required" >&2; exit 1; }

REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null)" || {
  echo "error: cannot resolve repo (gh auth?)" >&2; exit 1;
}
OWNER="${REPO%%/*}"
SELF="$(gh api user --jq .login 2>/dev/null || echo '')"

# --- collect PR numbers referenced by commits in range ---
PR_NUMS="$(git log "$RANGE" --format='%s%n%b' | grep -oE '#[0-9]+' | tr -d '#' | sort -un || true)"

# --- PRs already credited in CHANGELOG Thanks lines ---
CREDITED_PRS="$(grep -iE 'Thanks:' CHANGELOG.md 2>/dev/null | grep -oE '#[0-9]+' | tr -d '#' | sort -u || true)"

is_credited_pr() {
  [ -n "$CREDITED_PRS" ] && echo "$CREDITED_PRS" | grep -qx "$1"
}

declare -A SEEN_LOGIN   # login -> display name (may be backfilled)
declare -A LOGIN_PRS    # login -> " pr pr ..."

add_credit() { # $1=login $2=name $3=pr-or-empty
  local login="$1" name="$2" pr="$3"
  [ -z "$login" ] && return 0
  case "$login" in
    "$OWNER"|"$SELF"|web-flow|github-actions*|*"[bot]") return 0 ;;
  esac
  if [ -n "$pr" ] && is_credited_pr "$pr"; then return 0; fi
  if [ -z "${SEEN_LOGIN[$login]+x}" ]; then
    SEEN_LOGIN[$login]="$name"
  elif [ -z "${SEEN_LOGIN[$login]}" ] && [ -n "$name" ]; then
    SEEN_LOGIN[$login]="$name"   # backfill display name
  fi
  if [ -n "$pr" ]; then
    LOGIN_PRS[$login]="${LOGIN_PRS[$login]:-} $pr"
  fi
  return 0
}

# --- resolve PR authors ---
for pr in $PR_NUMS; do
  is_credited_pr "$pr" && continue
  info="$(gh api "repos/${REPO}/pulls/${pr}" --jq '[.user.login, (.user.name // "")] | @tsv' 2>/dev/null || true)"
  [ -z "$info" ] && continue
  login="${info%%	*}"
  name="${info#*	}"
  [ "$name" = "$login" ] && name=""
  add_credit "$login" "$name" "$pr"
done

# --- resolve commit authors (covers direct pushes / non-PR commits) ---
while IFS= read -r sha; do
  [ -z "$sha" ] && continue
  info="$(gh api "repos/${REPO}/commits/${sha}" \
    --jq '[.author.login // "", .commit.author.name // ""] | @tsv' 2>/dev/null || true)"
  [ -z "$info" ] && continue
  login="${info%%	*}"
  name="${info#*	}"
  add_credit "$login" "$name" ""
done < <(git log "$RANGE" --format='%H')

# --- emit ---
if [ "${#SEEN_LOGIN[@]}" -eq 0 ]; then
  echo "# no external contributors in range $RANGE"
  exit 0
fi

out="Thanks:"
for login in "${!SEEN_LOGIN[@]}"; do
  name="${SEEN_LOGIN[$login]}"
  prs="$(echo "${LOGIN_PRS[$login]:-}" | tr ' ' '\n' | grep -v '^$' | sort -un | tr '\n' ' ' | sed 's/ $//' || true)"
  entry="@${login}"
  if [ -n "$name" ] && [ "$name" != "$login" ]; then
    entry="${entry} (${name})"
  fi
  if [ -n "$prs" ]; then
    prlist="$(echo "$prs" | tr ' ' '\n' | sed 's/^/#/' | paste -sd, -)"
    entry="${entry} for ${prlist}"
  fi
  out="${out} ${entry}."
done
echo "$out"
