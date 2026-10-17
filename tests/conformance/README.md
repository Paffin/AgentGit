# Conformance vectors

`gen_vectors.py` is a from-scratch **Python reference implementation** of AgentGit's hashed formats. It
shares no code with the Rust crates, so agreement between the two is evidence that the *specification*
([`docs/architecture/object-format.md`](../../docs/architecture/object-format.md)) is what the code does —
not merely that the code agrees with itself.

```bash
python3 tests/conformance/gen_vectors.py > tests/conformance/vectors.json
cargo test --workspace            # the Rust tests read vectors.json
```

| Vector | Rust test |
|---|---|
| blob IDs, `ChangesetV1` payload + ID | `crates/ag-changeset/tests/conformance.rs` |
| tree manifest + tree ID | `crates/ag-tree/tests/conformance.rs` |
| attestation signing bytes | `crates/ag-policy/tests/conformance.rs` |
| audit hash chain | `crates/ag-storage/tests/conformance.rs` |

`vectors.json` is committed; regenerate it only when a format version changes deliberately (a hashing or
encoding change is a format break and needs an ADR). A second implementation of AgentGit in any language can
use these vectors to prove compatibility.
