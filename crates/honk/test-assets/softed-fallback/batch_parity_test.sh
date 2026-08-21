#!/usr/bin/env bash
set -euo pipefail

abspath() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}

honk="$(abspath "$1")"
hoonc="$(abspath "$2")"
prelude="$(abspath "$3")"
softed_entry="$(abspath "$4")"
plain_entry="$(abspath "$5")"
deps="$(dirname "$(dirname "$softed_entry")")"
work="${TEST_TMPDIR:?Bazel must provide TEST_TMPDIR}/softed-fallback-batch"

mkdir -p \
    "$work/reference-softed/.local/share" \
    "$work/reference-softed/.config" \
    "$work/reference-plain/.local/share" \
    "$work/reference-plain/.config" \
    "$work/candidate"
cd "$work"

env -i \
    HOME="$work/reference-softed" \
    XDG_DATA_HOME="$work/reference-softed/.local/share" \
    XDG_CONFIG_HOME="$work/reference-softed/.config" \
    TMPDIR="$work" \
    RUST_LOG=warn \
    "$hoonc" --new --arbitrary --output reference-softed.jam \
    "$softed_entry" "$deps" >/dev/null

env -i \
    HOME="$work/reference-plain" \
    XDG_DATA_HOME="$work/reference-plain/.local/share" \
    XDG_CONFIG_HOME="$work/reference-plain/.config" \
    TMPDIR="$work" \
    RUST_LOG=warn \
    "$hoonc" --new --arbitrary --output reference-plain.jam \
    "$plain_entry" "$deps" >/dev/null

printf 'candidate-softed.jam\t%s\tarbitrary\ncandidate-plain.jam\t%s\tarbitrary\n' \
    "$softed_entry" "$plain_entry" >batch.tsv

env -i \
    HOME="$work/candidate" \
    TMPDIR="$work" \
    RUST_LOG=warn \
    "$honk" --new --batch-manifest batch.tsv --prelude "$prelude" "$deps" >/dev/null

test -s candidate-softed.jam
test -s candidate-plain.jam
cmp reference-softed.jam candidate-softed.jam
cmp reference-plain.jam candidate-plain.jam
