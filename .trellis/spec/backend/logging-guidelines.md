# Logging Guidelines

> The foundation has no runtime logging framework; preserve that fact and keep sensitive data out of output.

## Current State

No `tracing`, `log`, or logging-subscriber dependency exists in the Rust workspace. Do not
describe structured logging, levels, retention, or telemetry as implemented.

The only explicit Rust output is the build-time binding exporter, which prints the generated
repository path after writing `src/lib/ipc/bindings.ts`. Tauri startup uses a fixed
`expect("error while running Unimail")` message. Neither output includes user data.

## Current Rules

- Tauri commands and core constructors do not print request or response payloads.
- Errors crossing IPC must be intentionally serialized and sanitized; raw debug strings are
  not a public error contract.
- Build and validation scripts may report repository-relative file names and fixed check
  results, as the existing binding and changed-path checks do.
- If runtime logging is introduced later, add the dependency, initialization point, level
  policy, redaction tests, and destination/retention behavior to this guide in the same task.

## Never Log

- OAuth client secrets, access tokens, refresh tokens, passwords, cookies, API keys, updater
  private keys, signing/notarization credentials, or certificate contents.
- Email addresses, message bodies, headers, attachment contents, search terms, or provider
  response payloads.
- Database contents, encryption keys, local mail paths, home-directory paths, device IDs,
  hostnames, or local configuration values.
- Full environment dumps or `.env` contents.

## Safe Foundation Metadata

The `application_info` response is an allowlist, not a general diagnostic dump. Its current
safe fields are application name, package version, OS family, and fixed capability labels.
Adding a field requires the IPC contract review in [Error Handling](./error-handling.md).

```rust
// Wrong: debug output can expose future sensitive fields.
println!("application info: {info:?}");

// Correct: return the allowlisted DTO without logging its payload.
ApplicationInfo::current()
```

## Review Checks

- Search new output statements and error formatting for secrets, PII, message content, and
  filesystem paths.
- Prefer fixed context messages over dumping entire objects.
- Verify test failures do not print real local mail data or credentials.
- Keep sensitive generated files out of Git; logging redaction is not a substitute for
  `npm run check:paths` and repository ignore rules.
