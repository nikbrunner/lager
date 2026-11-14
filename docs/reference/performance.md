# Inventory performance

This page records the [PRD §6.2](../prd/1.0.0.md#62-responsiveness-and-verification) measurements: process start to first usable frame, source completion, and input latency while a refresh or a slow provider is loading, against fixtures of 100 and 1,000 repositories. Each p95 is nearest-rank. A target that misses is recorded as measured.

## Results

Measured 2026-11-14 on an Apple M5 Pro (18 cores, 48 GB), macOS 26.4.1 (25E253), Python 3.14.7, release build of the working tree at `c47eb9f` plus the linear inventory rebuild. The machine was in ordinary use: load average 5.3 at the end of the run. One run of the script covers every row, delayed-provider rows included.

| Condition | Samples | 100 repos p95 | 1,000 repos p95 | Target | Result |
| --- | --- | --- | --- | --- | --- |
| First usable frame, cold | 20 runs | 12.0 ms | 28.0 ms | ≤250 ms | pass |
| First usable frame, warm | 20 runs | 10.5 ms | 27.5 ms | ≤250 ms | pass |
| Source completion, cold | 20 runs | 855.7 ms | 7,987.3 ms | none | recorded |
| Source completion, warm | 20 runs | 853.6 ms | 7,957.4 ms | none | recorded |
| Search key during local refresh | 100 keys | 0.9 ms | 16.9 ms | ≤50 ms | pass |
| Search key during delayed provider | 100 keys | 0.6 ms | 4.7 ms | ≤50 ms | pass |
| Row navigation during local refresh | 100 keys | 2.4 ms | 22.7 ms | ≤50 ms | pass |
| Row navigation during delayed provider | 100 keys | 0.6 ms | 3.6 ms | ≤50 ms | pass |
| `lager ls`, whole process | 20 runs | 827.6 ms | 8,913.7 ms | none | recorded |

Medians at 1,000 repositories: 5.4 ms for search and 14.6 ms for navigation during a refresh, 3.9 ms and 3.5 ms while only the provider loads. Input still slows while local observations arrive, not while a provider loads. The slowest cold first frame at 100 repositories was 377.9 ms, a single outlier among values near 10 ms; the nearest-rank p95 is 12.0 ms.

`lager ls` exits `1` on both fixtures because the unreadable checkouts are scan problems. It finishes the scan and every Git observation before printing, so its time tracks source completion.

## Fixtures

The fixture holds 100 or 1,000 declared and unregistered entries in the shares below. Each conflicting checkout also shows as a row of its own identity beside the declaration it blocks, so the inventory has 105 or 1,050 rows.

| Kind | Share | How it is built |
| --- | --- | --- |
| Declared, clean | 50% | Explicit declaration and a checkout at its destination |
| Declared, dirty | 10% | As above, with a modified tracked file |
| Declared, missing | 10% | Explicit declaration, no checkout |
| Unregistered | 20% | Checkout with an origin and no declaration |
| Conflicting | 5% | Declared destination holding a checkout of another identity |
| Unreadable | 5% | Declared destination holding a checkout with mode `000` |

Every checkout is a copy of one single-commit repository with its own `origin`. The configuration declares a `github.com` provider. Its `gh` is a stub that answers with an empty catalog after 20 seconds, longer than any delayed-provider measurement.

## Method

```sh
cargo build --release --locked
python3 scripts/inventory-performance.py
```

The script takes `--sizes`, `--runs`, `--events`, `--binary` and `--provider-only`, which measures the delayed-provider rows alone, and prints JSON. For each size it builds the fixture in a fresh temporary directory with its own HOME, configuration, `LAGER_CACHE_DIR`, a PATH of stub `gh`, `pbcopy` and `open` before the system tools, Git's global and system configuration off, and dead HTTP proxies. It never reads the real HOME and removes the directory afterwards.

Every measurement runs `lager tui` on a pseudo-terminal of 132×32 that is its controlling terminal, reads the output as it arrives and keeps a screen model of it.

- **First usable frame:** from just before the process is spawned to the read after which the header shows `NORMAL ·`, which the table frame draws and the loading frame does not.
- **Source completion:** from the same start to the read after which the source line reads `Local + declarations` or `Local: incomplete` without `scanning` and without a `pending` count.
- **Cold and warm:** Lager keeps no cache between processes, so every launch is cold in the PRD's sense. The cold series starts right after the fixture is built. The warm series follows three discarded launches that warm the OS caches.
- **Search:** `/` opens search, then `z` and Backspace alternate; each key is timed from the write to the read after which the search row shows the new query.
- **Navigation:** `j` and `k` alternate; each key is timed from the write to the read after which another row is highlighted.
- **During local refresh:** after the first scan completes, `R` starts a refresh, ten keys follow, and the cycle repeats. Only keys sent while the source line shows `scanning` or `pending` count, until 100 have.
- **During delayed provider:** `tui --remote` starts, and keys are sent once the local scan and every Git observation have finished while the source line still shows `github.com: loading`.
- **`lager ls`:** wall time of the whole process, output captured.

The measured time includes the reading side's own processing, so each figure is an upper bound for Lager.
