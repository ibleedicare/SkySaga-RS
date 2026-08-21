# Alpha V10 (b36731) investigation tools

Everything here reads a **running** 2017 client, except the generator. They exist because
static reading of this binary proved unreliable: nearly every wrong conclusion during the
Alpha V10 work came from inferring structure out of raw bytes, and nearly every correct one
came from asking the live process a direct question.

All the memory readers need `kernel.yama.ptrace_scope=0` and are read-only, with one exception
noted below.

| tool | what it answers |
|---|---|
| `gen-packet-map-b36731.py` | regenerates `client_build.rs` from the doc's id table, cross-checked against `PacketId.cs` |
| `loading-stage-b36731.py` | which loading stage the client is in; can also **request** a transition (`--request`, the only writer here) |
| `level-ready-b36731.py` | the `ModeLevelBase` state and the readiness byte that gate `DOWNLOAD_WORLD` |
| `level-pending-b36731.py` | the twelve loaders and which one has not finished |
| `walk-loader-registry-b36731.py` | the list a loader searches to reach state 2 |
| `diff-loader-b36731.py` | a stalled object against a working one, field by field |
| `resource-states-b36731.py` | the resource table's state histogram |
| `dump-resource-entry-b36731.py` | whole resource entries |
| `walk-resource-object-b36731.py` | follows pointers looking for anything that names an object |
| `name-geodata-tables-b36731.py` | GeoData table counts from a live client |
| `probe-geodata-tables-b36731.py`, `dump-geodata-container-b36731.py` | how the GeoData manager stores its tables |

## Addresses

Several tools take a default address. Under Wine these have been stable across every observed
run, which is why the defaults are useful at all:

| | |
|---|---|
| `0x10232f90` | the loading-screen global (`DAT_0143785c`'s value) |
| `0x1301e520` | the `ModeLevelBase` |
| `0x10022ee0` | the resource manager |

They are still **per-process**, so re-derive rather than trust them if anything reads oddly.
`scripts/attach-when-client-starts.sh` and the breakpoint on `0x008f7a7a` recover the first two.

## A warning about `--set`

`loading-stage-b36731.py --set` writes the *current* stage byte, which only changes what the
loading screen says. It does not run the transition, and it will happily make the screen claim
a stage the client is not in. Use `--request`, which writes the requested stage and the pending
flag so the client's own tick performs the transition.
