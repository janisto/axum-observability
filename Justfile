set shell := ["bash", "-ceu"]

@_:
    just --list

[group('lifecycle')]
install: download install-tools

[group('lifecycle')]
install-tools:
    cargo install --locked cargo-edit
    cargo install --locked cargo-llvm-cov
    cargo install --locked cargo-deny
    cargo install --locked cargo-audit
    cargo install --locked cargo-mutants
    cargo install --locked cargo-sort
    cargo install --locked cargo-machete

[group('lifecycle')]
download:
    cargo fetch --locked

[group('lifecycle')]
update:
    cargo upgrade --incompatible allow
    cargo update
    cargo fetch --locked

[group('qa')]
fmt:
    cargo fmt --all

[group('qa')]
fmt-check:
    cargo fmt --all -- --check

[group('qa')]
lint:
    cargo clippy --locked --all-targets --all-features -- -D warnings

[group('qa')]
doc:
    RUSTDOCFLAGS="-D rustdoc::all" cargo doc --locked --all-features --no-deps

[group('qa')]
sort-check:
    cargo sort --check --grouped

[group('qa')]
unused-dependencies:
    cargo machete

[group('test')]
test:
    cargo test --locked --all-targets --all-features

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
qa: workflow-check fmt-check sort-check unused-dependencies lint test test-doc doc deny audit

[group('adversarial')]
mutation *args:
    cargo mutants {{ args }}
