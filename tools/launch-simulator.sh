#!/usr/bin/env bash
# Launch the synthetic FH6 telemetry sender (random-ish data on the default port 1337).
# Pass through any sim.py flags, e.g. ./launch-simulator.sh --scenario accel
exec python3 "$(dirname "$0")/sim.py" "$@"
