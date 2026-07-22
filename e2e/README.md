# Consumer image

This Axum server compiles against the crate path from the exact checkout and
runs as a non-root binary-only distroless image. The same explicit
`ObservabilityConfig` drives `ObservabilityLayer` and its stdout JSON layer.

Build the image with:

```sh
just e2e-image observability-e2e-local:manual
```

The recipe uses Podman when it is installed and otherwise falls back to Docker.
For example, run the image with Podman:

```sh
podman run --rm --name axum-observability-e2e \
  --publish 8080:8080 \
  --env OBS_E2E_CASE=common_level1 \
  --env OBS_E2E_SECRET_CANARY=audit-canary \
  observability-e2e-local:manual
```

The process accepts this public interface:

- `OBS_E2E_CASE` is required and must be exactly one of `common_level1`,
  `common_level2`, `aws_level1`, `azure_level1`, or `gcp_level1`.
- `OBS_E2E_SECRET_CANARY` is required and must be nonempty. Send its exact value
  as the bearer token; it is a leak-detection input and must not appear in
  emitted logs.
- `PORT` is optional, defaults to `8080`, and must be an integer from 1 through
  65535.
- `GET /trace` requires `Authorization: Bearer <OBS_E2E_SECRET_CANARY>`.
  A valid token returns `200` with `ok`, `request_id`, and `canary_received`
  fields. A missing or invalid token returns `401` with
  `{"error":"unauthorized"}`.

For the example above:

```sh
curl --fail --silent \
  --header 'Authorization: Bearer audit-canary' \
  http://127.0.0.1:8080/trace
```

Structured application and access records are emitted as one JSON object per
stdout line. Startup and configuration failures are diagnostics on stderr.
Building the image verifies packaging and integration only; it does not run the
server or validate emitted records.

Independent audit tooling may exercise this interface and compare the records
with this package's public logging contract. The auditor owns any
cross-implementation methodology and conclusions; this repository claims only
its own documented behavior. Running such an audit is optional and does not
constitute release approval.
