# E2E consumer

This Axum server compiles against the crate path from the exact checkout and
runs as a non-root binary-only distroless image. The same explicit
`ObservabilityConfig` drives `ObservabilityLayer` and its stdout JSON layer.

```sh
just e2e-image observability-e2e-local:ci
```

Cross-repository assertions remain owned by the central observability project.
