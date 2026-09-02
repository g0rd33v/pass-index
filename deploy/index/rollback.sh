#!/bin/sh
# The button. Puts back the site that was live before the last release.
#
#   ./deploy/index/rollback.sh            # back one release
#   ./deploy/index/rollback.sh release-20260827-181500   # back to a named one
set -eu
HOST=pass-hub
SRC=/srv/pass-index/src
COMPOSE=$SRC/deploy/index/docker-compose.yml
cd "$(dirname "$0")/../.."

TARGET="${1:-}"
if [ -z "$TARGET" ]; then
  # Timestamped release tags only. The repo also carries named release-<word>
  # tags, which sort after every timestamp and once sent this rollback to a
  # four-day-old build.
  TARGET=$(git tag -l 'release-[0-9]*-[0-9]*' | sort | tail -2 | head -1)
fi
[ -n "$TARGET" ] || { echo "no release tag to go back to"; exit 1; }

echo "returning to $TARGET"
git checkout -q "$TARGET" -- crates deploy Cargo.toml
# The host must run what the checkout says, or the nightly keeps executing
# the rolled-back release's script against the restored binaries.
rsync -a --delete crates/ "$HOST:$SRC/crates/"
rsync -a deploy/index/ "$HOST:$SRC/deploy/index/"
rsync -a deploy/index/collect-daily.sh "$HOST:/srv/pass-index/collect-daily.sh"
rsync -a Cargo.toml "$HOST:$SRC/Cargo.toml"
ssh "$HOST" "
  set -eu
  cd $SRC
  # Run the image that IS the target release, not :previous. release.sh tags
  # every build pass-index:<stamp>, so a named rollback runs that exact build.
  # :previous is only ever one release back, so using it made a rollback to
  # an older named release serve the wrong binary while printing success.
  TAG='pass-index:${TARGET#release-}'
  if docker image inspect \"\$TAG\" >/dev/null 2>&1; then
    docker image tag \"\$TAG\" pass-index:latest
  else
    echo \"no image \$TAG on the host; rebuilding from the restored source\"
    docker compose -f $COMPOSE build pass-index >/dev/null
  fi
  docker compose -f $COMPOSE up -d --force-recreate pass-index >/dev/null
  sleep 4
  curl -sf -o /dev/null http://127.0.0.1:8185/index \
    && echo 'the restored build is live' \
    || { echo '!! the restored build does not serve'; exit 1; }
"
echo "source is back at $TARGET. Ship it again with: ./deploy/index/release.sh \"...\""
