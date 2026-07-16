# Examples

Runnable examples are under `examples/`:

```bash
cargo run --example basic
cargo run --example gcp
cargo run --example aws
cargo run --example azure
cargo run --example local_wrapper
```

Each example builds an Axum router and exits; applications can use the same
composition before binding a listener. `gcp` is the recommended starting point
for Cloud Run JSON logs. `local_wrapper` demonstrates a project-owned function
that narrows request IDs and selects a custom correlation header while keeping
the crate's baseline validation and fallback.

For production, combine `config.json_layer(writer)` with the application's
chosen `EnvFilter` and writer. Create the subscriber once at process startup;
do not initialize it from request handling code or from a library.
