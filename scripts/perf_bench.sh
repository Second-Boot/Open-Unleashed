#!/usr/bin/env bash
# TEST GLUE (not part of the game): the renderer's fixed-scene benchmark (`--perf-bench`, `app/scripted.rs` run_perf_bench).
#
# Each run starts one race still (`<track> 993 --opponents 3 --screenshot`, the chase camera at the start, the scene frozen) and
# draws its frame again and again before the still: warm-up frames, then N measured ones, the device waited for after each.
# The game logs one `PERF` line (median / p95 GPU busy ms from the stage marks, CPU encode ms, finish / submit ms, the
# NFS_GPU_TIME-style wall ms) and one `PERFSTAGE` line (each stage's median GPU ms); this script prints them per run with
# the load average at the start of the run, and runs ONE game process at a time.
#
# Variants (A/B) are interleaved run by run so a changing machine load hits every side alike:
#   -V 'label=<binary>[;ENV=value;ENV=value...]'   (repeatable; default: one variant "cur" = target/release/open-unleashed)
#
# usage: scripts/perf_bench.sh -t <pristine save dir> [-n frames] [-s WxH] [-r runs] [-o out dir] [-V variant]... [scene...]
#   scenes: <track>[:label] (default: alps:day industrial:night monaco1:night - the tracks' own time of day)
#   every run gets a fresh copy of the save dir (-t, required), NFS_NO_FOCUS=1, no sound, NFS_HD_TEX_DIR=<a missing dir>
#   unless it is set, and a 300 s limit. NFS_ENH and other variables of the caller's environment pass through.
set -euo pipefail

frames=300 size=2560x1600 runs=3 out="" template=""
variants=()
while getopts "n:s:r:o:t:V:" opt; do
    case $opt in
        n) frames=$OPTARG ;;
        s) size=$OPTARG ;;
        r) runs=$OPTARG ;;
        o) out=$OPTARG ;;
        t) template=$OPTARG ;;
        V) variants+=("$OPTARG") ;;
        *) sed -n '2,17p' "$0"; exit 2 ;;
    esac
done
shift $((OPTIND - 1))
[[ -n $template && -d $template ]] || { echo "perf_bench: -t <pristine save dir> is required" >&2; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
[[ ${#variants[@]} -gt 0 ]] || variants=("cur=$root/target/release/open-unleashed")
scenes=("$@")
[[ ${#scenes[@]} -gt 0 ]] || scenes=(alps:day industrial:night monaco1:night)
[[ -n $out ]] || out=$(mktemp -d "${TMPDIR:-/tmp}/perf_bench.XXXXXX")
mkdir -p "$out"
hd=${NFS_HD_TEX_DIR:-$out/no-hd-pack}

load() { uptime | sed 's/.*load average[s]*: *//' | awk -F'[ ,]+' '{print $1}'; }

for ((k = 1; k <= runs; k++)); do
    for sc in "${scenes[@]}"; do
        track=${sc%%:*}
        for v in "${variants[@]}"; do
            label=${v%%=*}
            spec=${v#*=}
            bin=${spec%%;*}
            envs=()
            [[ $spec == *";"* ]] && IFS=';' read -r -a envs <<<"${spec#*;}"
            tag="${track}_${label}_$k"
            save="$out/save_$tag"
            rm -rf "$save"
            cp -R "$template" "$save"
            l=$(load)
            env NFS5_SAVE_DIR="$save" NFS_NO_FOCUS=1 NFS_HD_TEX_DIR="$hd" NFS_SHOT_SIZE="$size" ${envs[@]+"${envs[@]}"} \
                perl -e 'alarm 300; exec @ARGV' "$bin" "$track" 993 --opponents 3 --screenshot "$out/$tag.png" --perf-bench "$frames" \
                >"$out/$tag.log" 2>&1 || true
            rm -rf "$save"
            perf=$(grep -o 'PERF size=.*' "$out/$tag.log" | head -1 || true)
            echo "$sc $label run=$k load=$l ${perf:-PERF missing (see $out/$tag.log)}"
            grep -o 'PERFSTAGE .*' "$out/$tag.log" | head -1 | sed "s/^/  $label /" || true
        done
    done
done
echo "logs and stills: $out"
