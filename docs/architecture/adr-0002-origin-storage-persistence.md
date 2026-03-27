# ADR-0002: Origin-scoped storage persistence for cookies, localStorage, and permission cache

- Status: Accepted
- Date: 2026-03-18

## Context

`cosmo_app_legacy` already evaluated cookie attributes and origin checks for diagnostics, but it did not keep a real storage model that could be reused across subsequent requests or script executions. That prevented:

- `Set-Cookie` from affecting later HTTP requests.
- `localStorage` from surviving the initial script execution turn for the same origin.
- origin-to-origin permission outcomes from being reused as a cacheable decision record.

The implementation needs to stay small enough for the legacy app/runtime boundary while still documenting which specification constraints are intentionally followed.

## Decision

We store browser security state in one process-local, origin-scoped persistence model:

1. **Cookie jar**
   - Keyed by the RFC 6454 serialized origin (`scheme://host:port`).
   - Stores cookie name/value plus `Secure`, `HttpOnly`, and `SameSite`.
   - Applies `Set-Cookie` updates on response receipt, including redirect responses.
   - Emits the same diagnostics as before for rejected cookies and stores only accepted cookies.

2. **localStorage**
   - Keyed by the same serialized origin.
   - Stored as string key/value pairs and injected into the JS runtime before script execution.
   - The JS runtime writes the updated snapshot back to the origin store after execution.

3. **Permission decision cache**
   - Keyed by `(initiator_origin, target_origin)`.
   - Stores the most recent allow/deny result and timestamp for sandbox-origin style decisions.

## Persistence format

The current implementation is **process-local in-memory state**:

```text
PersistentSecurityState {
  cookie_jar: HashMap<Origin, Vec<StoredCookie>>,
  local_storage: HashMap<Origin, HashMap<Key, Value>>,
  permission_cache: HashMap<(Origin, Origin), PermissionCacheEntry>,
}
```

This format is intentionally simple so the same logical schema can later be serialized to JSON or SQLite without changing the app/runtime interfaces.

## Deletion / eviction policy

- **Cookies**
  - Replaced on `(origin, cookie-name)` collision.
  - Rejected cookies are never persisted.
  - No max-age / expires eviction yet; this remains a follow-up item.
- **localStorage**
  - Replaced wholesale per origin after each script execution snapshot sync.
  - Cleared only when script code calls `localStorage.clear()` or overwrites/removes entries.
- **Permission cache**
  - Last-write-wins per `(initiator_origin, target_origin)`.
  - Timestamp retained so future TTL-based eviction can be added without schema changes.

## Specification notes

- **RFC 6265bis**
  - `SameSite=None` requires `Secure`.
  - `Secure` cookies are ignored for non-HTTPS contexts.
  - `HttpOnly` metadata is preserved but not exposed to script-visible storage APIs.
- **RFC 6454**
  - Origin comparison and persistence keys use the serialized `(scheme, host, port)` tuple.
- **HTML Standard Web Storage**
  - `localStorage` visibility is origin-scoped and script-facing.
  - This implementation exposes `getItem`, `setItem`, `removeItem`, and `clear`.

## Consequences

- Request creation can now attach origin-eligible cookies consistently.
- JS gains a minimal but real `localStorage` integration path.
- Diagnostics still explain why cookies were rejected, while accepted cookies now influence behavior.
- The model is stricter than full browser behavior because cookies are origin-scoped rather than domain/path scoped; this is an intentional compatibility/security trade-off for the legacy stack.
