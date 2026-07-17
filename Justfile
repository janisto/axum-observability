set shell := ["bash", "-ceu"]

@_:
    just --list

[group('lifecycle')]
install:
    cargo fetch --locked
    cargo install --locked cargo-deny cargo-audit cargo-mutants cargo-llvm-cov

[group('lifecycle')]
update:
    cargo update

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
workflow-check:
    actionlint
    zizmor --offline .

[group('qa')]
coverage-lcov:
    cargo llvm-cov --locked --all-features --workspace --lcov --output-path coverage.lcov

[group('qa')]
coverage-html:
    cargo llvm-cov --locked --all-features --workspace --html

[group('qa')]
coverage: coverage-lcov coverage-html

[group('qa')]
qa: workflow-check fmt-check lint test test-doc deny audit

[group('adversarial')]
mutation *args:
    cargo mutants {{ args }}
