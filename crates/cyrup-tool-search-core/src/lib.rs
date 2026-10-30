//! `cyrup-tool-search-core` — the engine-independent pieces pi's `codemode` and `tool_search` share
//! (gap-analysis area 18, `docs/gap-analysis/18-pi-codemode.md`).
//!
//! **[CYRUP-DELTA]** this fork does not ship the `codemode` tool (docs/LEAN-FORK-PLAN.md step 1), so
//! what remains here is what the live consumers — `cyrup-tool-search` and `cyrup-mcp` — actually use:
//! the BM25 ranker, the identifier/sanitiser, the script-source shape, and the JavaScript string,
//! number and key-order semantics they all compute with. None of it knows an engine exists, and no
//! engine is in the graph any more: both of pi's JavaScript consumers, the codemode sandbox and
//! `workflowScript`, are deleted (steps 1 and 4), which is what removed `deno_core` and V8.
//!
//! Renamed from `cyrup-codemode` at step 3 of that plan. The crate outlived the tool its name named,
//! and a crate called `codemode` in a fork with no codemode reads like a leftover rather than a
//! dependency. The rename is a separate, deletion-free commit so `git log --follow` survives.
//!
//! | module | upstream @v1.0.1 | ledger |
//! |---|---|---|
//! | [`identifier`] | `packages/codemode/src/identifier.ts` | CODE-004 |
//! | [`source`] | `packages/codemode/src/source.ts` | CODE-003 |
//! | [`rank`] | `packages/coding-agent/src/extensions/tool-search/tool.ts` | CODE-010 |
//!
//! # Byte parity with JavaScript
//!
//! The ranker's input and the schema text `cyrup-mcp` renders are compared with upstream byte for
//! byte, and
//! upstream computes them with JavaScript string and number semantics (UTF-16 lengths, the
//! `String.prototype.trim` whitespace set, `Array.prototype.sort`'s UTF-16 code-unit order,
//! `JSON.stringify`'s number format, own-key order with integer-like keys first). Those semantics
//! are reproduced once, in [`js`], and every module goes through it.

#![forbid(unsafe_code)]

pub mod identifier;
pub mod js;
pub mod rank;
pub mod source;
