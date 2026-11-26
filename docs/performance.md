# Frame-rate switches (`VICTORY_*`)

Most of the frame-rate work of the fork sits behind environment variables. Unless the table says
otherwise a switch is **off when unset**: a plain `victory run` behaves as before. The recommended
set is in [`scripts/series-s-preset.ps1`](../scripts/series-s-preset.ps1).

Measured on an i5-14600KF / RTX 5070 12 GB / 32 GB, `run --console series-s`, 720p internal size,
same scene, window 300-360 s of a 367 s run: `main` 5.07 fps; the preset below 13.6-14.2 fps
(steady state after 240 s: sd 0.3 fps, no second above 200 ms per frame, no RAM/VRAM growth, no
device removal). The first 2-3 minutes in the world (loading, pipeline compilation) stay at 2-12 fps.
The picture is within the run-to-run variance of `main` (the scene itself is not deterministic).

## The preset

| Variable | Value | Default | What it does | Measured |
|---|---|---|---|---|
| `VICTORY_RELAY` | `2` | **on, stage 2** (`0` off, `1` stage 1) | Relay: the title's submissions go to the card ahead of its own waits. Already the default of `main`. | basis of everything below |
| `VICTORY_QUEUE2` | `1` | `0` | Fetches of what shaders wrote go to a copy queue (bit 2: uploads too). | with the relay |
| `VICTORY_ASYNC_SUBMIT` | `1` | off | Submission on a thread of its own (needs `LATE_WAIT`). | with the relay |
| `VICTORY_INDIRECT_WINDOW` | `1` | off | Indirect draws are clamped in windows instead of one by one. | with `QUEUE2` + `ASYNC_SUBMIT`: 5.07 -> 7.4 fps |
| `VICTORY_BACK_OWN_VIEW` | `1` | off | Copy-back through views of our own. | 5.95 vs 5.07 fps on a former base |
| `VICTORY_LATE_WAIT` | `3` | `3` while the relay is on (otherwise `0`; `VICTORY_GPU_TIMING` forces `0`) | Title waits are met when the card has really run the work. Assumes the title does not rewrite memory read by a flight in the air (as on the console). | |
| `VICTORY_MERGE_ON_CARD` | `1` | **on** (`0` off, `alt` 20 s on / 20 s off) | Title bytes changed under in-flight shader writes are merged on the card with a mask instead of settling the flights (upstream #37). `=0` is the former behaviour. | +13 % (11.68 -> 13.2 fps), settles 1.7 -> 0 per frame |
| `VICTORY_CARD_SYNC_BATCH` | `0` | **on** in `exp/card-batch` (`0` = former copy-by-copy order) | The list that brings card copies up to date records every copy, then one state-change call, then every hash pass, instead of copy -> barrier -> hash per copy (fork PR #1227, KyleS98). Merge-on-card stretches stay per copy. | author, RTX 3090 Ti, `CARD_BUFFERS_MIN_KB=64`: card time of the lists 29.4 -> 5.6 ms, 11.7 -> 13.8 fps; not measured by us |
| `VICTORY_CARD_BUFFERS_MIN_KB` | `64` (recommended for measuring) | main's behaviour (1 MiB: `VICTORY_CARD_BUFFERS_MIN_MB`=1); the preset is unchanged | Smallest title buffer whose copy lives in the card's memory, in KiB. 64 makes the send-and-hash list long (~300 copies a frame), which is what `CARD_SYNC_BATCH` shortens. | measure with and without `CARD_SYNC_BATCH` |
| `VICTORY_PREP_PAR_COPY` | `1` | off | The fills of a submission are copied on the `victory-wide` threads. | prep 56 -> 49 ms; fps within noise alone |
| `VICTORY_PREP_PAR_HEAP` | `1` | off | The heap buffer slots are looked at on the `victory-wide` threads. | idem |
| `VICTORY_JOIN_PARTS` | `1` | off (`alt` 20 s / 20 s) | Parts that follow each other in the queues go to the host as one submission. | +5 % on 3 runs (13.08 -> 13.78 fps) |

## Other switches added by the merge (all off unless noted, not in the preset)

| Variable | Default | Note |
|---|---|---|
| `VICTORY_INDIRECT_RING` / `VICTORY_WINDOW_JUMP` / `VICTORY_INDIRECT_GATHER` / `VICTORY_HOIST_CLAMPS` | off | Variants of the indirect-draw windows. `HOIST_CLAMPS` is ignored when `INDIRECT_WINDOW=1`. |
| `VICTORY_HEAP_WORKER` | off | Textures picked by index uploaded on a worker thread. |
| `VICTORY_WIDE_TEXTURES` | off | A submission's texture checks run several at a time. |
| `VICTORY_COPY_THREADS` | `1` | Fills of upload memory read on several threads. |
| `VICTORY_COPY_ONCE` | on (`0` = former) | `copy_mapped` reads the source once. |
| `VICTORY_STALE_TARGETED` | off | A stale-range settle waits only for the flights that name the ranges. |
| `VICTORY_DSTORAGE_PHYSICAL` / `VICTORY_DSTORAGE_WORKERS` | off | DirectStorage worker pool sized to physical cores. |
| `VICTORY_REAPER` | on (`0` off) | A thread drops retired D3D12 objects off the submit paths. |
| `VICTORY_D3D12_GBV=unguarded` | - | Lighter GPU-based validation (diagnostics). |

## Diagnostic and tuning variables documented where they are read

About sixty further `VICTORY_*` names come with the ports (profiler `VICTORY_PROFILE*`, frame log
`VICTORY_FRAME_LOG`, `VICTORY_CARD_METER*`, `VICTORY_DRIVER_WATCH`, join limits `VICTORY_JOIN_MOST` /
`VICTORY_JOIN_LISTS`, merge limits `VICTORY_MERGE_RUNS` / `VICTORY_MERGE_GAP`, `VICTORY_QUEUE2_LANDING_MB`,
`VICTORY_UPLOAD_POOL`, `VICTORY_STAGING_POOL`, `VICTORY_SPREAD*`, `VICTORY_FILL_WAIT`, ...). None is needed
for the preset; each has its doc comment at the place it is read (`git grep -n '"VICTORY_NAME"' crates`).
Measured and found without effect, do not set: `VICTORY_MERGE_RUNS=512`; `VICTORY_CARD_HASH_WIDE=0` is slower (12.6 fps).

## Defaults that changed with the merge

These come from ports of upstream and `fxceless/victory-fork` work (authors kept in the commits,
`cherry-pick -x`) and apply without any variable: XXH3 change detection of texture memory,
wide snapshots/compares/refreshes, the host device on the fastest adapter, descriptor pages
skipped when unchanged, ~45 extra shader lowerings (`lower.rs`), and `VICTORY_MERGE_ON_CARD` on.

## Measuring

`VICTORY_*=alt` switches run 20 s with and 20 s without in one run and say each change in the log.
Run `main` and a candidate on the same scene, three runs each, before moving a switch into the
preset: run-to-run noise is about 3 %.
