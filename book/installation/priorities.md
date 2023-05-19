# Update Priorities

When publishing releases, reth will include an "Update Priority" section in the release notes, in the same manner Lighthouse does.

The "Update Priority" section will include a table which may appear like so:

| User Class           | Priority        |
|----------------------|-----------------|
| Payload Builders     | Medium Priority |
| Non-Payload Builders | Low Priority    |

To understand this table, the following terms are important:

- *Payload builders* are those who use reth to build and validate payloads.
- *Non-payload builders* are those who run reth for other purposes (e.g., data analysis, RPC or applications).
- *High priority* updates should be completed as soon as possible (e.g., hours or days).
- *Medium priority* updates should be completed at the next convenience (e.g., days or a week).
- *Low priority* updates should be completed in the next routine update cycle (e.g., two weeks).
