#!/usr/bin/env bash
# Publishes the contents of ci-out/ as a single commit on the branch
# ci-results/<git branch>--<name> (force-pushed, no history), so the logs and
# output files of a run can be read with plain git. The git branch is part of
# the name so that runs on different branches do not overwrite each other.
set -euo pipefail

name="$1"
status="$2"

mkdir -p ci-out
{
  echo "job:    $name"
  echo "status: $status"
  echo "commit: $GITHUB_SHA"
  echo "run:    $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
} > ci-out/STATUS.txt

cd ci-out
git init -q
git config user.name "ci"
git config user.email "ci@users.noreply.github.com"
git config core.autocrlf false
git add -A
git commit -q -m "$name $status ${GITHUB_SHA:0:7}"
branch="${GITHUB_HEAD_REF:-$GITHUB_REF_NAME}"
branch="${branch//\//-}"
git push -q -f "https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" "HEAD:refs/heads/ci-results/${branch}--${name}"
