//! Turns pasted text (game links, share links, roblox:// deep links, ids, usernames) into a
//! `ParsedInput`. Pure and local: nothing here touches the network.
//! Rules and the 38 recorded cases: docs/research/research-links-private.json (L16),
//! docs/research/ref_parse.py, docs/research/fixtures/links-parse-cases.json, plus the
//! verifier's gaps (verify-links-private.json L16).

use crate::model::ParsedInput;

/// Never panics; anything unusable is `Invalid` / `Unsupported` with a short user-facing reason.
pub fn parse(input: &str) -> ParsedInput {
    let _ = input;
    todo!("target: parse")
}
