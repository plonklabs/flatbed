#!/usr/bin/env bash
# Slot-scoped NATS broker for the broker-backed integration tests.
#
# The tests use fixed stream/bucket names, so two checkouts sharing one
# broker clobber each other's streams. Isolation is per broker, not per
# name: each worktree gets its own container on its own host port, and the
# tests point at it through NATS_URL.
#
#   slot N (cwd under worktrees/flatbedN) -> container flatbed-nats-N, port 4222+N
#   main checkout                         -> container flatbed-nats,   port 4222
#
# Usage:
#   scripts/nats-broker.sh up      # start (or restart) this slot's broker
#   scripts/nats-broker.sh down    # stop and remove this slot's broker
#   scripts/nats-broker.sh url     # print the NATS_URL for this slot
#
# Run the tests against it with:
#   NATS_URL=$(scripts/nats-broker.sh url) \
#     cargo test -p flatbed --features nats,openapi \
#       --test nats_broker --test nats_route_broker --test nats_request_broker -- --ignored
set -euo pipefail

slot=""
case "$PWD" in
  */worktrees/flatbed[0-9]*) slot="${PWD##*/worktrees/flatbed}"; slot="${slot%%/*}" ;;
esac

if [ -n "$slot" ]; then
    name="flatbed-nats-$slot"
    port=$((4222 + slot))
else
    name="flatbed-nats"
    port=4222
fi

case "${1:-}" in
  up)
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -p "$port:4222" nats:2.10-alpine -js >/dev/null
    echo "localhost:$port"
    ;;
  down)
    docker rm -f "$name" >/dev/null 2>&1 || true
    ;;
  url)
    echo "localhost:$port"
    ;;
  *)
    echo "usage: $0 up|down|url" >&2
    exit 2
    ;;
esac
