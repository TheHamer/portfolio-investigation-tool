# AI usage

AI assistance was used extensively throughout this project. ChatGPT was my planning and review partner; Claude Code wrote most of the code. I directed both, and I reviewed and evaluated what they produced before accepting it.

## Tools used

- **ChatGPT**, for planning, architectural decisions, alternative approaches and code reviews.
- **Claude Code** (Claude Opus models) in VS Code, for writing, modifying and refactoring code and tests.

## Tasks delegated and influence on the design

**ChatGPT** helped plan the application, assess architectural trade-offs and review the implementation and tests. Its input influenced the layered backend, atomic SQLite imports and exact-decimal financial calculations.

**Claude Code** wrote and refactored the backend, frontend, tests, Docker setup and documentation using that design. I reviewed the code and design decisions, and requested changes where needed.

## How I checked the output

All AI-generated code was reviewed and considered, including its correctness, readability, architectural appropriateness and trade-offs. I evaluated the implementation and design decisions rather than accepting them automatically, and sent work back with specific corrections where it fell short.

The evidence that the application is correct comes from:

- **Automated tests**, written by Claude Code against the contract and run during development: the contract's worked examples, the five-file upload sequence with exact counts, a rejected upload leaving stored trades unchanged, restart persistence, concurrent uploads (identical and conflicting), rollback after a database failure part-way through an import, database constraints, as-of consistency, missing-data handling and reconciliation of every position. The frontend has Vitest tests for formatting, the API client and the upload, overview and position-detail components, and `tools/smoke_test.py` checks a running Docker Compose deployment end to end against the independent recomputation.
- **Independent recomputation.** `tools/recompute.py` recalculates every position and total from the CSVs with Python's `decimal` module, sharing no code with the application. Its results match the application's exactly, and `backend/tests/golden_values.rs` checks the API against them.
- **Manual checks** of the user journey: Claude Code drove the UI in a headless browser and exercised the application through Docker Compose, including the five uploads, the closed and carried-forward positions, persistence across restarts and phone-width layout.
- **AI-assisted code reviews.** Two reviews of the near-final code were carried out with AI assistance. They are not independent human assessments. They found the core functionality and financial calculations correct, and their remaining recommendations were evaluated and addressed.

## Suggestions corrected or rejected

- **Closed positions without a price.** The first version marked a closed position unavailable if its instrument had no current price, which Claude Code had documented as the conservative choice. Review identified that a closed position is worth zero whatever the price, so this would wrongly withhold the portfolio totals. Such positions are now valued, while their historical trade-day FX rates are still required.
- **An inferred holiday calendar.** An early error message described valid trade dates as "weekdays only". That is an inference the contract explicitly forbids, so the wording was removed.
- **Misleading colour.** Signed trade costs were first coloured green or red, which suggests profit or loss; colouring is now reserved for P&L.
- **Problems from my review of the first version**, listed in a refactoring brief: precision checks ignored trailing zeros (`10.000000` was accepted as a price); quantities had an undocumented 32-bit limit; the SQLite decimal checks accepted `0` and `1..2`; a query could not use its index fully; an error path multiplied decimals without overflow checks; and IDs with surrounding whitespace were rejected, a rule the contract does not state. All six were fixed, with tests.
