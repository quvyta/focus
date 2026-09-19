#!/bin/sh
# Draws the README's moving picture again: qfocus.gif and qfocus.mp4 in this folder.
#
# The picture picks a focus on the Today page, starts the counter, takes a break, stops it into a
# record, then walks the day and week charts and opens one session's spans on the Records page.
# The frames come from the test harness with real key presses and a hand-moved clock (the ignored
# `readme_gif` test in tests/readme_shots.rs) over a made-up week in a temporary folder, so the
# picture shows no one's data and is the same on every machine; ffmpeg joins them, each held as
# long as the test says. Needs ffmpeg with libx264.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
frames="$root/target/readme-gif"

export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"
cd "$root"
# Release, because drawing some thirty large PNGs is slow without optimisation.
cargo test --release --test readme_shots readme_gif -- --ignored

list="$frames/frames.txt"
palette="$frames/palette.png"
# The length the frames add up to; the frame rate filter would hold the last frame longer.
length=$(awk '$1 == "duration" { total += $2 } END { print total }' "$list")

# The frames are drawn at twice the size for sharpness; three quarters keeps the files small while
# every cell stays readable. The frames have square corners filled with the terminal's ground: a
# GIF with transparent pixels loses ffmpeg's frame-difference cropping and grows many times, and
# dropping the alpha of rounded corners would turn them black. Without dithering the flat tones
# of the interface stay flat, which keeps both the text crisp and the file small.
scale="fps=10,scale=trunc(iw*3/8)*2:-2:flags=lanczos,format=rgb24"

ffmpeg -loglevel error -y -f concat -i "$list" -t "$length" \
    -vf "$scale,format=yuv420p" \
    -c:v libx264 -preset veryslow -tune animation -crf 20 -movflags +faststart -an \
    "$here/qfocus.mp4"

ffmpeg -loglevel error -y -f concat -i "$list" \
    -vf "$scale,palettegen=max_colors=256:stats_mode=full:reserve_transparent=0" \
    "$palette"
ffmpeg -loglevel error -y -f concat -i "$list" -i "$palette" -t "$length" \
    -lavfi "$scale[x];[x][1:v]paletteuse=dither=none:diff_mode=rectangle" \
    -loop 0 \
    "$here/qfocus.gif"

ls -l "$here/qfocus.gif" "$here/qfocus.mp4"
