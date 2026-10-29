# ADR 0036: explicit requested reviewers

Status: accepted.

## Decision

Store at most 20 requested reviewer identities per proposal as routing metadata. The
proposal author owns the list. Requests do not grant repository access, count as approval,
add a policy requirement, send an external message or automatically integrate anything.
The author cannot request their own review. Actual review permissions remain unchanged.

The list persists across proposal revisions. A review satisfies a request for display only
when it records approval or requested changes against the current proposal head. Comments
and decisions on earlier revisions leave the current request pending. The author can remove
requests explicitly; recording a review does not mutate the requested list.

Replacing the list compares the complete inspected set within the same metadata transaction
that writes the new set and audit event. Identities are canonicalized to a sorted unique set.
A retry whose desired set is already stored succeeds without another audit event. A different
concurrent edit is a conflict. Closed or integrated proposals cannot change their requests.
Stream-scoped credentials are checked against the proposal's immutable target stream.

Schema 9 adds a relational `proposal_reviewers` table with a composite primary key and a
reviewer lookup index. The table supports a requested-reviewer filter before keyset paging.
Back up before upgrading from schema 8; rollback restores that backup. Existing proposals
start with no requests. No canonical history object, merge rule or policy document changes.

HTTP and gRPC expose the same operation and advertise `proposals.review_requests`.
Older clients can still review; older servers omit the additive list and the Web UI hides
request editing when the feature is absent. The CLI and SDKs use the same inspected-set
precondition as the browser.
