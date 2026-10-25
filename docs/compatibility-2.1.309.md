# Native CLI compatibility audit

Verified on macOS arm64, 2026-10-24. Target: 2.1.309; retained floor: 2.1.220.
Tracks #844. Native binaries came from exact npm package versions in isolated
task directories, with install scripts disabled. No global binary upgrade was used.

## Native contract results

- 2.1.220: test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 1.28s
- 2.1.309: test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.64s

The complete existing contract suite passed at both endpoints. Pinned PR CI and
the declared tested maximum now agree. Help/config/schema and local protocol
contracts do not prove model behavior or every upstream semantic.

## Local repository checks

- unit-all: test result: ok. 691 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 7.43s
- doc: test result: ok. 93 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
- fake-async: test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s
- fake-sync-all: test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s
- fake-sync-only: test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s
- integration-fixtures: test result: ok. 11 passed; 0 failed; 11 ignored; 0 measured; 0 filtered out; finished in 0.00s; test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s; test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.03s; test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.33s; test result: ok. 0 passed; 0 failed; 29 ignored; 0 measured; 0 filtered out; finished in 0.00s; test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.10s; test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s; test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s; test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s

Required format, feature/build and Clippy checks also passed. Existing controlled
fixtures cover schema/results, sessions, interruption and process settlement.
No new authenticated native smoke result is claimed. Historical native-capture
versions elsewhere in the documentation remain unchanged.

## Fixture isolation correction

The initial auth-error fixture used shell command substitution inside its fake
message and invoked the real CLI (#850). Its owned process group exited before
a guarded cleanup attempt; no cleanup signal was sent. Provider activity was
not established. The fixture now prints the message literally. A marker-writing
fake provider shadows PATH for that test and a two-second deadline bounds it.
The guard failed against the old quoting and passes with the correction; the
full checks above use the corrected fixture. Production execution is unchanged.
