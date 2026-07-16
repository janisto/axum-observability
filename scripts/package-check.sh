#!/usr/bin/env bash
set -euo pipefail

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
package="target/package/axum-observability-${version}.crate"

cargo package --locked
cargo package --locked --list | while IFS= read -r path; do
  case "$path" in
    .cargo_vcs_info.json|Cargo.lock|Cargo.toml|Cargo.toml.orig|README.md|CHANGELOG.md|EXAMPLES.md|LICENSE|src/*|examples/*) ;;
    *) echo "unexpected packaged path: $path" >&2; exit 1 ;;
  esac
done

test -f "$package"
test "$(wc -c < "$package")" -lt 10485760

temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
tar -xzf "$package" -C "$temporary"
mkdir -p "$temporary/consumer/src"

cat > "$temporary/consumer/Cargo.toml" <<EOF
[package]
name = "axum-observability-package-smoke"
version = "0.0.0"
edition = "2024"

[dependencies]
axum = "=0.8.9"
axum-observability = { path = "../axum-observability-${version}" }
EOF

cat > "$temporary/consumer/src/main.rs" <<'EOF'
use axum::{Router, routing::get};
use axum_observability::{ObservabilityConfig, ObservabilityLayer, Preset};

fn main() {
    let config = ObservabilityConfig::default().with_preset(Preset::Gcp);
    let _app: Router = Router::new()
        .route("/", get(|| async { "ok" }))
        .layer(ObservabilityLayer::new(config));
}
EOF

cargo generate-lockfile --manifest-path "$temporary/consumer/Cargo.toml"
cargo run --locked --manifest-path "$temporary/consumer/Cargo.toml"
