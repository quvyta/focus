#!/bin/sh
# Draws the README's moving picture again: qfocus.gif and qfocus.mp4 in this folder.
#
# The picture picks a focus on the Today page, starts the counter, takes a break, stops it into a
# record, then walks the day and week charts and opens one session's spans on the Records page.
# The ignored `readme_gif` test in tests/readme_shots.rs records it with real key presses and a
# hand-moved clock over a made-up week in a temporary folder, so the picture shows no one's data
# and is the same on every machine, then encodes it with ffmpeg. Needs ffmpeg with libx264.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"
cd "$root"
# Release, because drawing some hundreds of large PNGs is slow without optimisation.
cargo test --release --test readme_shots readme_gif -- --ignored
ls -l docs/screenshots/qfocus.gif docs/screenshots/qfocus.mp4
