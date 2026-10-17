# Security model

AgentGit assumes that some of the code, some of the people and — above all — some of the *agents* touching
a repository are wrong, confused or hostile. This page states what is defended against, where the defence
lives, which test exercises it, and what is **not** defended.

To report a vulnerability see [SECURITY.md](../../SECURITY.md).

## Trust zones

Four inputs are treated as untrusted: **user code**, **repository content**, **agents**, and **results of
tools run on that content**. The deterministic core (hashing, formats, merge, CAS) does not depend on any
of them being honest.

| Zone | What it can do | What it cannot do |
|---|---|---|
| Agent | propose changes, claim work (leases), attach signed provenance | integrate on its own (default), approve its own work, forge a CI result, write outside its workspace |
| Repository content | be merged, checked out, validated | escape the checkout, plant `.git`/`.ag`, follow a symlink out |
| Execution plane (CI) | issue signed attestations **if its key is listed in the policy** | be impersonated by anyone without its key |
| Local user with disk access | everything | — see "Known limits" |

## Controls

| Threat | Control | Where | Test |
|---|---|---|---|
| Object substitution / bit rot | content IDs recomputed on **every read**; corruption is an error, never data | `ObjectStore::get` | `fs_detects_corruption_and_put_heals_it`, `a_damaged_blob_is_an_error_not_a_silent_checkout` |
| Non-canonical encodings that decode to the same value | strict decoders: minimal varints, no trailing bytes, sorted unique ops | `ag_object::canon`, `Changeset::decode` | `rejects_non_minimal_and_overflowing_varints`, `accepted_varints_reencode_identically`, cross-implementation vectors |
| Hash confusion between object types | domain-separated IDs (kind and version in the preimage) | `ag_object::object_id` | `domain_separation_by_kind_and_version` |
| Oversized / pathological input | limits on blob size, ops, parents, message, DAG depth, tree size; length prefixes bounded before allocation | `ag_types::Limits`, `Reader::bounded_varint` | `bounded_reads_do_not_allocate_huge_buffers`, `rejects_duplicate_parents_and_limits` |
| Path traversal | `RepoPath` forbids `..`, `.`, absolute paths, empty components, NUL/control chars, `\`; **re-validated on decode** | `ag_types::RepoPath` | `rejects_dangerous_paths`, `decode_rejects_traversal_paths`, property test |
| Planting `.git` / `.ag` (hook injection) | those components are rejected in any case combination | `RepoPath`, Git importer | `hostile_git_content_cannot_escape_or_plant_dot_git` |
| Writing through symlinks on checkout | every parent must be a real directory; symlink targets must stay inside the root (no absolute, no `..` climbing above it); temp-file + rename | `workdir::{safe_target,check_link_target}`; also re-checked at publication | `checkout_refuses_to_write_through_symlinked_directories`, `symlink_targets_must_stay_inside_the_root` |
| Terminal escape injection via commit messages, names, diffs | control characters are rendered as `\u{…}` in human output | `sanitize_for_terminal` | `hostile_text_in_history_cannot_inject_terminal_escapes` |
| Lost or duplicated update under races | stream CAS on `(head, version)` inside the committing transaction | `Tx::cas_stream`, `commit_integration` | concurrency tests (12 and 24 parallel connections) |
| Replay / double submission | idempotency keys bound to a request hash | `intent_create` | `reused_idempotency_key_for_a_different_request_is_refused` |
| Forged CI results | attestations are signed, bound to tree + policy version + environment, and only count if issuer *and key* are in `trusted_issuers`; a trusted FAIL beats a PASS | `ag-policy` | `self_signed_agent_attestations_are_worthless`, `attestation_is_bound_to_tree_policy_version_and_signature` |
| Stale validation | the candidate tree ID changes when the stream moves, so old attestations stop matching | engine | `attestation_cannot_be_reused_after_the_stream_moves` |
| Self-approval, approval laundering | approvals bind to the reviewed head; compared by *principal*, so new runs do not help; agents don't count by default | `ag-policy` | `one_person_cannot_approve_twice_via_runs`, `approvals_are_bound_to_the_reviewed_head_and_exclude_the_author` |
| Agents exceeding their mandate | integration by agents is off unless `agent_autonomy` allows it and the change fits (paths, file count, no protected paths); human approvals required otherwise | `ag-policy` | `agent_autonomy_envelope_and_provenance`, `agents_propose_humans_integrate` |
| Stale lock holders | exclusive leases + strictly increasing fencing tokens checked server-side | `ag-agent::leases` | `fencing_tokens_are_strictly_increasing_and_reject_stale_holders` |
| Forged provenance | records are signed with the registered agent key; the key must match the registry; a record cannot be re-filed under another changeset | `ag-agent::provenance` | `provenance_is_signed_verified_and_tamper_evident` |
| Policy typos silently weakening rules | unknown fields rejected; unsatisfiable policies rejected on load | `Policy::parse` | `unknown_fields_are_rejected`, `unsatisfiable_policies_are_rejected` |
| Tampering with history/audit | hash-chained audit entries written in the same transaction as the change; `ag audit`/`ag verify` | `Tx::audit_*`, `fsck` | `audit_chain_detects_tampering`, `fsck_reports_a_healthy_repository_and_finds_each_kind_of_damage` |
| Data loss by GC | fail closed; roots incl. reflog retention; grace period; atomic age-checked delete | `gc.rs` | GC tests in [Publication & integration](publication-and-integration.md) |
| Key theft from disk | key files `0600`, refused if group/world accessible, never overwritten | `ag_crypto::KeyPair` | `key_file_roundtrip_and_permissions` |

## Prompt injection

AgentGit never feeds repository text to a model and never interprets it as instructions. Fields that agents
fill in (`inputs`, `rationale` in provenance, commit messages, file contents) are stored and displayed as
**untrusted data**; the CLI neutralizes terminal control characters. If *you* build an agent on top, treat
everything it reads from the repository the same way — AgentGit gives you identity, isolation and policy so
that a hijacked agent can still only *propose*.

## Known limits (read these)

- **Local mode trusts the local user.** `--as` / `AG_IDENTITY` is a label, not authentication. Anyone who
  can write `.ag/` can edit `meta.db` or the object store. Integrity checks and the audit chain make
  tampering **detectable**, not impossible. Enforcing identities across users requires the planned
  server mode with authentication and short-lived scoped credentials.
- **No sandbox.** `ag validate run` executes the command on your machine with a cleared environment and a
  temporary directory — that is hygiene, not isolation. Use a real isolated execution plane for untrusted
  code (planned).
- **Keys are not encrypted at rest** and live in the repository by default. Put them elsewhere with
  `AG_KEY_DIR`, and keep them out of environments that run untrusted code: whoever can read an agent's key
  can forge its provenance; whoever can read an issuer key can forge validations.
- **Case-insensitive filesystems** (macOS/Windows defaults) can fold paths that differ only by case at
  checkout. Not yet detected.
- **Availability.** Limits bound single inputs, but there is no rate limiting or quota system (those belong
  to the server).
- **No external audit** has been performed.
