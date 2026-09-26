#!/usr/bin/env bash
# Build the Surfer web app and serve it together with the embed playground page.
#
# Usage: ./serve.sh [--release] [--no-build] [port]
#
# Layout served from this directory:
#   /            -> index.html (the host page embedding Surfer in an iframe)
#   /surfer/     -> the trunk-built Surfer web app
#   /examples/   -> symlink to the repository's example waveforms
set -euo pipefail

cd "$(dirname "$0")"
PLAYGROUND_DIR="$(pwd)"

RELEASE=""
BUILD=1
PORT=8000
for arg in "$@"; do
    case "$arg" in
        --release) RELEASE="--release" ;;
        --no-build) BUILD=0 ;;
        *) PORT="$arg" ;;
    esac
done

if [ "$BUILD" = 1 ]; then
    (
        cd ../surfer
        RUSTFLAGS="--cfg=web_sys_unstable_apis" trunk build index.html $RELEASE \
            --public-url /surfer/ \
            --dist "$PLAYGROUND_DIR/surfer"
    )
fi

ln -sfn ../examples examples

echo "Serving embed playground at http://localhost:$PORT/"
python3 -m http.server "$PORT" --bind 127.0.0.1
