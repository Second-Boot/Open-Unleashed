#!/bin/sh
# Start the game from this directory (the game data is in ./game).
cd "$(dirname "$0")"
exec ./PorscheUnleashed "$@"
