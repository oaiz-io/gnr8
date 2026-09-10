# Future stronger design: protected-change attestations

Status: issue-ready design; not implemented.

## Goal

Permit an intentional protected-surface break only after a trusted reviewer approves the exact
finding set at the exact pull-request head. A new commit, a changed base, a policy change, or any
added/removed/changed protected finding must invalidate that approval.

The checked-in exact-finding acceptance list deliberately trusts normal repository review, binds
each record to the report's exact graph-delta fingerprint, and fails when that delta becomes stale.
It does not prove that a separately authorized reviewer approved
the current head. Pull-request labels, branch-committed records, and unsigned comment commands are
insufficient for that stronger trust boundary: the pull-request author can change or replay them,
and none inherently binds the decision to report content and the current head.

## Attested payload

Define a versioned canonical JSON payload containing:

- repository identity, resolved base commit, and pull-request head commit;
- the sorted `gate_operations` and `exempt_tags` policy;
- the change-report schema version;
- every protected breaking finding, in report order, including its code, operation and operation id,
  subject, affected operations on both sides, tags, exemption/protection state, and message; and
- a SHA-256 digest over the canonical bytes.

Canonicalization must be repository-owned and shared by report creation and verification. It must not
depend on JSON object insertion order or a third-party generator. The attestation signs the payload
digest plus its schema version; the head commit remains explicit even though it also determines the
report, making commit invalidation independently auditable.

## Trust and flow

1. The ordinary unprivileged pull-request workflow runs `gnr8 changes`, publishes the reports, and
   uploads the canonical approval payload. It cannot approve itself.
2. A reviewer-triggered workflow that exists on the default branch verifies the actor has the chosen
   repository role, downloads artifacts from the exact workflow run, verifies repository/PR/head/base,
   and displays the protected findings for review.
3. On approval, that trusted workflow signs the payload digest with a repository-controlled key or an
   identity-backed signing service and stores the attestation outside the pull-request branch.
4. A later gate run downloads the attestation by an immutable identifier and verifies the signature,
   trust root, repository, head, base, policy, schema version, and recomputed finding digest. Only an
   exact match changes the protected findings from denied to reviewed. Missing, malformed, expired, or
   mismatched attestations do not weaken the gate.

The trusted workflow must never execute pull-request code. It consumes report bytes as untrusted data,
applies size/schema limits, and performs authorization using default-branch code. Key rotation and
attestation expiry need explicit policy. A copied attestation cannot authorize another repository,
head, base, policy, or finding set because all are signed.

## Architecture gap

gnr8 currently has no trusted attestation store, signing-key configuration, reviewer-authorization
boundary, or canonical approval-payload format. The checked-in acceptance mechanism is therefore a
repository-reviewed record, not an implementation of this separately authorized design.

## Acceptance criteria for implementation

- Canonical payload bytes and digest are deterministic on every supported runner.
- One added, removed, or modified finding invalidates the signature.
- Any new head commit invalidates the signature, even when findings are unchanged.
- A base revision, selector, exemption, repository, or schema-version change invalidates it.
- Unsigned, wrongly signed, expired, replayed, oversized, and malformed attestations fail closed.
- Fork authors and pull-request workflow tokens cannot create a trusted attestation.
- Verification failure preserves reports, comments, summaries, artifacts, and annotations while the
  protected-change gate remains failed.
- Tests cover signer authorization, key rotation, concurrent new commits, artifact substitution, and
  exact-match success.
