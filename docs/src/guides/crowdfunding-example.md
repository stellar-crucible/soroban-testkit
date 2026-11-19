# Worked Example: Crowdfunding and Milestone Escrow

The `examples/crowdfunding` workspace member models a campaign funded with a
SEP-41 asset. A creator sets a funding goal and deadline, backers contribute
before the deadline, and the creator can release the raised funds in milestone
tranches after the goal is reached. If the deadline passes below goal, the
campaign is finalized as failed and each backer can claim their recorded
contribution once.

## Invariants covered

- Contributions move tokens into escrow and update both the campaign total and
  the backer's recorded amount.
- The creator release method rejects a campaign whose goal was not met.
- A failed campaign only returns the caller's recorded contribution; claiming
  zeroes that record, preventing a second withdrawal.
- Proptest varies amounts and goals to check that a creator cannot release
  funds from a failed campaign.

The integration suite is split into fixture, behavior assertions,
budget/storage, and property-test files. The batch refund benchmark builds a
multi-backer campaign and bounds host CPU cost for the batch setup; actual
refund claims remain individually authorized calls.

Run the example with `cargo test -p soroban-testkit-example-crowdfunding`.
