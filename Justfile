set shell := ["bash", "-ceu"]

@_:
    just --list

[group('lifecycle')]
install:
    cargo install --locked cargo-deny --version 0.20.2
    cargo install --locked cargo-audit --version 0.22.2
    cargo install --locked cargo-mutants --version 27.1.0
    cargo install --locked cargo-fuzz --version 0.13.2
    cargo install --locked cargo-llvm-cov --version 0.8.7

[group('qa')]
fmt:
    cargo fmt --all

[group('qa')]
fmt-check:
    cargo fmt --all -- --check

[group('qa')]
lint:
    cargo clippy --locked --all-targets --all-features -- -D warnings

[group('test')]
test:
    cargo test --locked --all-targets

[group('test')]
test-doc:
    cargo test --locked --doc

[group('qa')]
deny:
    cargo deny check

[group('qa')]
audit:
    cargo audit

[group('qa')]
coverage-lcov:
    cargo llvm-cov --locked --all-features --workspace --lcov --output-path coverage.lcov

[group('qa')]
coverage-html:
    cargo llvm-cov --locked --all-features --workspace --html

[group('qa')]
coverage: coverage-lcov coverage-html

[group('qa')]
package-check:
    scripts/package-check.sh

[group('qa')]
qa: fmt-check lint test test-doc deny audit

[group('adversarial')]
mutation *args:
    cargo mutants {{ args }}

[group('adversarial')]
fuzz target="request_id" duration="30":
    nightly_cargo="$(rustup which --toolchain nightly cargo)"; \
    nightly_bin="$(dirname "$nightly_cargo")"; \
    PATH="$nightly_bin:$PATH" "$nightly_cargo" fuzz run {{ target }} -- -max_total_time={{ duration }}
