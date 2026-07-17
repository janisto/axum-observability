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
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v brew >/dev/null && [[ "$(rustc --print sysroot)" == "$(brew --cellar rust)"/* ]]; then
        export LLVM_COV="$(brew --prefix llvm)/bin/llvm-cov"
        export LLVM_PROFDATA="$(brew --prefix llvm)/bin/llvm-profdata"
    fi
    cargo llvm-cov --locked --all-features --workspace --lcov --output-path coverage.lcov

[group('qa')]
coverage-html:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v brew >/dev/null && [[ "$(rustc --print sysroot)" == "$(brew --cellar rust)"/* ]]; then
        export LLVM_COV="$(brew --prefix llvm)/bin/llvm-cov"
        export LLVM_PROFDATA="$(brew --prefix llvm)/bin/llvm-profdata"
    fi
    cargo llvm-cov --locked --all-features --workspace --html

[group('qa')]
coverage: coverage-lcov coverage-html

[group('qa')]
qa: workflow-check fmt-check lint test test-doc deny audit

[group('adversarial')]
mutation *args:
    cargo mutants {{ args }}
