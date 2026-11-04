#!/usr/bin/env bash
# The results of a CI job, readable with plain git.
#
#   publish-results.sh status <job> <status>
#       In the job itself: writes ci-out/STATUS.txt (job, status, commit,
#       run). The job then hands ci-out over as an artifact.
#   publish-results.sh push <job> <dir>
#       In the publishing job, the only one with a token that can write:
#       commits <dir> alone (force-pushed, no history) to the branch
#       ci-results/<git branch>--<job>. The git branch is part of the name so
#       that runs on different branches do not overwrite each other. Nothing
#       in <dir> is run: it is committed as it is.
set -euo pipefail

action="$1"
name="$2"

case "$action" in
  status)
    status="$3"
    mkdir -p ci-out
    {
      echo "job:    $name"
      echo "status: $status"
      echo "commit: $GITHUB_SHA"
      echo "run:    $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
    } > ci-out/STATUS.txt
    ;;
  push)
    dir="$3"
    cd "$dir"
    git init -q
    git config user.name "ci"
    git config user.email "ci@users.noreply.github.com"
    git config core.autocrlf false
    # Hooks of a folder made from artifacts are never run.
    git config core.hooksPath /dev/null
    git add -A
    status="$(sed -n 's/^status: //p' STATUS.txt 2>/dev/null || true)"
    git commit -q -m "$name ${status:-unknown} ${GITHUB_SHA:0:7}"
    branch="${GITHUB_HEAD_REF:-$GITHUB_REF_NAME}"
    branch="${branch//\//-}"
    git push -q -f "https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" "HEAD:refs/heads/ci-results/${branch}--${name}"
    ;;
  *)
    echo "usage: publish-results.sh status <job> <status> | push <job> <dir>" >&2
    exit 2
    ;;
esac
