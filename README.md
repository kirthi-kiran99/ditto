# ditto
Ditto is a production traffic recorder and replay engine for Rust services. It captures every outbound HTTP call, database query, Redis operation, and internal function I/O — then replays the exact sequence against new builds in CI, diffing responses to catch regressions before they reach users.
