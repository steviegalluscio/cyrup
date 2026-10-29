//! `cyrup-codemode` — the engine-independent pieces pi's `codemode` and `tool_search` share
//! (gap-analysis area 18, `docs/gap-analysis/18-pi-codemode.md`).
//!
//! **[CYRUP-DELTA]** this fork does not ship the `codemode` tool (docs/LEAN-FORK-PLAN.md), so what
//! remains here is what the live consumers — `cyrup-tool-search` and `cyrup-mcp` — actually use:
//! the BM25 ranker, the identifier/sanitiser, the script-source shape, and the JavaScript string,
//! number and key-order semantics they all compute with. None of it knows an engine exists.
//!
//! | module | upstream @v1.0.1 | ledger |
//! |---|---|---|
//! | [`identifier`] | `packages/codemode/src/identifier.ts` | CODE-004 |
//! | [`source`] | `packages/codemode/src/source.ts` | CODE-003 |
//! | [`rank`] | `packages/coding-agent/src/extensions/tool-search/tool.ts` | CODE-010 |
//!
//! # Byte parity with JavaScript
//!
//! The declaration text and the ranker's input are compared with upstream byte for byte, and
//! upstream computes them with JavaScript string and number semantics (UTF-16 lengths, the
//! `String.prototype.trim` whitespace set, `Array.prototype.sort`'s UTF-16 code-unit order,
//! `JSON.stringify`'s number format, own-key order with integer-like keys first). Those semantics
//! are reproduced once, in [`js`], and every module goes through it.

#![forbid(unsafe_code)]

pub mod identifier;
pub mod js;
pub mod rank;
pub mod source;
