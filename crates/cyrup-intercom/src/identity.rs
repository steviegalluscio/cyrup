//! Env-var identity adoption + addressing — a port of `pi-intercom/index.ts:20-119,368-386,856-888`.
//!
//! Env names mirror pi with the `CYRUP_` prefix (the memory rule: port the literal mechanism).
//!
//! [CYRUP-DELTA] docs/LEAN-FORK-PLAN.md step 4 deletes pi's subagents extension, and with it the
//! whole subagent-child side of this module: `CYRUP_SUBAGENT_ORCHESTRATOR_*`/`CYRUP_SUBAGENT_CHILD_*`
//! addressing, `ChildOrchestratorMetadata` + `readChildOrchestratorMetadata`, the supervisor-channel
//! probe, and the `contact_supervisor` gate those were keyed on (`index.ts:84-103,1162-1163`). A
//! "supervisor" only ever existed as the parent of a spawned subagent, so with nothing spawning
//! children there is nothing left to address.

/// `DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX = "subagent-chat"` (`index.ts:20`).
pub const DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX: &str = "subagent-chat";

/// `CYRUP_INTERCOM_SESSION_ID` (`index.ts:441,606-615`, pi `PI_INTERCOM_SESSION_ID`).
///
/// Direction matters: upstream **publishes** this (`publishIntercomSessionId`, `index.ts:612-614`,
/// called from `startSessionRuntime`, `index.ts:946`) so a spawned CHILD inherits it and reads it
/// back as its SUPERVISOR's id (the child-orchestrator metadata that read it back is deleted here,
/// with the subagents extension that wrote it). A
/// session never re-reads it as its OWN registration id; that is
/// `ctx.sessionManager.getSessionId()` (cyrup: `HostServices::session_id()`, see
/// `crate::connect::connect_once`). Reading it for self-registration would make a child
/// re-register under its parent's id and take the parent's broker slot over.
pub const ENV_INTERCOM_SESSION_ID: &str = "CYRUP_INTERCOM_SESSION_ID";
/// `CYRUP_SESSION_ID` (pi `PI_SESSION_ID`) — the session id the bash tool publishes into every
/// command it runs (`cyrup-tools` `resolveSpawnContext`). `cyrup-intercom-cli send --to
/// name@machine` reads it as the second rung of its origin ladder (`resolveOrigin`,
/// `v0.16.0 cross-machine-envelope.ts:45`), after [`ENV_INTERCOM_SESSION_ID`]: a model that shells
/// out to the CLI is speaking FOR the session whose bash tool ran it.
pub const ENV_SESSION_ID: &str = "CYRUP_SESSION_ID";
/// `CYRUP_INTERCOM_ASK_TIMEOUT_MS` (`config.ts:8`, pi `PI_INTERCOM_ASK_TIMEOUT_MS`).
pub const ENV_INTERCOM_ASK_TIMEOUT_MS: &str = "CYRUP_INTERCOM_ASK_TIMEOUT_MS";
/// `CYRUP_INTERCOM_NAME_POLL_MS` (`index.ts:420-429`, pi `PI_INTERCOM_NAME_POLL_MS`).
pub const ENV_INTERCOM_NAME_POLL_MS: &str = "CYRUP_INTERCOM_NAME_POLL_MS";
/// `CYRUP_INTERCOM_LIVENESS_INTERVAL_MS` (pi `PI_INTERCOM_LIVENESS_INTERVAL_MS`,
/// `v0.10.1 broker/client.ts:48`). Declared here rather than in `transport/client.rs` because this
/// module is the crate's single env inventory; the resolver below is the port of `client.ts:47-50`.
pub const ENV_INTERCOM_LIVENESS_INTERVAL_MS: &str = "CYRUP_INTERCOM_LIVENESS_INTERVAL_MS";
/// `CYRUP_INTERCOM_LIVENESS_TIMEOUT_MS` (pi `PI_INTERCOM_LIVENESS_TIMEOUT_MS`,
/// `v0.10.1 broker/client.ts:53`).
pub const ENV_INTERCOM_LIVENESS_TIMEOUT_MS: &str = "CYRUP_INTERCOM_LIVENESS_TIMEOUT_MS";
/// `CYRUP_INTERCOM_STABLE_ID` (pi `STABLE_INTERCOM_SESSION_ID_ENV = "PI_INTERCOM_STABLE_ID"`,
/// `v0.10.1 index.ts:42`), resolved by `resolveConfiguredIntercomSessionId`
/// (`v0.10.1 index.ts:434-436`) and consumed at register.
///
/// Distinct from [`ENV_INTERCOM_SESSION_ID`] in BOTH direction and meaning: that one is *published*
/// downward for a child to read back as its supervisor's id, this one is *read* by a session as its
/// own restart-stable registration id.
pub const ENV_INTERCOM_STABLE_ID: &str = "CYRUP_INTERCOM_STABLE_ID";
/// `CYRUP_INTERCOM_SCOPE_ID` (pi `INTERCOM_SCOPE_ID_ENV = "PI_INTERCOM_SCOPE_ID"`,
/// `v0.13.0 config.ts:6`), resolved by [`crate::config::intercom_scope_id`]
/// (`getIntercomScopeId`, `v0.13.0 config.ts:21-24`) and carried on the register frame
/// (`v0.13.0 broker/client.ts:286-292`).
///
/// The broker's OPAQUE routing boundary, and the third distinct meaning in this file's session
/// trio: [`ENV_INTERCOM_SESSION_ID`] is published DOWNWARD for a child to read back as its
/// supervisor's id, [`ENV_INTERCOM_STABLE_ID`] is read by a session as its own restart-stable
/// registration id, and this one is read by a session as the ISOLATION CLASS its id lives in. A
/// session id is unique only within one scope; two scopes may hold the same id and they are two
/// different sessions.
///
/// Opt-in: absent (or whitespace-only) means unscoped, which is the scope every session registers
/// into today. Deliberately never read from `config.json` — that file is machine-global, so a scope
/// stored there would apply to every session on the machine and erase the boundary it draws.
pub const ENV_INTERCOM_SCOPE_ID: &str = "CYRUP_INTERCOM_SCOPE_ID";
///
/// [CYRUP-DELTA] The request type used to live in `cyrup-ext-subagents`, its only claimant, because
/// that crate could not depend on this one. Step 4 of docs/LEAN-FORK-PLAN.md deletes that crate, so
/// the topic and its request type are declared here: intercom is the emitter, and with no subagent
/// launcher left there is no cross-crate claimant to host them beside.
/// `INTERCOM_SESSION_IDENTITY_EVENT` (pi-intercom `v0.14.0 extension-api.ts:7`; pi-subagents keeps
/// its own copy, `shared/types.ts` @v0.71.0). cyrup-intercom emits an
/// [`IntercomSessionIdentityRequestV1`] on this TYPED bus topic at `session_start`, before it
/// chooses the session's intercom id; a native that owns the session's routing address claims it
/// inline.
pub const INTERCOM_SESSION_IDENTITY_EVENT: &str = "intercom:session-identity";

/// pi-intercom `IntercomSessionIdentityRequestV1 { version: 1; claim(stableId): void }`
/// (`v0.14.0 extension-api.ts:17-20`), and the `claimedIntercomSessionId` slot its `claim` writes
/// (`index.ts:1645-1651`).
///
/// Emitted with `HostServices::emit_typed_event`, which runs every listener before it returns, so
/// the emitter reads [`Self::claimed`] right after the emit and registers under it — upstream's
/// order exactly: nothing is registered before the claim is settled.
///
/// [CYRUP-DELTA] the version is the TYPE, not a runtime field: a listener downcasts the typed-bus
/// event to this struct (`cyrup_ext::NativeExtension::on_typed_bus_event`) where upstream checks
/// `request.version !== 1 || typeof request.claim !== "function"` (pi-subagents
/// `subagent-prompt-runtime.ts:538`), so a request of any other shape never reaches `claim`, and a
/// future V2 is a different type that an old listener cannot misread.
#[derive(Debug, Default)]
pub struct IntercomSessionIdentityRequestV1 {
    claimed: std::sync::Mutex<Option<String>>,
}

impl IntercomSessionIdentityRequestV1 {
    /// A fresh request with nothing claimed — upstream's `let claimedIntercomSessionId: string |
    /// undefined;`, one per `session_start`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `claim(stableId)`: `claimedIntercomSessionId ??= (typeof stableId === "string" &&
    /// stableId.trim()) || undefined` (`v0.14.0 index.ts:1648-1650`). The first non-empty trimmed
    /// claim wins; a blank one, or any claim after the first, changes nothing.
    pub fn claim(&self, stable_id: &str) {
        let trimmed = stable_id.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut claimed = self
            .claimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if claimed.is_none() {
            *claimed = Some(trimmed.to_string());
        }
    }

    /// The winning claim, if any listener made one.
    #[must_use]
    pub fn claimed(&self) -> Option<String> {
        self.claimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
/// `HERDR_BIN` — NOT `CYRUP_HERDR_BIN`. Same rule as [`ENV_TMUX_PANE`]: the `CYRUP_` prefix applies
/// to pi's OWN variables, and this one belongs to the Herdr vendor, read verbatim by upstream at
/// `v0.12.0 project-agent.ts:68` (`options.bin ?? process.env.HERDR_BIN ?? "herdr"`). A user who has
/// already set it for Herdr itself must not have to set a second, cyrup-private spelling.
pub const ENV_HERDR_BIN: &str = "HERDR_BIN";
/// `CYRUP_INTERCOM_CYRUP_BIN` (pi `PI_INTERCOM_PI_BIN`, `v0.12.0 project-agent.ts:245`) — the agent
/// binary a launched project pane runs. First rung of the ladder in
/// [`crate::project_pane::resolve_agent_command`].
pub const ENV_INTERCOM_CYRUP_BIN: &str = "CYRUP_INTERCOM_CYRUP_BIN";
/// `CYRUP_BIN` (pi `PI_BIN`, `v0.12.0 project-agent.ts:245`) — the machine-wide fallback for the
/// same, checked after [`ENV_INTERCOM_CYRUP_BIN`].
pub const ENV_CYRUP_BIN: &str = "CYRUP_BIN";
/// `TMUX_PANE` — NOT `CYRUP_TMUX_PANE`. The `CYRUP_` prefix rule applies to pi's OWN variables
/// (`PI_INTERCOM_*` → `CYRUP_INTERCOM_*`); this one is exported by **tmux itself** into every pane's
/// process environment, so renaming it would read a variable nothing ever sets and the pane term
/// would never populate. Do not "fix" this in a later prefix sweep.
pub const ENV_TMUX_PANE: &str = "TMUX_PANE";

/// `currentTmuxPane()` (`v0.12.0 index.ts:531-534`):
///
/// ```text
/// const pane = process.env.TMUX_PANE?.trim();
/// return pane ? pane : undefined;
/// ```
///
/// Upstream's own comment is the rationale: `$TMUX_PANE` is inherited at process start and immutable
/// for the lifetime — moving the pane between windows keeps its id — so it is a stable join key a
/// peer can use to live-resolve the current window via tmux. Absent outside tmux.
///
/// Trimmed AND blank-rejected here, at the producer, so a cyrup session never puts a
/// whitespace-only pane id on the wire.
#[must_use]
pub fn current_tmux_pane_from(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    env(ENV_TMUX_PANE)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// [`current_tmux_pane_from`] over the process environment.
#[must_use]
pub fn current_tmux_pane() -> Option<String> {
    current_tmux_pane_from(|k| std::env::var(k).ok())
}

/// `currentHerdrPane()` (`v0.14.0 index.ts:522-529`, `0ffe1d5`):
///
/// ```text
/// const pane = process.env.HERDR_PANE_ID?.trim();
/// return pane ? pane : undefined;
/// ```
///
/// Upstream's comment: "Herdr exports a launch-time pane alias to hosted processes. Its visible,
/// workspace-qualified id can change when the pane moves … Workspace and tab ids are intentionally
/// never registered." — the broker resolves those per `list` instead ([`crate::herdr_location`]).
///
/// `HERDR_PANE_ID` is Herdr's own variable ([`cyrup_herdr::env::HERDR_PANE_ID`]), not a `PI_*` one,
/// so — like [`ENV_TMUX_PANE`] — it is read verbatim. Trimmed and blank-rejected at the producer.
#[must_use]
pub fn current_herdr_pane_from(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    env(cyrup_herdr::env::HERDR_PANE_ID)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// [`current_herdr_pane_from`] over the process environment.
#[must_use]
pub fn current_herdr_pane() -> Option<String> {
    current_herdr_pane_from(|k| std::env::var(k).ok())
}

/// `getNamePollMs()` (`v0.10.1 index.ts:486-495`, 10 lines):
///
/// ```text
/// const configured = process.env[NAME_POLL_MS_ENV];
/// if (configured !== undefined) {
///   const value = Number(configured);
///   if (Number.isFinite(value) && value > 0) return value;
/// }
/// return 1000;
/// ```
///
/// Note this is NOT `getAskTimeoutMs`'s shape: an invalid value here falls back to the 1000 ms
/// default rather than throwing, and a **fractional** value is accepted (`Number.isFinite`, not
/// `Number.isSafeInteger`). Both differences are upstream's, so both are reproduced —
/// `"1500.5"` polls at 1500 ms after the truncation a `Duration::from_millis` forces.
#[must_use]
pub fn name_poll_ms() -> u64 {
    name_poll_ms_from(|k| std::env::var(k).ok())
}

/// The pure core of [`name_poll_ms`].
#[must_use]
pub fn name_poll_ms_from(env: impl Fn(&str) -> Option<String>) -> u64 {
    const DEFAULT_NAME_POLL_MS: u64 = 1000;
    match env(ENV_INTERCOM_NAME_POLL_MS) {
        // `Number("")` is 0, and `0 > 0` is false, so a blank value takes the default.
        Some(raw) => match raw.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v as u64,
            _ => DEFAULT_NAME_POLL_MS,
        },
        None => DEFAULT_NAME_POLL_MS,
    }
}

/// `Number.parseInt(raw, 10)` — the parse the liveness resolvers use, which is **not** the `Number()`
/// [`name_poll_ms_from`] uses. `parseInt` skips leading whitespace, takes an optional sign, then
/// consumes the longest run of decimal digits and STOPS at the first non-digit: `"30abc"` → 30,
/// `"30.9"` → 30, `"1e3"` → 1, `"0x10"` (radix 10) → 0, `""`/`"abc"`/`" "` → `NaN`. The result is a
/// JS `Number`, so an enormous digit run is a finite `f64`, never an error — hence `f64` here rather
/// than an integer parse that would fall back to the default on overflow.
fn js_parse_int_base10(raw: &str) -> Option<f64> {
    let s = raw.trim_start();
    let mut chars = s.chars().peekable();
    let negative = match chars.peek() {
        Some('-') => {
            chars.next();
            true
        }
        Some('+') => {
            chars.next();
            false
        }
        _ => false,
    };
    let digits: String = std::iter::from_fn(|| chars.next_if(char::is_ascii_digit)).collect();
    if digits.is_empty() {
        return None;
    }
    // A digit-only string always parses as `f64` (saturating to `f64::INFINITY` past ~1e308, which
    // `Number.isFinite` then rejects — exactly as JS does).
    let value: f64 = digits.parse().unwrap_or(f64::INFINITY);
    Some(if negative { -value } else { value })
}

/// `getLivenessIntervalMs()` (`v0.10.1 broker/client.ts:47-50`):
///
/// ```text
/// const raw = Number.parseInt(process.env.PI_INTERCOM_LIVENESS_INTERVAL_MS ?? "", 10);
/// return Number.isFinite(raw) && raw > 0 ? raw : 30_000;
/// ```
#[must_use]
pub fn liveness_interval_ms() -> u64 {
    liveness_interval_ms_from(&|k| std::env::var(k).ok())
}

/// The pure core of [`liveness_interval_ms`].
#[must_use]
pub fn liveness_interval_ms_from(env: &dyn Fn(&str) -> Option<String>) -> u64 {
    const DEFAULT_LIVENESS_INTERVAL_MS: u64 = 30_000;
    match env(ENV_INTERCOM_LIVENESS_INTERVAL_MS)
        .as_deref()
        .and_then(js_parse_int_base10)
    {
        Some(v) if v.is_finite() && v > 0.0 => v as u64,
        _ => DEFAULT_LIVENESS_INTERVAL_MS,
    }
}

/// `getLivenessTimeoutMs()` (`v0.10.1 broker/client.ts:52-55`):
///
/// ```text
/// const raw = Number.parseInt(process.env.PI_INTERCOM_LIVENESS_TIMEOUT_MS ?? "", 10);
/// return Number.isFinite(raw) && raw > 0 ? Math.min(raw, getLivenessIntervalMs()) : 5_000;
/// ```
///
/// The `Math.min` clamp is inside the CONFIGURED branch only: an unset timeout is a flat 5 000 ms
/// and is **not** clamped down to a shorter configured interval. That asymmetry is upstream's and is
/// reproduced verbatim — a `PI_INTERCOM_LIVENESS_INTERVAL_MS=1000` with no timeout override really
/// does probe every second under a 5 s deadline there.
#[must_use]
pub fn liveness_timeout_ms() -> u64 {
    liveness_timeout_ms_from(&|k| std::env::var(k).ok())
}

/// The pure core of [`liveness_timeout_ms`].
#[must_use]
pub fn liveness_timeout_ms_from(env: &dyn Fn(&str) -> Option<String>) -> u64 {
    const DEFAULT_LIVENESS_TIMEOUT_MS: u64 = 5_000;
    match env(ENV_INTERCOM_LIVENESS_TIMEOUT_MS)
        .as_deref()
        .and_then(js_parse_int_base10)
    {
        Some(v) if v.is_finite() && v > 0.0 => (v as u64).min(liveness_interval_ms_from(env)),
        _ => DEFAULT_LIVENESS_TIMEOUT_MS,
    }
}

/// `resolveIntercomPresenceName` (`v0.10.1 index.ts:419-426`): the trimmed session name if present,
/// else the unnamed-session alias `subagent-chat-<id[0:18]>` (with a leading `session-` prefix
/// stripped first).
///
/// The slice was **18**, not 8, from v0.10.0 (`126875e`, CHANGELOG 0.10.0: "Extend unnamed-session
/// fallback aliases with enough session-ID characters to distinguish UUIDv7 sessions started close
/// together"). This alias is the presence NAME the session registers under, so two unnamed sessions
/// whose UUIDv7 ids were minted in the same millisecond used to register the SAME name and neither
/// was reachable by it — `broker/routing.rs::find_session_ids` returned both and the broker answered
/// every send with `Multiple sessions named "…" are connected.`
#[must_use]
pub fn presence_name(session_name: Option<&str>, session_id: &str) -> String {
    if let Some(name) = session_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let normalized = session_id.strip_prefix("session-").unwrap_or(session_id);
    let short: String = normalized.chars().take(18).collect();
    format!("{DEFAULT_UNNAMED_SESSION_ALIAS_PREFIX}-{short}")
}

/// `shortSessionId` (`v0.9.2 index.ts:365-367`): the first 8 chars of a session id.
///
/// Upstream **kept** this at 8 for the picker label (`formatSessionLabel`,
/// `v0.10.1 index.ts:440-446`) and replaced it with [`session_id_prefixes`] for the `list` row,
/// which is the addressable column — do not conflate the two.
#[must_use]
pub fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// `duplicateSessionNames` (`v0.10.1 index.ts:379-386`): the lowercased names held by MORE THAN ONE
/// session in a roster.
///
/// ```text
/// new Set(
///   sessions.map(s => s.name?.toLowerCase())
///     .filter((name): name is string => Boolean(name))
///     .filter((name, index, names) => names.indexOf(name) !== index)
/// )
/// ```
///
/// Two mechanism notes: the `Boolean(name)` filter drops `undefined` **and** `""`, so an empty name
/// is never "duplicated"; and `names.indexOf(name) !== index` runs over the ALREADY-FILTERED array,
/// so the surviving indices are the second and later occurrences. Counting occurrences reproduces
/// that exactly and is what the `> 1` below expresses.
#[must_use]
pub fn duplicate_session_names<'a, I>(names: I) -> std::collections::HashSet<String>
where
    I: IntoIterator<Item = Option<&'a str>>,
{
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for name in names {
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            continue;
        };
        *counts.entry(name.to_lowercase()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, _)| name)
        .collect()
}

/// `formatSessionLabel` (`v0.10.1 index.ts:440-446`): how a peer is NAMED back to the human in the
/// picker, the compose header and the sent-confirmation.
///
/// ```text
/// if (!session.name) return session.id;
/// return duplicates.has(session.name.toLowerCase())
///   ? `${session.name} (${session.id.slice(0, 8)})`
///   : session.name;
/// ```
///
/// `!session.name` is a truthiness test, so an empty name falls through to the raw id — the same
/// `||`-shaped rule `formatSessionListRow` uses, but resolving to the **id** rather than to
/// "Unnamed session". The 8-char suffix is deliberately [`short_session_id`] and not the
/// distinguishing [`session_id_prefixes`]: the label is disambiguating text for a human who has the
/// picker in front of them, not an addressable token.
#[must_use]
pub fn format_session_label(
    name: Option<&str>,
    session_id: &str,
    duplicates: &std::collections::HashSet<String>,
) -> String {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return session_id.to_string();
    };
    if duplicates.contains(&name.to_lowercase()) {
        format!("{name} ({})", short_session_id(session_id))
    } else {
        name.to_string()
    }
}

/// `sessionIdPrefixes` (`v0.10.1 index.ts:387-406`, v0.9.3 `72309e0` "fix: show unique session ID
/// prefixes"): the shortest *distinguishing* id prefix for every session in one roster.
///
/// ```text
/// for (const session of sessions) {
///   let longestSharedPrefix = 0;
///   for (const other of sessions) {
///     if (other.id === session.id) continue;
///     let length = 0;
///     while (length < session.id.length && session.id[length] === other.id[length]) length += 1;
///     longestSharedPrefix = Math.max(longestSharedPrefix, length);
///   }
///   const minimumLength = Math.max(8, longestSharedPrefix + 1);
///   const groupBoundary = session.id.indexOf("-", minimumLength);
///   const length = groupBoundary === -1 ? minimumLength : groupBoundary;
///   prefixes.set(session.id, session.id.slice(0, length));
/// }
/// ```
///
/// Mechanism notes, both load-bearing:
/// - the comparison is by **UTF-16 code unit** upstream (`session.id[length]`) — session ids are
///   UUIDs, so this port compares `char`s and slices on char boundaries, which agrees for every id
///   the broker can mint and never panics on a non-ASCII one.
/// - `indexOf("-", minimumLength)` **extends** the prefix to the next `-` group boundary, and
///   returns the boundary index itself, so the emitted prefix EXCLUDES the `-`. A boundary at or
///   after `minimumLength` therefore always lengthens (never shortens) the prefix.
#[must_use]
pub fn session_id_prefixes<'a, I>(session_ids: I) -> std::collections::HashMap<String, String>
where
    I: IntoIterator<Item = &'a str> + Clone,
{
    let mut prefixes = std::collections::HashMap::new();
    for id in session_ids.clone() {
        let chars: Vec<char> = id.chars().collect();
        let mut longest_shared_prefix = 0usize;
        for other in session_ids.clone() {
            if other == id {
                continue;
            }
            let mut length = 0usize;
            let mut other_chars = other.chars();
            for c in &chars {
                match other_chars.next() {
                    Some(o) if o == *c => length += 1,
                    _ => break,
                }
            }
            longest_shared_prefix = std::cmp::max(longest_shared_prefix, length);
        }
        let minimum_length = std::cmp::max(8, longest_shared_prefix + 1);
        // `String::indexOf(search, position)` starts AT `position`; a `minimum_length` past the end
        // simply finds nothing, which is JS's `-1` branch.
        let group_boundary = chars
            .iter()
            .enumerate()
            .skip(minimum_length)
            .find(|(_, c)| **c == '-')
            .map(|(i, _)| i);
        let length = group_boundary.unwrap_or(minimum_length);
        prefixes.insert(id.to_string(), chars.iter().take(length).collect());
    }
    prefixes
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    /// ICOM-065 — `currentHerdrPane()` (`v0.14.0 index.ts:526-529`): Herdr's own `HERDR_PANE_ID`,
    /// trimmed, and absent when unset or blank.
    #[test]
    fn the_herdr_pane_is_herdrs_own_variable_trimmed() {
        assert_eq!(
            current_herdr_pane_from(env_of(&[("HERDR_PANE_ID", "  w5:p4 \n")])),
            Some("w5:p4".to_string())
        );
        assert_eq!(
            current_herdr_pane_from(env_of(&[("HERDR_PANE_ID", "   ")])),
            None
        );
        assert_eq!(current_herdr_pane_from(env_of(&[])), None);
        assert_eq!(
            current_herdr_pane_from(env_of(&[("CYRUP_HERDR_PANE_ID", "w1:p1")])),
            None,
            "not renamed by the PI_* → CYRUP_* rule"
        );
    }

    // `v0.10.1 index.ts:425` — `.slice(0, 18)`, NOT `.slice(0, 8)` (v0.10.0 `126875e`).
    #[test]
    fn presence_name_uses_alias_for_unnamed() {
        assert_eq!(presence_name(Some("  Alice "), "id"), "Alice");
        assert_eq!(
            presence_name(None, "session-deadbeefcafef00d"),
            "subagent-chat-deadbeefcafef00d"
        );
        assert_eq!(
            presence_name(Some("   "), "abcdefghij"),
            "subagent-chat-abcdefghij"
        );
    }

    /// The whole point of v0.10.0's 8→18 widening (`126875e`, CHANGELOG 0.10.0: "Extend
    /// unnamed-session fallback aliases with enough session-ID characters to distinguish UUIDv7
    /// sessions started close together").
    ///
    /// A UUIDv7 spends its first 13 hex digits on the 48-bit unix-ms timestamp plus the version
    /// nibble, so two ids minted in the SAME millisecond are byte-identical through
    /// `0192f3c1-9a10-7`. The first field that can differ is `rand_a`, the 12 random bits filling
    /// out the third group — characters 15..18. `slice(0, 8)` stops inside the timestamp, so both
    /// sessions registered under one presence name and neither was addressable; `slice(0, 18)`
    /// reaches past the version nibble and takes all of `rand_a`, which is what separates them.
    ///
    /// The fixture therefore has to vary `rand_a` (`7a3c` vs `7f21`) — that is what a real
    /// same-millisecond mint produces. Ids differing only in the trailing random block would still
    /// collide at 18 and would pin nothing about the widening.
    #[test]
    fn unnamed_aliases_distinguish_uuidv7_ids_minted_in_the_same_millisecond() {
        let a = "0192f3c1-9a10-7a3c-8000-aaaaaaaaaaaa";
        let b = "0192f3c1-9a10-7f21-8000-bbbbbbbbbbbb";
        // Same mint millisecond: identical through the timestamp AND the version nibble.
        assert_eq!(
            &a[..15],
            &b[..15],
            "the fixture must be two same-millisecond UUIDv7 mints"
        );
        // …so the pre-fix 8-char alias could not tell them apart.
        assert_eq!(
            &a[..8],
            &b[..8],
            "slice(0, 8) lands inside the shared timestamp"
        );

        assert_ne!(presence_name(None, a), presence_name(None, b));
        assert_eq!(presence_name(None, a), "subagent-chat-0192f3c1-9a10-7a3c");
        assert_eq!(presence_name(None, b), "subagent-chat-0192f3c1-9a10-7f21");
    }

    /// `formatSessionLabel` + `duplicateSessionNames` (`v0.10.1 index.ts:379-386,440-446`) —
    /// ICOM-013's residual. The unique-name case must NOT carry an id suffix, or every peer label
    /// in the picker, the compose header and the sent-confirmation would grow one.
    #[test]
    fn a_session_label_carries_its_id_suffix_only_when_the_name_is_ambiguous() {
        let roster = [
            Some("reviewer"),
            Some("Reviewer"),
            Some("builder"),
            None,
            Some(""),
        ];
        let duplicates = duplicate_session_names(roster);
        // Case-insensitive: `reviewer` and `Reviewer` collide.
        assert!(duplicates.contains("reviewer"));
        assert!(!duplicates.contains("builder"));
        // `Boolean(name)` drops both `undefined` and `""`, so neither can ever be "duplicated" —
        // two unnamed sessions must not both start rendering an id suffix.
        assert!(!duplicates.contains(""));
        assert_eq!(duplicates.len(), 1);

        let id = "0192f3c1-9a10-7000-8000-aaaaaaaaaaaa";
        assert_eq!(
            format_session_label(Some("builder"), id, &duplicates),
            "builder"
        );
        assert_eq!(
            format_session_label(Some("Reviewer"), id, &duplicates),
            "Reviewer (0192f3c1)",
            "the original casing is preserved; only the LOOKUP is lowercased"
        );
        // `!session.name` is a truthiness test, so an absent OR empty name is the raw id — not
        // "Unnamed session", which is `formatSessionListRow`'s rule, not this one.
        assert_eq!(format_session_label(None, id, &duplicates), id);
        assert_eq!(format_session_label(Some(""), id, &duplicates), id);
    }

    /// A name held three times is still one duplicate entry — the `indexOf(name) !== index` filter
    /// keeps occurrences 2..n, and the `Set` collapses them.
    #[test]
    fn duplicate_session_names_collapses_repeats() {
        let duplicates = duplicate_session_names([Some("worker"), Some("worker"), Some("WORKER")]);
        assert_eq!(duplicates.len(), 1);
        assert!(duplicates.contains("worker"));
    }

    /// `v0.10.1 index.ts:387-406`. Two ids sharing 20 characters must get prefixes that differ, are
    /// at least 8 chars, and are extended to (but excluding) the next `-` group boundary.
    #[test]
    fn session_id_prefixes_are_distinguishing_and_group_aligned() {
        let a = "0192f3c1-9a10-7000-8000-aaaaaaaaaaaa";
        let b = "0192f3c1-9a10-7000-8000-bbbbbbbbbbbb";
        let map = session_id_prefixes([a, b]);
        let pa = map.get(a).expect("a present");
        let pb = map.get(b).expect("b present");
        assert_ne!(pa, pb, "prefixes must distinguish");
        // Longest shared prefix is 24 chars ("0192f3c1-9a10-7000-8000-"); minimum is 25; the next
        // `-` at or after index 25 does not exist, so the prefix is exactly 25 chars.
        assert_eq!(pa, "0192f3c1-9a10-7000-8000-a");
        assert_eq!(pb, "0192f3c1-9a10-7000-8000-b");
    }

    /// The `Math.max(8, …)` floor and the group-boundary extension, on ids that share nothing.
    #[test]
    fn session_id_prefixes_floor_at_eight_then_extend_to_the_group_boundary() {
        // Distinct from character 0, so `longestSharedPrefix == 0` → minimumLength 8; the next `-`
        // is at index 8, so the prefix is the first 8 chars (the `-` is excluded).
        let map = session_id_prefixes(["aaaaaaaa-1111-2222", "bbbbbbbb-1111-2222"]);
        assert_eq!(
            map.get("aaaaaaaa-1111-2222").map(String::as_str),
            Some("aaaaaaaa")
        );

        // A lone session has no `other`, so it also floors at 8.
        let solo = session_id_prefixes(["0192f3c1-9a10-7000-8000-aaaaaaaaaaaa"]);
        assert_eq!(
            solo.get("0192f3c1-9a10-7000-8000-aaaaaaaaaaaa")
                .map(String::as_str),
            Some("0192f3c1")
        );

        // An id SHORTER than the 8-char floor with no `-` yields the whole id, not a panic.
        let short = session_id_prefixes(["ab", "cd"]);
        assert_eq!(short.get("ab").map(String::as_str), Some("ab"));
    }

    /// `getNamePollMs()` (`v0.10.1 index.ts:486-495`). Unlike `getAskTimeoutMs`, an invalid value
    /// FALLS BACK rather than throwing, and a fractional value is accepted (`Number.isFinite`).
    #[test]
    fn name_poll_ms_falls_back_rather_than_erroring() {
        assert_eq!(name_poll_ms_from(|_| None), 1000);
        assert_eq!(name_poll_ms_from(|_| Some(String::new())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("abc".to_string())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("0".to_string())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("-5".to_string())), 1000);
        assert_eq!(name_poll_ms_from(|_| Some("250".to_string())), 250);
        assert_eq!(name_poll_ms_from(|_| Some("1500.5".to_string())), 1500);
    }

    /// ICOM-038 / `v0.10.1 broker/client.ts:47-50`. `Number.parseInt(x, 10)` semantics, which are
    /// NOT `Number(x)`: it stops at the first non-digit instead of rejecting the whole string.
    #[test]
    fn liveness_interval_ports_parse_int_not_number() {
        fn with(raw: &str) -> u64 {
            let raw = raw.to_string();
            liveness_interval_ms_from(&|k: &str| {
                (k == ENV_INTERCOM_LIVENESS_INTERVAL_MS).then(|| raw.clone())
            })
        }
        assert_eq!(liveness_interval_ms_from(&|_| None), 30_000);
        assert_eq!(with(""), 30_000);
        assert_eq!(with("   "), 30_000);
        assert_eq!(with("abc"), 30_000);
        assert_eq!(with("0"), 30_000);
        assert_eq!(with("-5"), 30_000);
        assert_eq!(with("250"), 250);
        // `Number("30abc")` is NaN but `parseInt("30abc", 10)` is 30 — the distinguishing case.
        assert_eq!(with("30abc"), 30);
        // `Number("1e3")` is 1000 but `parseInt("1e3", 10)` is 1 — the other distinguishing case.
        assert_eq!(with("1e3"), 1);
        assert_eq!(with("30.9"), 30);
        assert_eq!(with(" 40 "), 40);
        // Radix 10 reads `0` and stops at `x`; `0 > 0` is false, so the default applies.
        assert_eq!(with("0x10"), 30_000);
    }

    /// ICOM-038 / `v0.10.1 broker/client.ts:52-55`. The `Math.min(raw, interval)` clamp applies to a
    /// CONFIGURED timeout only; the 5 000 ms default is never clamped by a shorter interval.
    #[test]
    fn liveness_timeout_clamps_only_when_configured() {
        fn resolve(interval: Option<&str>, timeout: Option<&str>) -> u64 {
            let interval = interval.map(str::to_string);
            let timeout = timeout.map(str::to_string);
            liveness_timeout_ms_from(&|k: &str| {
                if k == ENV_INTERCOM_LIVENESS_INTERVAL_MS {
                    interval.clone()
                } else if k == ENV_INTERCOM_LIVENESS_TIMEOUT_MS {
                    timeout.clone()
                } else {
                    None
                }
            })
        }
        assert_eq!(resolve(None, None), 5_000);
        assert_eq!(resolve(None, Some("abc")), 5_000);
        assert_eq!(resolve(None, Some("0")), 5_000);
        // Clamped down to the configured interval.
        assert_eq!(resolve(Some("1000"), Some("9000")), 1_000);
        // Under the interval: kept as-is.
        assert_eq!(resolve(Some("9000"), Some("1000")), 1_000);
        // The asymmetry: an unset timeout is NOT clamped by a shorter interval.
        assert_eq!(resolve(Some("1000"), None), 5_000);
    }
}
