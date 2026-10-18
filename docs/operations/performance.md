# Performance under load

What a real `ag-server` does when many clients talk to it at once: throughput and latency of integrations on
different streams and on one stream, event fan-out, the rate limit, and what happens when the server is told it is
saturated. The cost of the engine itself, by repository size, is in
[Performance of the engine](../architecture/performance.md).

These are measurements with their conditions, not service-level objectives. AgentGit promises none of these numbers.

## How it is measured

`ag-bench load` (`crates/ag-bench`) starts a **real `ag-server` process** for each scenario — fresh data directory,
SQLite control database, SQLite metadata and one file per object, `synchronous=FULL`, the server's default
configuration except the setting a scenario varies — creates users and credentials through the admin API, and drives
it with real clients from threads of its own process:

```sh
cargo build --release -p ag-server -p ag-bench
target/release/ag-bench load                            # every scenario, both protocols, 20 s each
target/release/ag-bench load --profile smoke            # 3 s each, 4 writers: what CI runs
target/release/ag-bench load --scenarios streams,sse --protocols http --workers 32 --duration-secs 60
target/release/ag-bench load --data-parent /dev/shm     # keep the servers' data on a memory-backed file system
target/release/ag-bench load --json out.json --markdown out.md
```

A *cycle* is what an agent does to land a change: build a changeset of two files, **upload** its objects, open a
**proposal**, **integrate** it. HTTP/JSON uploads each object with its own `PUT`; gRPC sends them as one client
stream, so the upload step differs between the protocols by design. A *unit* completed is a cycle that reached an
answer from the server; what counts is stated per scenario. Latency percentiles are nearest-rank over all
operations of the run (a reported p99 is a latency that happened); the clients are closed-loop (a client sends its
next request when the previous one is answered), so throughput and latency are not independent: more clients on a
saturated server raise latency, not throughput.

The generator and the server **share the machine**, so the numbers include the generator's own CPU use, and the
server's CPU seconds (from `/proc/<pid>/stat`) are shown next to them. Retries are not hidden: a failed cycle is
counted as a failure and the client starts a new one after the server's retry hint.

Conditions of the numbers below: commit `ff68edb` plus the change that introduced `ag-bench`; Intel Xeon @ 2.80 GHz,
4 logical CPUs, 16 GiB, Linux 6.18, rustc 1.97, release builds of both; 16 writers (one thread and one credential
each, plus one integrator credential each), 20 s per scenario. The data lived on the VM's virtual disk. It is a shared
VM: the load average was 2.4 when the run began and 13 when it ended, because other jobs were running; the numbers are
pessimistic and noisy, and differences below roughly 30 % mean nothing. Re-run on the hardware you care about.

## Results

### Reads: the ceiling

32 clients reading a stream head (credential check, authorization, one metadata read), no rate limit:

| protocol | requests/s | p50 | p90 | p99 | max | server CPU |
|---|---:|---:|---:|---:|---:|---:|
| HTTP/JSON | 5 100 | 5.0 ms | 11.9 ms | 21.3 ms | 47.9 ms | 159 % of one core |
| gRPC | 5 062 | 5.9 ms | 9.7 ms | 15.1 ms | 71.8 ms | 172 % of one core |

Both protocols are at the same ceiling, which is mostly the shared four cores (the generator takes the rest).

### Integrations on different streams

16 clients, each with its own stream of one repository; every cycle is a fast-forward.

| protocol | integrations/s | upload p50 / p99 | propose p50 / p99 | integrate p50 / p99 | whole cycle p50 / p99 / max | failures |
|---|---:|---:|---:|---:|---:|---:|
| HTTP/JSON | 79.5 | 12.5 / 45.6 ms | 41.5 / 396 ms | 75.4 / 482 ms | 162 / 657 / 1191 ms | 0 |
| gRPC | 72.3 | 12.0 / 94.3 ms | 38.9 / 429 ms | 96.0 / 611 ms | 177 / 914 / 1825 ms | 0 |

Different streams do not conflict, but they do not run in parallel either: the server used 88 % of **one** core
while 16 clients waited, i.e. it was not CPU-bound. The same scenario with the servers' data on a memory-backed file
system (`--data-parent /dev/shm`, HTTP/JSON, same VM, run afterwards) did **231 integrations/s** (propose p50 12 ms,
integrate p50 25 ms, cycle p50 55 ms): about 2.9 times more. On this VM, therefore, most of the write latency is
the disk: every write transaction is an fsync (`synchronous=FULL`, a deliberate durability choice documented in
[Durability](durability.md)), and writers to one repository's SQLite database take turns. That is a statement about
this VM's disk; on a disk with cheap fsyncs the ratio will differ. PostgreSQL metadata (`[repos]` storage, see
[PostgreSQL & S3](postgres-and-s3.md)) was not measured.

### Integrations on one stream

16 clients on the stream `main`, each reading the head, changing its own two files and integrating. A change built
on a head that has moved by the time it is integrated is merged by the server (three-way merge).

| protocol | answered/s | integrated as | integrate p50 / p99 | whole cycle p50 / p99 / max | `aborted` |
|---|---:|---|---:|---:|---:|
| HTTP/JSON | 44.1 | 95 fast-forward, 807 merge | 43.5 / 810 ms | 116 / 1145 / 2004 ms | 112 (11 %) |
| gRPC | 48.8 | 98 fast-forward, 895 merge | 58.9 / 655 ms | 99 / 681 / 1101 ms | 159 (14 %) |

Failure mode: when the stream moves again and again while a merge is being prepared, the server gives up after eight
attempts and answers `aborted` — "stream changed 8 times while integrating; retry" (HTTP 409, retryable). It lost
nothing: the proposal is untouched and the client integrates it again. At 16 writers on one stream, roughly one
integration in nine needed that second try. Clients should treat `aborted` as "retry" (the error is marked retryable in the protocol); the generator here does not retry inside a cycle, it counts the failure and starts a new cycle.

Same file, same stream (every client changes `shared.txt` on the head it read), 16 clients:

| protocol | answered/s | integrated | `conflict` outcomes | integrate p50 / p99 | failures |
|---|---:|---:|---:|---:|---:|
| HTTP/JSON | 91.6 | 329 | 1 512 | 57.6 / 386 ms | 0 |
| gRPC | 93.3 | 319 | 1 559 | 60.4 / 341 ms | 0 |

Nothing is lost or merged silently: the first change on a head wins, the others are told which paths conflict.
A conflict is answered faster than a successful merge because no merge changeset is written.

### Event fan-out

100 subscribers (50 SSE over HTTP/JSON, 50 gRPC server streams) follow one repository while 16 writers integrate on
their own streams. Delivery lag is the client's receive time minus the event's server timestamp (same host; 1 ms
resolution).

| | value |
|---|---|
| integrations during the run | 1 049 (51.5/s; 79.5/s in the otherwise identical `streams` run without subscribers, on a machine whose load varied) |
| `stream.integrate` events delivered | 1 049 to every one of the 100 subscribers; 0 duplicates; 0 feeds failed |
| lag, SSE | p50 37 ms, p90 136 ms, p99 287 ms, max 622 ms |
| lag, gRPC | p50 37 ms, p90 133 ms, p99 281 ms, max 584 ms |
| server | 215 % of one core, 81 MiB peak RSS |

Every event reached every subscriber, once. The lag is dominated by the machine being saturated (116 threads of load
generator on four cores), not by the feed; it should be read as an upper bound for this setup.

### The rate limit

The default limit is per credential (`limits.requests_per_second = 200`, `burst = 400`); this scenario uses 50 and
100. One credential is hammered by 8 closed-loop clients for 20 s while a second credential makes one call every
200 ms.

| protocol | requests sent | admitted | the configuration allows | rejected | second credential |
|---|---:|---:|---:|---|---|
| HTTP/JSON | 160 256 | 1 100 (55.0/s) | burst + rate × time = 100 + 50 × 20 = 1 100 | `rate_limited`, HTTP 429, every one | 100 of 100 answered |
| gRPC | 172 682 | 1 100 (55.0/s) | 1 100 | `rate_limited` (gRPC `RESOURCE_EXHAUSTED`) | 100 of 100 answered |

The limiter admits exactly what the token bucket allows, rejections cost the server less than a served request
(p50 0.87 ms against 0.95 ms) and do not disturb other credentials. The retry hint: the error body carries
`retry_after_ms` (1–20 ms here); the HTTP `Retry-After` header is in whole seconds rounded up (every rejection said
`1`), as the header's format requires, so a client that honours the header waits much longer than the bucket needs
— clients that can read the JSON hint should prefer it. One second after the hammering stopped the credential was
served again. A rejection is cheap but not free: 8 clients rejected 8 000 times a second used about 150 % of one core.

### Saturation: load shedding

The server refuses requests beyond `limits.max_concurrent_requests` at once instead of queuing them. With the cap
set to 4 (the default is 512) and 64 clients reading a stream head:

| protocol | requests/s answered | served | refused (`unavailable`, HTTP 503) | refusal p50 | served p50 |
|---|---:|---:|---:|---:|---:|
| HTTP/JSON | 5 104 | 45 639 | 56 525 | 9.4 ms | 12.8 ms |
| gRPC | 15 140 | 56 578 | 246 387 | 3.7 ms | 5.0 ms |

The server stayed up and ready (`/readyz` answered 200 after the run). Refusals carry `unavailable` and are
retryable. The latency of a refusal is mostly the time to get scheduled on a saturated machine, not the server's work.
gRPC clients got through far more refused requests per second in this run; this was not investigated further.

## What these numbers do not say

* **No deployment is measured.** One VM, loopback networking, the generator on the same cores, a virtual disk.
* **PostgreSQL metadata, S3 objects, several replicas, TLS** were not exercised.
* **Small repositories.** Each scenario starts from an empty repository and a run adds a few thousand small
  changesets. Behaviour on a 100 000-file repository under load follows from the [engine numbers](../architecture/performance.md)
  (an integration there costs 0.3–1 s of CPU) but was not measured end to end.
* **Latency of a single idle request** is not what a user sees: closed-loop clients on a saturated server measure
  queueing. The idle cost is the engine table plus a few milliseconds.
* **Long runs.** 20 s per scenario shows no failure over time, memory growth or compaction effects.
* **The Web UI, MCP server and workers** are not driven.

## Finding the bottleneck of your own setup

* The server's request and integration metrics and its CPU are in [Observability](observability.md); if the
  server is far from one core busy and integrations are slow, look at the disk (fsync latency) before the CPU.
* Same-stream `aborted` answers mean many writers integrate to one stream; give agents their own streams and
  integrate them in turn, or retry.
* To see how much of your write latency is the disk, run the `streams` scenario twice, once with `--data-parent`
  on a memory-backed file system.

## In CI

[`perf.yml`](../../.github/workflows/perf.yml) runs on changes to the server, the engine and the tool:

* **smoke** (every push to `main` touching those paths and every pull request touching the tool): the engine
  benchmark at 1 000 and 10 000 files and `ag-bench load --profile smoke`, release builds, **non-gating**
  (`continue-on-error`); the results are in the job summary and uploaded as the `perf-smoke` artifact. A runner's
  numbers are not compared with anything: the point is that the tools still run end to end.
* **full** (weekly, and by hand): the sizes in this page, the artifact is `perf-full`.

The functional behaviour that the load run exercises — `rate_limited` rejections never exceeding the bucket, shedding
that leaves the server ready, every subscriber receiving every `stream.integrate` event once, same-file changes
producing `conflict` — is also asserted, with small numbers, by `cargo test -p ag-bench`
(`crates/ag-bench/tests/smoke.rs`), which starts a real `ag-server` process.
