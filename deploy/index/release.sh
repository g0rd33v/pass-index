#!/bin/sh
# One command to ship, and one point to come back to.
#
#   ./deploy/index/release.sh "what changed"
#
# It commits the working tree, tags the commit, keeps the image that is live
# right now as pass-index:previous, builds the new one, and swaps it in. Every
# release leaves exactly one thing to undo, which rollback.sh does.
set -eu
MSG="${1:-release}"
HOST=pass-hub
SRC=/srv/pass-index/src
COMPOSE=$SRC/deploy/index/docker-compose.yml
STAMP=$(date -u +%Y%m%d-%H%M%S)

cd "$(dirname "$0")/../.."
# The release before this one, captured before we tag the new one. If the new
# build fails its health check, both the image AND the source are put back to
# this — the source too, because the crates/compose/collect-daily.sh were
# already rsynced to the host, and reverting only the image left the nightly
# running the failed release's script against the previous binary.
PREV=$(git tag -l 'release-[0-9]*-[0-9]*' | sort | tail -1)

git add -A
git commit -q -m "$MSG" || echo "nothing new to commit"
git tag -f "release-$STAMP" >/dev/null
echo "tagged release-$STAMP  ($(git rev-parse --short HEAD))"

ship() {  # ship <label>  — rsync the working tree to the host
  rsync -a --delete crates/ "$HOST:$SRC/crates/"
  rsync -a deploy/index/ "$HOST:$SRC/deploy/index/"
  rsync -a deploy/index/collect-daily.sh "$HOST:/srv/pass-index/collect-daily.sh"
  rsync -a Cargo.toml "$HOST:$SRC/Cargo.toml"
}
ship

if ssh "$HOST" "
  set -eu
  # The fallback is the image the container is RUNNING, not whatever :latest
  # points at — those differ the moment anything else builds :latest.
  RUNNING=\$(docker inspect pass-index --format '{{.Image}}' 2>/dev/null || true)
  [ -n \"\$RUNNING\" ] && docker image tag \"\$RUNNING\" pass-index:previous
  cd $SRC
  docker compose -f $COMPOSE build pass-index >/dev/null
  docker image tag pass-index:latest pass-index:$STAMP
  docker compose -f $COMPOSE up -d pass-index >/dev/null
  sleep 4
  if ! curl -sf -o /dev/null http://127.0.0.1:8185/index; then
    echo '!! the new build does not serve; putting the previous image back'
    if docker image inspect pass-index:previous >/dev/null 2>&1; then
      docker image tag pass-index:previous pass-index:latest
      docker compose -f $COMPOSE up -d pass-index >/dev/null
      sleep 3
      curl -sf -o /dev/null http://127.0.0.1:8185/index \
        && echo 'the previous image is live again' \
        || echo '!! the previous image does not serve either — intervene by hand'
    else
      echo '!! no previous image to fall back to — intervene by hand'
    fi
    exit 1
  fi
  echo 'live: $STAMP'
"; then
  echo "rollback with: ./deploy/index/rollback.sh"
else
  # The image is back to previous; put the source back to match, or the
  # nightly runs the failed release's collect-daily.sh against it.
  if [ -n "$PREV" ]; then
    echo "restoring source to $PREV to match the reverted image"
    git checkout -q "$PREV" -- crates deploy Cargo.toml && ship
  fi
  echo "release failed and was reverted; working tree still holds the attempt (tag release-$STAMP)"
  exit 1
fi
