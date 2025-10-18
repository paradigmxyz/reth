# feat(arb-reth): add arb-reth CLI bin scaffold and workspace member

This PR introduces the initial scaffolding for a dedicated arb-reth binary, mirroring op-reth’s structure. This keeps reth modular and avoids forking.

Changes:
- Add crates/arbitrum/bin with a minimal arb-reth entrypoint
- Include crates/arbitrum/bin in workspace members
- Ensure reth builds with and without the arbitrum feature (no behavior change to main reth binary)

Link to Devin run: https://app.devin.ai/sessions/47efd5a758e24a76bfc18c712f9d3a92
Requested by: Til Jordan (@tiljrd)
